//! アプリをアクティブ化しないパネル。Spotlight / Raycast と同じ手法で、
//! NSWindow を NSPanel サブクラスへ差し替えて NonactivatingPanel を styleMask に
//! 加えることで、NSApp.activate なしに key window にできる。

use anyhow::{Context, Result};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::sel;
use objc2_app_kit::{NSWindow, NSWindowAnimationBehavior, NSWindowStyleMask};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;

// クラス登録はプロセスで一度だけ。tao のウィンドウクラス（TaoWindow）は focusable
// ivar を持ち、set_focusable で書き込まれるため、差し替え後も同じ ivar が読めるよう
// 同名の ivar を追加する。canBecomeKeyWindow / canBecomeMainWindow も tao と同じく
// この ivar を読む実装にし、set_focusable(false) の意味を変えない。
fn nonactivating_panel_class() -> &'static AnyClass {
    static PANEL_CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    PANEL_CLASS.get_or_init(|| {
        let superclass = AnyClass::get(c"NSPanel").expect("NSPanel は AppKit に常に存在します");
        let mut builder = ClassBuilder::new(c"CoosenpaiNonactivatingPanel", superclass)
            .expect("パネルクラスの登録はプロセスで一度だけです");
        #[allow(deprecated)]
        extern "C" fn is_focusable(this: &AnyObject, _cmd: Sel) -> Bool {
            // SAFETY: このクラスは登録時に focusable ivar を追加しており、tao の
            // TaoWindow と同じ型（Bool）・同名でアクセスする。
            unsafe { *this.get_ivar::<Bool>("focusable") }
        }
        // SAFETY: 各セレクタの引数・戻り値のエンコーディングはこの関数シグネチャと
        // 一致し、NSPanel から継承した同名メソッドのオーバーライドになる。
        unsafe {
            builder.add_ivar::<Bool>(c"focusable");
            builder.add_method(
                sel!(canBecomeKeyWindow),
                is_focusable as extern "C" fn(_, _) -> _,
            );
            builder.add_method(
                sel!(canBecomeMainWindow),
                is_focusable as extern "C" fn(_, _) -> _,
            );
        }
        builder.register()
    })
}

/// Tauri ウィンドウの裏の NSWindow をアクティブ化しないパネルに変える。
/// 以降の表示は NSApp.activate を伴わない。非表示のうちに AppKit の
/// メインスレッドから呼ぶ。
pub fn convert_to_nonactivating_panel(native_window: *mut c_void) -> Result<()> {
    let window = NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    let window = unsafe { window.cast::<NSWindow>().as_ref() };
    let object: &AnyObject = unsafe { &*(std::ptr::from_ref(window).cast::<AnyObject>()) };
    let panel_class = nonactivating_panel_class();
    // インスタンスサイズが違うクラスへ差し替えると ivar アクセスが領域外を読み書きする。
    // tao の TaoWindow と同じレイアウト（NSPanel + focusable ivar）であることを検査する。
    let current_size = object.class().instance_size();
    let panel_size = panel_class.instance_size();
    if current_size != panel_size {
        anyhow::bail!(
            "パネルクラスとインスタンスサイズが一致しません: current={current_size} panel={panel_size}"
        );
    }
    // SAFETY: 変換後のクラスは NSPanel のサブクラスで、tao の TaoWindow と同じ
    // focusable ivar を持ち、インスタンスサイズの一致も上で確認済み。
    // tao が触るのはこの ivar と同名メソッドだけなので、差し替え後も tao の
    // ウィンドウ操作は成立する。
    unsafe { AnyObject::set_class(object, panel_class) };
    // styleMask は枠なし・アクティブ化しないパネル・contentView 全面の 3 つだけにする
    // （Miniaturizable 等の不要なビットを持ち越さない）。表示アニメーションは切る。
    // 既定の表示アニメーションだと、WindowServer 側で拡大途中の枠が固まり、画面には
    // 中央の縮小矩形（白 1px 枠つき）だけが合成される状態が観測されたため。
    window.setStyleMask(
        NSWindowStyleMask::Borderless
            | NSWindowStyleMask::NonactivatingPanel
            | NSWindowStyleMask::FullSizeContentView,
    );
    window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    Ok(())
}

/// NSApp.activate を呼ばずにパネルを前面へ出して key window にする。
/// convert_to_nonactivating_panel 済みのウィンドウに限る。AppKit のメインスレッドから呼ぶ。
pub fn order_front_and_make_key(native_window: *mut c_void) -> Result<()> {
    let window = NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    let window = unsafe { window.cast::<NSWindow>().as_ref() };
    window.orderFrontRegardless();
    window.makeKeyWindow();
    Ok(())
}

/// 再表示するパネルの key を取り直し、WebView をキーボード入力先にする。
/// 両ポインタは同じ Tauri WebviewWindow の with_webview から渡す。
/// convert_to_nonactivating_panel 済みで、AppKit のメインスレッドから呼ぶ。
pub fn show_nonactivating_webview_panel(
    native_window: *mut c_void,
    native_webview: *mut c_void,
) -> Result<()> {
    let window = NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    // SAFETY: 呼出元は同じ WebviewWindow の NSWindow と WKWebView を渡す。
    let window = unsafe { window.cast::<NSWindow>().as_ref() };
    // OS の選択 UI を経由した再表示でも、以前の key 状態を引き継がず取得し直す。
    if window.isKeyWindow() {
        window.orderOut(None);
    }
    anyhow::ensure!(
        reacquire_nonactivating_webview_panel(native_window, native_webview)?,
        "送信ポップアップをkey windowにできません"
    );
    Ok(())
}

/// パネルを隠さず key と WebView の入力先を取り直し、実際の key 所有を返す。
/// 両ポインタは同じ WebviewWindow のものとし、AppKit のメインスレッドから呼ぶ。
pub fn reacquire_nonactivating_webview_panel(
    native_window: *mut c_void,
    native_webview: *mut c_void,
) -> Result<bool> {
    let window = NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    let webview =
        NonNull::new(native_webview).context("対象ウィンドウのWebViewを取得できません")?;
    // SAFETY: 呼出元が保持する同じ WebviewWindow の NSWindow と WKWebView。
    let window = unsafe { window.cast::<NSWindow>().as_ref() };
    let webview = unsafe { webview.cast::<objc2_app_kit::NSView>().as_ref() };
    order_front_and_make_key(native_window)?;
    anyhow::ensure!(
        window.makeFirstResponder(Some(webview)),
        "送信ポップアップのWebViewにキーボード入力先を設定できません"
    );
    Ok(window.isKeyWindow())
}

/// AppKit のメインスレッド上で同期的に表示する。アプリはアクティブ化しない。
pub fn show_window_without_activation(native_window: *mut c_void) -> Result<()> {
    let window = NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    unsafe { window.cast::<NSWindow>().as_ref() }.orderFrontRegardless();
    Ok(())
}

/// AppKit のメインスレッド上で同期的に非表示にする。
pub fn hide_window(native_window: *mut c_void) -> Result<()> {
    let window = NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    unsafe { window.cast::<NSWindow>().as_ref() }.orderOut(None);
    Ok(())
}
