use crate::platform::MacOwnWindowBounds;
use async_trait::async_trait;
use coosenpai_core::ports::{OwnWindowBounds, OwnWindowBoundsPort, PortError, WindowBounds};
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};

#[derive(Clone)]
pub struct TauriOwnWindowBounds {
    app: AppHandle,
    cached: Arc<RwLock<Option<OwnWindowBounds>>>,
}

impl TauriOwnWindowBounds {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            cached: Arc::new(RwLock::new(None)),
        }
    }

    pub fn request_refresh(&self) -> tauri::Result<()> {
        let app = self.app.clone();
        let cached = self.cached.clone();
        self.app.run_on_main_thread(move || {
            let next = collect_bounds(&app).ok();
            if let Ok(mut guard) = cached.write() {
                *guard = next;
            }
        })
    }

    fn cached(&self) -> Result<OwnWindowBounds, PortError> {
        self.cached
            .read()
            .map_err(|_| {
                PortError::Unavailable("自ウィンドウ bounds のロックが壊れました".to_owned())
            })?
            .clone()
            .ok_or_else(|| {
                PortError::Unavailable("自ウィンドウ bounds を取得できません".to_owned())
            })
    }
}

fn collect_bounds(app: &AppHandle) -> Result<OwnWindowBounds, PortError> {
    let captured_at = chrono::Utc::now();
    let mut physical = Vec::new();
    for label in ["main", "bubble"] {
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
        let scale = window
            .scale_factor()
            .map_err(|error| PortError::Unavailable(error.to_string()))?;
        let position: PhysicalPosition<i32> = window
            .outer_position()
            .map_err(|error| PortError::Unavailable(error.to_string()))?;
        let size: PhysicalSize<u32> = window
            .outer_size()
            .map_err(|error| PortError::Unavailable(error.to_string()))?;
        let logical_position = position.to_logical::<f64>(scale);
        let logical_size = size.to_logical::<f64>(scale);
        let converted = MacOwnWindowBounds::to_physical(
            captured_at,
            &[WindowBounds {
                x: logical_position.x,
                y: logical_position.y,
                width: logical_size.width,
                height: logical_size.height,
            }],
            scale,
        )?;
        physical.extend(converted.bounds);
    }
    Ok(OwnWindowBounds {
        captured_at,
        bounds: physical,
    })
}

#[async_trait]
impl OwnWindowBoundsPort for TauriOwnWindowBounds {
    async fn read_own_window_bounds(&self) -> Result<OwnWindowBounds, PortError> {
        self.cached()
    }
}

