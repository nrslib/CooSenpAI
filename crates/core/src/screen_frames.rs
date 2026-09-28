use crate::config::Config;
use crate::image_processing::{png_dimensions, process_png, ExcludedBounds, ProcessedImage};
use crate::ports::{
    CapturedScreen, OcrPort, OwnWindowContext, OwnWindowFrame, OwnWindowImageRect, ScreenDisplay,
};
use crate::watch_coordinator::{normalize_ocr_blocks, OcrText};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const MENU_BAR_HEIGHT: f64 = 28.0;

pub struct PreparedScreenFrame {
    pub context_id: String,
    pub display: ScreenDisplay,
    pub image: ProcessedImage,
    pub provider_width: u32,
    pub provider_height: u32,
    pub provider_path: PathBuf,
    pub ocr: Option<OcrText>,
    pub own_window_context: Option<OwnWindowContext>,
}

impl PreparedScreenFrame {
    #[allow(clippy::too_many_arguments)]
    pub fn observation_frame(
        &self,
        scope_generation: u64,
        captured_at: chrono::DateTime<chrono::Utc>,
        relative_seconds: f64,
        trigger: crate::state::ActivityTriggerKind,
        front_app: Option<String>,
        focus: Option<crate::ports::FocusElement>,
        debug_enabled: bool,
    ) -> crate::observer::ObservationFrameInput {
        crate::observer::ObservationFrameInput {
            display: Some(self.display),
            window_id: None,
            window_bounds: None,
            scope_generation,
            context_id: self.context_id.clone(),
            captured_at,
            debug_id: debug_enabled.then(|| self.context_id.clone()),
            relative_seconds,
            trigger,
            front_app,
            app: None,
            target: "fullscreen".to_owned(),
            ocr_text: self.ocr.as_ref().map(|ocr| ocr.text.clone()),
            focus,
            own_window_context: self.own_window_context.clone(),
            image_path: self.provider_path.clone(),
        }
    }
}

pub struct PreparedScreenFrames {
    pub frames: Vec<PreparedScreenFrame>,
    pub comparison_hash: String,
    pub ocr_signature: Option<String>,
}

pub fn display_exclusions(
    display: ScreenDisplay,
    width: u32,
    height: u32,
    exclusions: &[ExcludedBounds],
) -> Vec<ExcludedBounds> {
    let scale_x = f64::from(width) / display.bounds.width;
    let scale_y = f64::from(height) / display.bounds.height;
    exclusions
        .iter()
        .map(|bounds| ExcludedBounds {
            x: (bounds.x - display.bounds.x) * scale_x,
            y: (bounds.y - display.bounds.y) * scale_y,
            width: bounds.width * scale_x,
            height: bounds.height * scale_y,
        })
        .collect()
}

pub fn display_own_window_rectangles(
    display: ScreenDisplay,
    source_width: u32,
    source_height: u32,
    image_width: u32,
    image_height: u32,
    windows: &[OwnWindowFrame],
) -> Vec<OwnWindowImageRect> {
    let scale_x = f64::from(source_width) / display.bounds.width;
    let scale_y = f64::from(source_height) / display.bounds.height;
    let mut rectangles = Vec::new();
    for window in windows {
        let bounds = window.bounds;
        let display_right = display.bounds.x + display.bounds.width;
        let menu_bar_bottom = display.bounds.y + MENU_BAR_HEIGHT;
        if bounds.x >= display.bounds.x
            && bounds.x + bounds.width <= display_right
            && bounds.y >= display.bounds.y
            && bounds.y + bounds.height <= menu_bar_bottom
        {
            continue;
        }
        let left = ((bounds.x - display.bounds.x) * scale_x)
            .floor()
            .max(0.0)
            .min(f64::from(source_width)) as u32;
        let top = ((bounds.y - display.bounds.y) * scale_y)
            .floor()
            .max(0.0)
            .min(f64::from(source_height)) as u32;
        let right = ((bounds.x + bounds.width - display.bounds.x) * scale_x)
            .ceil()
            .max(0.0)
            .min(f64::from(source_width)) as u32;
        let bottom = ((bounds.y + bounds.height - display.bounds.y) * scale_y)
            .ceil()
            .max(0.0)
            .min(f64::from(source_height)) as u32;
        if right <= left || bottom <= top {
            continue;
        }
        let image_left = ((f64::from(left) * f64::from(image_width) / f64::from(source_width))
            .ceil() as u32)
            .min(image_width);
        let image_top = ((f64::from(top) * f64::from(image_height) / f64::from(source_height))
            .ceil() as u32)
            .min(image_height);
        let image_right = ((f64::from(right) * f64::from(image_width) / f64::from(source_width))
            .ceil() as u32)
            .min(image_width);
        let image_bottom =
            ((f64::from(bottom) * f64::from(image_height) / f64::from(source_height)).ceil()
                as u32)
                .min(image_height);
        if image_right > image_left && image_bottom > image_top {
            rectangles.push(OwnWindowImageRect {
                kind: window.kind,
                x: image_left,
                y: image_top,
                width: image_right - image_left,
                height: image_bottom - image_top,
            });
        }
    }
    rectangles
}

