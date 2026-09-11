//! macOS の capture、OCR、通知、workspace 通知を core の trait に接続する。

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
mod adapters;

#[cfg(target_os = "macos")]
mod foreground;

#[cfg(target_os = "macos")]
mod application_activation;

#[cfg(target_os = "macos")]
pub use application_activation::ApplicationActivationMonitor;

#[cfg(target_os = "macos")]
mod termination;

#[cfg(target_os = "macos")]
mod speech;

#[cfg(target_os = "macos")]
mod hearing;

#[cfg(target_os = "macos")]
mod speech_permissions;

#[cfg(target_os = "macos")]
mod hearing_permissions;

#[cfg(target_os = "macos")]
mod audio_permissions;

#[cfg(target_os = "macos")]
mod speech_devices;

#[cfg(target_os = "macos")]
mod clipboard;

#[cfg(target_os = "macos")]
mod keychain;

#[cfg(target_os = "macos")]
mod window_info;
pub use window_info::{
    screenshot_selection_windows, SelectionWindow, SelectionWindowCandidate,
    SelectionWindowObservation,
};

#[cfg(target_os = "macos")]
mod display_capture;

#[cfg(target_os = "macos")]
mod watch_capture;

#[cfg(target_os = "macos")]
pub use watch_capture::{capture_application_window, capture_screen};

#[cfg(target_os = "macos")]
mod panel;
#[cfg(target_os = "macos")]
mod window_key;
#[cfg(target_os = "macos")]
pub use window_key::WindowKeyMonitor;

#[cfg(target_os = "macos")]
mod mouse_monitor;

#[cfg(target_os = "macos")]
mod screen;

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "macos")]
pub use panel::*;

#[cfg(target_os = "macos")]
pub use mouse_monitor::*;

#[cfg(target_os = "macos")]
pub use adapters::*;

#[cfg(target_os = "macos")]
pub use foreground::*;

#[cfg(target_os = "macos")]
pub use termination::*;

#[cfg(target_os = "macos")]
pub use speech::*;

#[cfg(target_os = "macos")]
pub use hearing::*;

#[cfg(target_os = "macos")]
pub use speech_permissions::*;

#[cfg(target_os = "macos")]
pub use hearing_permissions::MacHearingPermissions;

#[cfg(target_os = "macos")]
pub use speech_devices::*;

#[cfg(target_os = "macos")]
pub use clipboard::*;

#[cfg(target_os = "macos")]
pub use keychain::*;

#[cfg(target_os = "macos")]
pub use screen::*;

#[cfg(not(target_os = "macos"))]
pub fn platform_is_available() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub async fn open_external_url(_url: &str) -> Result<(), coosenpai_core::ports::PortError> {
    Err(coosenpai_core::ports::PortError::Unavailable(
        "外部リンクは macOS でのみ開けます".to_owned(),
    ))
}

#[cfg(not(target_os = "macos"))]
pub async fn open_file(_path: &std::path::Path) -> Result<(), coosenpai_core::ports::PortError> {
    Err(coosenpai_core::ports::PortError::Unavailable(
        "ファイルは macOS でのみ開けます".to_owned(),
    ))
}

#[cfg(target_os = "macos")]
mod voice_output;
#[cfg(target_os = "macos")]
pub use voice_output::MacSystemVoiceProvider;

#[cfg(target_os = "macos")]
mod voicevox;
#[cfg(target_os = "macos")]
pub use voicevox::{voicevox_voices, VoicevoxProvider, VoicevoxVoice};

#[cfg(target_os = "macos")]
pub mod region_screenshot;
#[cfg(target_os = "macos")]
pub use region_screenshot::SCREENSHOT_ACCESS_REQUIRED;

#[cfg(target_os = "macos")]
mod own_windows;

#[cfg(target_os = "macos")]
pub use own_windows::current_process_window_bounds;

#[cfg(target_os = "macos")]
mod own_window_changes;

#[cfg(target_os = "macos")]
pub use own_window_changes::OwnWindowChanges;
