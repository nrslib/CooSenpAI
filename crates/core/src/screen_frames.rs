use crate::config::Config;
use crate::image_processing::{png_dimensions, process_png, ExcludedBounds, ProcessedImage};
use crate::ports::{CapturedScreen, OcrPort, ScreenDisplay};
use crate::watch_coordinator::{normalize_ocr_blocks, OcrText};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub struct PreparedScreenFrame {
    pub context_id: String,
    pub display: ScreenDisplay,
    pub image: ProcessedImage,
    pub provider_path: PathBuf,
    pub ocr: Option<OcrText>,
}

impl PreparedScreenFrame {
    pub fn observation_frame(
        &self,
        scope_generation: u64,
        captured_at: chrono::DateTime<chrono::Utc>,
        relative_seconds: f64,
        trigger: crate::state::ActivityTriggerKind,
        front_app: Option<String>,
        debug_enabled: bool,
    ) -> crate::observer::ObservationFrameInput {
        crate::observer::ObservationFrameInput {
            display: Some(self.display),
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
        let ignored_top = (28.0 * f64::from(height) / bounds.height).round().max(1.0) as u32;
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
            provider_path,
            ocr: ocr_text,
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

