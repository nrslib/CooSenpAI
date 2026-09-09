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

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use coosenpai_core::image_processing::{own_window_exclusions, process_png_sync, ImageLimits};
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    #[test]
    fn window_geometry_retains_global_logical_coordinates_across_display_scales() {
        let captured = chrono::Utc.with_ymd_and_hms(2026, 8, 28, 0, 0, 0).unwrap();
        let result = collect_bounds_from(
            captured,
            [WindowSnapshot {
                visible: true,
                scale_factor: 2.0,
                position: PhysicalPosition::new(-3800, 60),
                size: PhysicalSize::new(600, 400),
            }],
        )
        .expect("logical bounds");
        assert_eq!(result.bounds[0].x, -1900.0);
        assert_eq!(result.bounds[0].y, 30.0);
        assert_eq!(result.bounds[0].width, 300.0);
        assert!(collect_bounds_from(
            captured,
            [WindowSnapshot {
                visible: true,
                scale_factor: 0.0,
                position: PhysicalPosition::new(0, 0),
                size: PhysicalSize::new(1, 1),
            }]
        )
        .is_err());
    }

    #[test]
    fn every_visible_popup_and_details_window_is_included_and_masked() {
        for target in ["details", "capture-popup", "speech-popup", "model-popup"] {
            let captured_at = chrono::Utc.with_ymd_and_hms(2026, 8, 28, 0, 0, 0).unwrap();
            assert!(OWN_WINDOW_LABELS.contains(&target));
            let windows = OWN_WINDOW_LABELS.iter().map(|label| WindowSnapshot {
                visible: *label == target,
                scale_factor: 1.0,
                position: if *label == target {
                    PhysicalPosition::new(2, 3)
                } else {
                    PhysicalPosition::new(0, 0)
                },
                size: if *label == target {
                    PhysicalSize::new(4, 5)
                } else {
                    PhysicalSize::new(1, 1)
                },
            });
            let own_windows = collect_bounds_from(captured_at, windows).expect("own window bounds");
            let exclusions = own_window_exclusions(&own_windows, captured_at).expect("exclusions");

            let input_image = RgbaImage::from_pixel(16, 16, Rgba([255, 255, 255, 255]));
            let mut input = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(input_image)
                .write_to(&mut input, ImageFormat::Png)
                .expect("input PNG");
            let output = process_png_sync(
                &input.into_inner(),
                16,
                0,
                &exclusions,
                ImageLimits::default(),
            )
            .expect("masked PNG");
            let masked = image::load_from_memory(&output.masked_png)
                .expect("decode masked PNG")
                .to_rgba8();

            assert_eq!(masked.get_pixel(2, 3), &Rgba([0, 0, 0, 255]));
            assert_eq!(masked.get_pixel(5, 7), &Rgba([0, 0, 0, 255]));
            assert_eq!(masked.get_pixel(1, 2), &Rgba([255, 255, 255, 255]));
        }
    }
}
