use anyhow::{Context, Result};
use coosenpai_core::ports::ForegroundApplication;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWindow, NSWorkspace};
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

pub fn activate_application(identity: &ForegroundApplication) -> Result<()> {
    let application =
        NSRunningApplication::runningApplicationWithProcessIdentifier(identity.process_id)
            .context("元の前面アプリが終了しています")?;
    activate_running_application(&application, "元の前面アプリを再アクティブ化できません")
}

pub fn activate_current_application() -> Result<()> {
    let application = NSRunningApplication::currentApplication();
    activate_running_application(&application, "自アプリを前面にできません")
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
