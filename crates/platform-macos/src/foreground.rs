use anyhow::{Context, Result};
use coosenpai_core::ports::ForegroundApplication;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSWindow, NSWorkspace,
};
use std::{ffi::c_void, ptr::NonNull};

pub fn frontmost_application_identity() -> Option<ForegroundApplication> {
    let application = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(ForegroundApplication {
        process_id: application.processIdentifier(),
        bundle_id: application
            .bundleIdentifier()
            .map(|value| value.to_string()),
    })
}

pub fn application_is_running(identity: &ForegroundApplication) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(identity.process_id).is_some_and(
        |application| {
            !application.isTerminated()
                && application
                    .bundleIdentifier()
                    .map(|bundle| bundle.to_string())
                    == identity.bundle_id
        },
    )
}

pub fn activate_application(identity: &ForegroundApplication) -> Result<()> {
    let application =
        NSRunningApplication::runningApplicationWithProcessIdentifier(identity.process_id)
            .context("元の前面アプリが終了しています")?;
    activate_running_application(&application, "元の前面アプリを再アクティブ化できません")
}

/// 自アプリから対象アプリへアクティブ状態を引き渡す。完了は呼出側で観測する。
pub fn return_activation_to_application(identity: &ForegroundApplication) -> Result<()> {
    let marker = MainThreadMarker::new().context("AppKitのメインスレッドではありません")?;
    let target = NSRunningApplication::runningApplicationWithProcessIdentifier(identity.process_id)
        .context("元の前面アプリが終了しています")?;
    if target.isActive() && !NSApplication::sharedApplication(marker).isActive() {
        return Ok(());
    }
    if objc2::available!(macos = 14.0) {
        NSApplication::sharedApplication(marker).yieldActivationToApplication(&target);
        anyhow::ensure!(
            target.activateFromApplication_options(
                &NSRunningApplication::currentApplication(),
                NSApplicationActivationOptions::empty(),
            ),
            "元の前面アプリへのアクティブ状態の引渡しが拒否されました"
        );
        Ok(())
    } else {
        activate_running_application(&target, "元の前面アプリを再アクティブ化できません")
    }
}

pub fn activate_current_application() -> Result<bool> {
    let marker = MainThreadMarker::new().context("AppKitのメインスレッドではありません")?;
    if objc2::available!(macos = 14.0) {
        let application = NSApplication::sharedApplication(marker);
        application.activate();
        // macOS 14 以降の activate は完了状態を返さないため、直後の状態を診断値にする。
        return Ok(application.isActive());
    }

    // macOS 13 を引き続きサポートするため、14 未満だけ旧 API を使う。
    let application = NSRunningApplication::currentApplication();
    #[allow(deprecated)]
    let _ =
        application.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    Ok(application.isActive())
}

// アクティブ済みのアプリへの NSApp.activate はそれ自体がアプリ切り替えの遷移を起こし、
// 直後の画面撮影を遅らせる（macOS 26.5.1 実測）ため、既にアクティブなら呼ばない。
pub fn activate_current_application_unless_active() -> Result<bool> {
    let marker = MainThreadMarker::new().context("AppKitのメインスレッドではありません")?;
    let is_active = NSApplication::sharedApplication(marker).isActive();
    unless_active(is_active, activate_current_application)
}

fn unless_active(is_active: bool, activate: impl FnOnce() -> Result<bool>) -> Result<bool> {
    if is_active {
        Ok(true)
    } else {
        activate()
    }
}

/// 自アプリが今アクティブか。AppKit のメインスレッドから呼ぶ。
pub fn current_application_is_active() -> Result<bool> {
    let marker = MainThreadMarker::new().context("AppKitのメインスレッドではありません")?;
    Ok(NSApplication::sharedApplication(marker).isActive())
}

/// アプリ自体を再 activate せず、対象ウィンドウだけを表示して key 化する。
pub fn make_key_and_order_front(native_window: *mut c_void) -> Result<()> {
    let native_window =
        NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    let native_window = unsafe { native_window.cast::<NSWindow>().as_ref() };
    native_window.makeKeyAndOrderFront(None);
    Ok(())
}

/// AppKit のメインスレッドから呼び出し、指定したウィンドウを背面へ戻す。
pub fn order_window_back(native_window: *mut c_void) -> Result<()> {
    let native_window =
        NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
    let native_window = unsafe { native_window.cast::<NSWindow>().as_ref() };
    native_window.orderBack(None);
    Ok(())
}

fn activate_running_application(
    application: &NSRunningApplication,
    error_message: &str,
) -> Result<()> {
    #[allow(deprecated)]
    let accepted =
        application.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    if !accepted {
        anyhow::bail!("{error_message}")
    }
    Ok(())
}

