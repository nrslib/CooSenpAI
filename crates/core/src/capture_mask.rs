use crate::image_processing::{own_window_exclusions, ExcludedBounds};
use crate::ports::{CapturedScreen, OwnWindowBoundsPort, PortError};
use chrono::{DateTime, Utc};
use std::future::Future;

pub struct StableScreenCapture {
    pub screens: Vec<CapturedScreen>,
    pub exclusions: Vec<ExcludedBounds>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum CaptureWithMaskError {
    OwnWindows,
    Capture(PortError),
}

/// 全画面の取得一式を枠のスナップショットで囲み、途中の表示・移動を検出したら渡さない。
pub async fn capture_with_window_mask(
    own_windows: &dyn OwnWindowBoundsPort,
    capture: impl Future<Output = Result<Vec<CapturedScreen>, PortError>>,
) -> Result<StableScreenCapture, CaptureWithMaskError> {
    let before = own_windows
        .read_own_window_bounds()
        .await
        .map_err(|_| CaptureWithMaskError::OwnWindows)?;
    let captured_at = Utc::now();
    let exclusions =
        own_window_exclusions(&before, captured_at).ok_or(CaptureWithMaskError::OwnWindows)?;
    let screens = capture.await.map_err(CaptureWithMaskError::Capture)?;
    let after = own_windows
        .read_own_window_bounds()
        .await
        .map_err(|_| CaptureWithMaskError::OwnWindows)?;
    if before.revision != after.revision
        || before.bounds != after.bounds
        || !after.is_fresh_at(Utc::now())
    {
        return Err(CaptureWithMaskError::OwnWindows);
    }
    Ok(StableScreenCapture {
        screens,
        exclusions,
        captured_at,
    })
}

