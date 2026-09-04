use crate::state::DesktopState;
use coosenpai_core::image_processing::png_dimensions;
use coosenpai_core::ports::{ForegroundApplicationPort, RuntimeLogger};
use tauri::Manager;

pub(super) fn normalize_message(message: String) -> String {
    message.trim().to_owned()
}

pub(super) fn capture_origin(state: &DesktopState) -> super::CaptureOrigin {
    let main_was_foreground = state.app.get_webview_window("main").is_some_and(|window| {
        window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
    });
    super::CaptureOrigin {
        main_was_foreground,
        frontmost_application: crate::platform::MacForegroundApplications
            .frontmost_application()
            .ok()
            .flatten(),
    }
}

pub(super) fn dismiss_capture_popup(
    port: &dyn ForegroundApplicationPort,
    origin: &super::CaptureOrigin,
    hide_popup: impl FnOnce(),
) {
    if !origin.main_was_foreground {
        if let Some(application) = &origin.frontmost_application {
            let _ = port.activate_application(application);
        }
    }
    hide_popup();
}

pub(super) fn close_capture_popup(state: &DesktopState, origin: &super::CaptureOrigin) {
    let before = main_window_presentation(state);
    dismiss_capture_popup(&crate::platform::MacForegroundApplications, origin, || {
        if let Some(window) = state.app.get_webview_window("capture-popup") {
            let _ = window.hide();
        }
    });
    let after = main_window_presentation(state);
    let _ = state.logger.write(
        "INFO",
        &format!(
            "送信ポップアップを閉じました: main-visible={}→{} main-focused={}→{} origin-main={}",
            before.0, after.0, before.1, after.1, origin.main_was_foreground
        ),
    );
}

fn main_window_presentation(state: &DesktopState) -> (bool, bool) {
    state
        .app
        .get_webview_window("main")
        .map(|window| {
            (
                window.is_visible().unwrap_or(false),
                window.is_focused().unwrap_or(false),
            )
        })
        .unwrap_or((false, false))
}

pub(super) fn validate_captured_file(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file() && metadata.len() <= super::MAX_PREVIEW_BYTES)
        .and_then(|_| std::fs::read(path).ok())
        .and_then(|bytes| png_dimensions(&bytes))
        .is_some()
}
