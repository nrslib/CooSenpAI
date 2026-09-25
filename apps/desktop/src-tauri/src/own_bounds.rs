use async_trait::async_trait;
#[cfg(not(target_os = "macos"))]
use chrono::{DateTime, Utc};
#[cfg(not(target_os = "macos"))]
use coosenpai_core::ports::WindowBounds;
use coosenpai_core::ports::{OwnWindowBounds, OwnWindowBoundsPort, PortError};
use tauri::AppHandle;
#[cfg(not(target_os = "macos"))]
use tauri::{Manager, PhysicalPosition, PhysicalSize};

#[cfg(not(target_os = "macos"))]
const OWN_WINDOW_LABELS: [&str; 7] = [
    "main",
    "bubble",
    "avatar",
    "details",
    "capture-popup",
    "speech-popup",
    "model-popup",
];

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy)]
struct WindowSnapshot {
    visible: bool,
    scale_factor: f64,
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
}

#[derive(Clone)]
pub struct TauriOwnWindowBounds {
    app: AppHandle,
    #[cfg(target_os = "macos")]
    changes: std::sync::Arc<coosenpai_platform_macos::OwnWindowChanges>,
}

impl TauriOwnWindowBounds {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            #[cfg(target_os = "macos")]
            changes: std::sync::Arc::new(coosenpai_platform_macos::OwnWindowChanges::default()),
        }
    }
}

#[cfg(target_os = "macos")]
fn collect_bounds(
    changes: &coosenpai_platform_macos::OwnWindowChanges,
) -> Result<OwnWindowBounds, PortError> {
    coosenpai_platform_macos::current_process_window_bounds(changes)
        .map_err(|error| PortError::Unavailable(error.to_string()))
}

#[cfg(not(target_os = "macos"))]
fn collect_bounds(app: &AppHandle) -> Result<OwnWindowBounds, PortError> {
    let captured_at = Utc::now();
    let mut snapshots = Vec::new();
    for label in OWN_WINDOW_LABELS {
        let Some(window) = app.get_webview_window(label) else {
            return Err(PortError::Unavailable(format!(
                "自ウィンドウ {label} を取得できません"
            )));
        };
        if !window
            .is_visible()
            .map_err(|error| PortError::Unavailable(error.to_string()))?
        {
            continue;
        }
        snapshots.push(WindowSnapshot {
            visible: true,
            scale_factor: window
                .scale_factor()
                .map_err(|error| PortError::Unavailable(error.to_string()))?,
            position: window
                .outer_position()
                .map_err(|error| PortError::Unavailable(error.to_string()))?,
            size: window
                .outer_size()
                .map_err(|error| PortError::Unavailable(error.to_string()))?,
        });
    }
    collect_bounds_from(captured_at, snapshots)
}

#[cfg(not(target_os = "macos"))]
fn collect_bounds_from(
    captured_at: DateTime<Utc>,
    windows: impl IntoIterator<Item = WindowSnapshot>,
) -> Result<OwnWindowBounds, PortError> {
    let mut logical = Vec::new();
    for window in windows {
        if !window.visible {
            continue;
        }
        if !window.scale_factor.is_finite()
            || window.scale_factor <= 0.0
            || window.size.width == 0
            || window.size.height == 0
        {
            return Err(PortError::Unavailable(
                "自ウィンドウの寸法または倍率が不正です".to_owned(),
            ));
        }
        let position = window.position.to_logical::<f64>(window.scale_factor);
        let size = window.size.to_logical::<f64>(window.scale_factor);
        logical.push(WindowBounds {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        });
    }
    Ok(OwnWindowBounds {
        revision: 0,
        captured_at,
        bounds: logical,
    })
}

#[async_trait]
impl OwnWindowBoundsPort for TauriOwnWindowBounds {
    async fn read_own_window_bounds(&self) -> Result<OwnWindowBounds, PortError> {
        #[cfg(not(target_os = "macos"))]
        let app = self.app.clone();
        #[cfg(target_os = "macos")]
        let changes = self.changes.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.app
            .run_on_main_thread(move || {
                #[cfg(target_os = "macos")]
                let bounds = collect_bounds(&changes);
                #[cfg(not(target_os = "macos"))]
                let bounds = collect_bounds(&app);
                let _ = tx.send(bounds);
            })
            .map_err(|error| PortError::Unavailable(error.to_string()))?;
        tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .map_err(|_| PortError::Timeout)?
            .map_err(|error| PortError::Unavailable(error.to_string()))?
    }
}