pub fn attach_own_window_context(
    frames: &mut [PreparedScreenFrame],
    windows: &[OwnWindowFrame],
    user_response_in_progress: Option<bool>,
) {
    for frame in frames {
        let own_windows = display_own_window_rectangles(
            frame.display,
            frame.image.width,
            frame.image.height,
            frame.provider_width,
            frame.provider_height,
            windows,
        );
        frame.own_window_context = (!own_windows.is_empty() || user_response_in_progress.is_some())
            .then_some(OwnWindowContext {
                windows: own_windows,
                user_response_in_progress,
            });
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_screen_frames(
    mut captures: Vec<CapturedScreen>,
    directory: &Path,
    config: &Config,
    exclusions: &[ExcludedBounds],
    ocr: &dyn OcrPort,
    ocr_enabled: bool,
    semaphore: Arc<Semaphore>,
    cancellation: &CancellationToken,
) -> Result<PreparedScreenFrames> {
    anyhow::ensure!(!captures.is_empty(), "撮影結果にディスプレイがありません");
    captures.sort_by_key(|capture| capture.display.id);
    anyhow::ensure!(
        captures
            .windows(2)
            .all(|pair| pair[0].display.id != pair[1].display.id),
        "撮影結果のディスプレイ ID が重複しています"
    );
    let mut frames = Vec::with_capacity(captures.len());
    let mut image_hash = Sha256::new();
    let mut ocr_hash = Sha256::new();
    let mut all_ocr_succeeded = ocr_enabled;
    for capture in captures {
        ensure_active(cancellation)?;
        let bounds = capture.display.bounds;
        anyhow::ensure!(
            bounds.x.is_finite()
                && bounds.y.is_finite()
                && bounds.width.is_finite()
                && bounds.height.is_finite()
                && bounds.width > 0.0
                && bounds.height > 0.0,
            "ディスプレイの寸法が不正です"
        );
        let bytes = tokio::fs::read(&capture.path).await?;
        let (width, height) = png_dimensions(&bytes).context("画面 PNG が不正です")?;
        let mut excluded = display_exclusions(capture.display, width, height, exclusions);
        let ignored_top = (MENU_BAR_HEIGHT * f64::from(height) / bounds.height)
            .round()
            .max(1.0) as u32;
        excluded.push(ExcludedBounds {
            x: 0.0,
            y: 0.0,
            width: f64::from(width),
            height: f64::from(ignored_top),
        });
        let image = process_png(
            bytes,
            config.watch.downscale_width,
            ignored_top,
            excluded.clone(),
            semaphore.clone(),
        )
        .await?;
        let (provider_width, provider_height) =
            png_dimensions(&image.provider_png).context("縮小後の画面 PNG が不正です")?;
        ensure_active(cancellation)?;
        let provider_path = directory.join(format!("provider-{}.png", capture.display.id));
        tokio::fs::write(&provider_path, &image.provider_png).await?;
        let ocr_text = if ocr_enabled {
            let ocr_path = directory.join(format!("ocr-{}.png", capture.display.id));
            tokio::fs::write(&ocr_path, &image.masked_png).await?;
            ensure_active(cancellation)?;
            ocr.recognize(
                &ocr_path,
                &config.watch.ocr_gate.level,
                Duration::from_millis(config.watch.ocr_gate.timeout_ms),
                cancellation.clone(),
            )
            .await
            .ok()
            .map(|blocks| normalize_ocr_blocks(&blocks, width, height, &excluded, ignored_top))
        } else {
            None
        };
        ensure_active(cancellation)?;
        let identity = format!(
            "{}:{},{},{},{}:{width}x{height};",
            capture.display.id, bounds.x, bounds.y, bounds.width, bounds.height
        );
        image_hash.update(identity.as_bytes());
        image_hash.update(image.comparison_hash.as_bytes());
        if let Some(text) = &ocr_text {
            ocr_hash.update(identity.as_bytes());
            ocr_hash.update(text.signature.as_bytes());
        } else {
            all_ocr_succeeded = false;
        }
        frames.push(PreparedScreenFrame {
            context_id: crate::debug::DebugStore::new_id(),
            display: capture.display,
            image,
            provider_width,
            provider_height,
            provider_path,
            ocr: ocr_text,
            own_window_context: None,
        });
    }
    Ok(PreparedScreenFrames {
        frames,
        comparison_hash: format!("{:x}", image_hash.finalize()),
        ocr_signature: all_ocr_succeeded.then(|| format!("{:x}", ocr_hash.finalize())),
    })
}

fn ensure_active(cancellation: &CancellationToken) -> Result<()> {
    anyhow::ensure!(
        !cancellation.is_cancelled(),
        "見守りの撮影が取り消されました"
    );
    Ok(())
}
