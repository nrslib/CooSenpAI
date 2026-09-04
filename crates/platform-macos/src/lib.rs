//! macOS の capture、OCR、通知、workspace 通知を core の trait に接続する。

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
mod adapters;

#[cfg(target_os = "macos")]
mod foreground;

#[cfg(target_os = "macos")]
mod termination;

#[cfg(target_os = "macos")]
mod speech;

#[cfg(target_os = "macos")]
mod hearing;

#[cfg(target_os = "macos")]
mod speech_permissions;

#[cfg(target_os = "macos")]
mod speech_devices;

#[cfg(target_os = "macos")]
mod clipboard;

#[cfg(target_os = "macos")]
mod keychain;

#[cfg(target_os = "macos")]
mod window_info;

#[cfg(target_os = "macos")]
mod screen;

#[cfg(target_os = "macos")]
pub use macos::*;

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
