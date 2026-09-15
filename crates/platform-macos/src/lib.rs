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
mod focus_element;

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
pub use focus_element::MacFocusedElement;

#[cfg(target_os = "macos")]
pub use keychain::*;

#[cfg(target_os = "macos")]
pub use screen::*;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::fs::{File, OpenOptions};
    use std::future::Future;
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    pub const PROCESS_TEST_TIMEOUT: Duration = Duration::from_secs(30);

    pub struct HelperProcessLock {
        file: File,
    }

    impl Drop for HelperProcessLock {
        fn drop(&mut self) {
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }

    pub async fn acquire_helper_process_lock() -> HelperProcessLock {
        tokio::task::spawn_blocking(|| {
            let path = std::env::temp_dir().join("coosenpai-helper-process-tests.lock");
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(path)
                .expect("helper process test lock");
            let deadline = std::time::Instant::now() + PROCESS_TEST_TIMEOUT;
            loop {
                let result =
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    return HelperProcessLock { file };
                }
                let error = std::io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(code)
                        if code == libc::EAGAIN
                            || code == libc::EWOULDBLOCK
                            || code == libc::EINTR => {}
                    _ => panic!("helper process test lock: {error}"),
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "helper process test lock timeout"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        })
        .await
        .expect("helper process test lock task")
    }

    async fn run_with_real_time_fail_safe<F>(operation: F) -> Result<F::Output, ()>
    where
        F: Future,
    {
        let (finished, waiting) = std::sync::mpsc::channel::<()>();
        let (expired, expired_receiver) = tokio::sync::oneshot::channel();
        let watchdog = std::thread::spawn(move || {
            if matches!(
                waiting.recv_timeout(PROCESS_TEST_TIMEOUT),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                let _ = expired.send(());
            }
        });
        let result = {
            tokio::pin!(operation);
            tokio::pin!(expired_receiver);
            tokio::select! {
                result = &mut operation => Ok(result),
                _ = &mut expired_receiver => Err(()),
            }
        };
        drop(finished);
        watchdog.join().expect("real-time fail-safe thread");
        result
    }

    pub async fn wait_for_task_completion_with_real_time_fail_safe<T>(
        mut task: tokio::task::JoinHandle<T>,
        description: &'static str,
    ) -> Result<T, tokio::task::JoinError>
    where
        T: Send + 'static,
    {
        match run_with_real_time_fail_safe(&mut task).await {
            Ok(result) => result,
            Err(()) => match run_with_real_time_fail_safe(&mut task).await {
                Ok(result) => result,
                Err(()) => panic!("{description}: task cleanup real-time fail-safe timeout"),
            },
        }
    }

    pub async fn with_real_time_fail_safe_and_cleanup<F, C, E>(
        operation: F,
        cleanup: C,
        description: &'static str,
    ) -> F::Output
    where
        F: Future,
        C: FnOnce() -> tokio::task::JoinHandle<Result<(), E>>,
        E: std::fmt::Display + Send + 'static,
    {
        match run_with_real_time_fail_safe(operation).await {
            Ok(result) => result,
            Err(()) => {
                match wait_for_task_completion_with_real_time_fail_safe(cleanup(), description)
                    .await
                {
                    Ok(Ok(())) => panic!("{description}: real-time fail-safe timeout"),
                    Ok(Err(error)) => panic!("{description}: cleanup failed: {error}"),
                    Err(error) => panic!("{description}: cleanup task failed: {error}"),
                }
            }
        }
    }
}

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
