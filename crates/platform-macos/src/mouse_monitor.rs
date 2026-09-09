//! 自アプリと他アプリへのクリックを観測する NSEvent の監視。送信ポップアップが
//! key になっていなくても、ポップアップ外のクリックを検知して閉じるために使う。
//! ローカル監視も元のイベントを返し、利用者の操作を妨げない。

use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSWindow};
use objc2_foundation::{NSPoint, NSRect};
use std::ffi::c_void;
use std::ptr::NonNull;

/// 監視で観測したクリック。座標は AppKit のスクリーン座標（左下原点、ポイント）で、
/// ポップアップの frame も同じ座標系で渡す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseClick {
    pub x: f64,
    pub y: f64,
    pub window_x: f64,
    pub window_y: f64,
    pub window_width: f64,
    pub window_height: f64,
}

/// 自アプリと他アプリの両方を監視する。非表示時にメインスレッドで `remove` を呼ぶ。
pub struct MouseDownMonitor {
    tokens: MonitorPair,
}

// SAFETY: retained ポインタは解除まで保持するだけで、操作はメインスレッドで行う。
unsafe impl Send for MouseDownMonitor {}

struct MonitorPair {
    global: usize,
    local: usize,
}

impl MonitorPair {
    fn install(
        global: impl FnOnce() -> Result<usize>,
        local: impl FnOnce() -> Result<usize>,
        remove: impl FnOnce(usize),
    ) -> Result<Self> {
        let global = global()?;
        match local() {
            Ok(local) => Ok(Self { global, local }),
            Err(error) => {
                remove(global);
                Err(error)
            }
        }
    }

    fn remove(self, mut remove: impl FnMut(usize)) {
        remove(self.global);
        remove(self.local);
    }
}

impl MouseDownMonitor {
    pub fn remove(self) -> Result<()> {
        self.tokens.remove(remove_token);
        Ok(())
    }
}

fn remove_token(token: usize) {
    // SAFETY: install が保持した NSEvent の非nullトークンを一度だけ取り戻す。
    unsafe {
        let token = Retained::from_raw(token as *mut AnyObject).expect("監視トークン");
        NSEvent::removeMonitor(&token);
    }
}

/// メインスレッドから登録する。左右・その他のボタンを両方の配送経路で観測する。
pub fn install_mouse_down_monitor(
    native_window: *mut c_void,
    handler: impl Fn(MouseClick) + Send + Sync + 'static,
) -> Result<MouseDownMonitor> {
    let window = NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    let window_ptr = window.as_ptr() as usize;
    let click_handler = std::sync::Arc::new(move |event: NonNull<NSEvent>| {
        // SAFETY: event はコールバック中、window は監視解除まで有効。
        let (location, frame) = unsafe {
            let window = &*(window_ptr as *const NSWindow);
            (screen_location(event.as_ref()), window.frame())
        };
        handler(MouseClick {
            x: location.x,
            y: location.y,
            window_x: frame.origin.x,
            window_y: frame.origin.y,
            window_width: frame.size.width,
            window_height: frame.size.height,
        });
    });
    let global_handler = click_handler.clone();
    let global = block2::RcBlock::new(move |event: NonNull<NSEvent>| global_handler(event));
    let local = block2::RcBlock::new(move |event: NonNull<NSEvent>| {
        click_handler(event);
        event.as_ptr()
    });
    let mask =
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
    let tokens = MonitorPair::install(
        || {
            NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global)
                .map(|token| Retained::into_raw(token) as usize)
                .context("グローバルなマウス監視を登録できません")
        },
        || {
            // SAFETY: local は渡された有効なイベントをそのまま返す。
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) }
                .map(|token| Retained::into_raw(token) as usize)
                .context("ローカルなマウス監視を登録できません")
        },
        remove_token,
    )?;
    Ok(MouseDownMonitor { tokens })
}

/// グローバル監視のイベントは他アプリのもので window を持たず、locationInWindow が
/// スクリーン座標になる。window を持つ場合はスクリーン座標へ変換する。
unsafe fn screen_location(event: &NSEvent) -> NSPoint {
    let location = event.locationInWindow();
    let marker = MainThreadMarker::new_unchecked();
    match event.window(marker) {
        Some(window) => {
            let rect = window.convertRectToScreen(NSRect::new(
                location,
                objc2_foundation::NSSize::new(0.0, 0.0),
            ));
            rect.origin
        }
        None => location,
    }
}

