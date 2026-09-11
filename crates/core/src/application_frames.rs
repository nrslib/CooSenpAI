use crate::config::Config;
use crate::image_processing::{png_dimensions, process_png, ProcessedImage};
use crate::ports::{ApplicationCapture, OcrPort, ScreenDisplay, WindowBounds};
use crate::watch_coordinator::{normalize_ocr_blocks, OcrText};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq)]
pub struct ApplicationFrameSignatureInput {
    pub window_id: u32,
    pub window_bounds: WindowBounds,
    pub display: ScreenDisplay,
    pub image_width: u32,
    pub image_height: u32,
    pub image_hash: String,
    pub ocr_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationSignatures {
    pub comparison_hash: String,
    pub ocr_signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplicationSignatureComparison {
    pub changed: bool,
    pub changed_by_ocr: bool,
}

pub fn aggregate_application_signatures(
    frames: &[ApplicationFrameSignatureInput],
) -> ApplicationSignatures {
    let mut image_hash = Sha256::new();
    let mut ocr_hash = Sha256::new();
    let mut all_ocr_succeeded = true;
    for (index, frame) in frames.iter().enumerate() {
        let identity = format_application_frame_identity(index, frame);
        image_hash.update(identity.as_bytes());
        image_hash.update(frame.image_hash.as_bytes());
        if let Some(signature) = &frame.ocr_signature {
            ocr_hash.update(identity.as_bytes());
            ocr_hash.update(signature.as_bytes());
        } else {
            all_ocr_succeeded = false;
        }
    }
    ApplicationSignatures {
        comparison_hash: format!("{:x}", image_hash.finalize()),
        ocr_signature: all_ocr_succeeded.then(|| format!("{:x}", ocr_hash.finalize())),
    }
}

pub fn compare_application_signatures(
    previous_hash: Option<&str>,
    previous_ocr: Option<&str>,
    current: &ApplicationSignatures,
) -> ApplicationSignatureComparison {
    let changed_by_ocr = matches!(
        (previous_ocr, current.ocr_signature.as_deref()),
        (Some(_), Some(_))
    );
    let changed = match (previous_ocr, current.ocr_signature.as_deref()) {
        (Some(previous), Some(current)) => previous != current,
        _ => previous_hash != Some(current.comparison_hash.as_str()),
    };
    ApplicationSignatureComparison {
        changed,
        changed_by_ocr,
    }
}

fn format_application_frame_identity(
    index: usize,
    frame: &ApplicationFrameSignatureInput,
) -> String {
    format!(
        "{index}:window={};size={}x{};",
        frame.window_id, frame.image_width, frame.image_height,
    )
}

pub struct PreparedApplicationFrame {
    pub context_id: String,
    pub window_id: u32,
    pub window_bounds: WindowBounds,
    pub display: ScreenDisplay,
    pub image: ProcessedImage,
    pub provider_path: PathBuf,
    pub ocr: Option<OcrText>,
}

impl PreparedApplicationFrame {
    #[allow(clippy::too_many_arguments)]
    pub fn observation_frame(
        &self,
        scope_generation: u64,
        captured_at: chrono::DateTime<chrono::Utc>,
        relative_seconds: f64,
        trigger: crate::state::ActivityTriggerKind,
        front_app: Option<String>,
        app: String,
        target: String,
        debug_enabled: bool,
    ) -> crate::observer::ObservationFrameInput {
        crate::observer::ObservationFrameInput {
            display: Some(self.display),
            window_id: Some(self.window_id),
            window_bounds: Some(self.window_bounds),
            scope_generation,
            context_id: self.context_id.clone(),
            captured_at,
            debug_id: debug_enabled.then(|| self.context_id.clone()),
            relative_seconds,
            trigger,
            front_app,
            app: Some(app),
            target,
            ocr_text: self.ocr.as_ref().map(|ocr| ocr.text.clone()),
            image_path: self.provider_path.clone(),
        }
    }
}

pub struct PreparedApplicationFrames {
    pub frames: Vec<PreparedApplicationFrame>,
    pub signatures: ApplicationSignatures,
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_application_frames(
    captures: Vec<ApplicationCapture>,
    directory: &Path,
    config: &Config,
    ocr: &dyn OcrPort,
    ocr_enabled: bool,
    semaphore: Arc<Semaphore>,
    cancellation: &CancellationToken,
) -> Result<PreparedApplicationFrames> {
    anyhow::ensure!(
        !captures.is_empty(),
        "撮影結果にアプリウィンドウがありません"
    );
    tokio::fs::create_dir_all(directory).await?;
    let mut frames = Vec::with_capacity(captures.len());
    let mut signatures = Vec::with_capacity(captures.len());
    for capture in captures {
        ensure_active(cancellation)?;
        anyhow::ensure!(
            !frames
                .iter()
                .any(|frame: &PreparedApplicationFrame| frame.window_id == capture.window_id),
            "撮影結果のウィンドウ ID が重複しています"
        );
        let bounds = capture.window_bounds;
        anyhow::ensure!(
            bounds.x.is_finite()
                && bounds.y.is_finite()
                && bounds.width.is_finite()
                && bounds.height.is_finite()
                && bounds.width > 0.0
                && bounds.height > 0.0,
            "アプリウィンドウの寸法が不正です"
        );
        let display_bounds = capture.display.bounds;
        anyhow::ensure!(
            display_bounds.x.is_finite()
                && display_bounds.y.is_finite()
                && display_bounds.width.is_finite()
                && display_bounds.height.is_finite()
                && display_bounds.width > 0.0
                && display_bounds.height > 0.0,
            "アプリウィンドウのディスプレイ寸法が不正です"
        );
        let bytes = tokio::fs::read(&capture.path).await?;
        let (width, height) = png_dimensions(&bytes).context("アプリの画面 PNG が不正です")?;
        let image = process_png(
            bytes,
            config.watch.downscale_width,
            0,
            Vec::new(),
            semaphore.clone(),
        )
        .await?;
        ensure_active(cancellation)?;
        let provider_path = directory.join(format!("provider-{}.png", capture.window_id));
        tokio::fs::write(&provider_path, &image.provider_png).await?;
        let ocr_text = if ocr_enabled {
            let ocr_path = directory.join(format!("ocr-{}.png", capture.window_id));
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
            .map(|blocks| normalize_ocr_blocks(&blocks, width, height, &[], 0))
        } else {
            None
        };
        ensure_active(cancellation)?;
        signatures.push(ApplicationFrameSignatureInput {
            window_id: capture.window_id,
            window_bounds: bounds,
            display: capture.display,
            image_width: width,
            image_height: height,
            image_hash: image.comparison_hash.clone(),
            ocr_signature: ocr_text.as_ref().map(|value| value.signature.clone()),
        });
        frames.push(PreparedApplicationFrame {
            context_id: crate::debug::DebugStore::new_id(),
            window_id: capture.window_id,
            window_bounds: bounds,
            display: capture.display,
            image,
            provider_path,
            ocr: ocr_text,
        });
    }
    Ok(PreparedApplicationFrames {
        frames,
        signatures: aggregate_application_signatures(&signatures),
    })
}

fn ensure_active(cancellation: &CancellationToken) -> Result<()> {
    anyhow::ensure!(
        !cancellation.is_cancelled(),
        "見守りの撮影が取り消されました"
    );
    Ok(())
}

