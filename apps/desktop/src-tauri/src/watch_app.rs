use super::{
    capture_is_allowed, capture_trigger, ensure_capture_active, mark_meaningful_change,
    record_gate, trigger_name, CaptureDisposition, WatchMemory,
};
use crate::state::DesktopState;
use crate::watch_presenter::WatchResult;
use anyhow::{Context, Result};
use coosenpai_core::config::{Config, WatchAppConfig};
use coosenpai_core::debug::DebugStore;
use coosenpai_core::image_processing::{png_dimensions, process_png};
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::ports::{ActivitySnapshot, ApplicationCapturePort, OcrPort, RuntimeLogger};
use coosenpai_core::state::{ActivityTriggerKind, PendingFrameContext};
use coosenpai_core::watch_coordinator::{
    application_capture_is_needed, application_is_foreground, normalize_ocr_blocks,
    ApplicationTriggerCoordinator, StagnationFingerprint,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub(super) struct ApplicationWatchSet {
    targets: HashMap<String, ApplicationWatchState>,
    initial_fingerprints: BTreeMap<String, StagnationFingerprint>,
}

struct ApplicationWatchState {
    coordinator: ApplicationTriggerCoordinator,
    last_hash: Option<String>,
    last_ocr: Option<String>,
    last_capture: Instant,
}

impl ApplicationWatchSet {
    pub(super) fn new(
        config: &Config,
        initial: Option<&ActivitySnapshot>,
        now: Instant,
        fingerprints: &BTreeMap<String, StagnationFingerprint>,
    ) -> Self {
        let mut value = Self {
            targets: HashMap::new(),
            initial_fingerprints: fingerprints.clone(),
        };
        value.sync(config, initial, now);
        value
    }

    pub(super) fn sync(
        &mut self,
        config: &Config,
        activity: Option<&ActivitySnapshot>,
        now: Instant,
    ) {
        let configured = config
            .watch
            .apps
            .iter()
            .map(|application| application.bundle_id.as_str())
            .collect::<HashSet<_>>();
        self.targets
            .retain(|bundle_id, _| configured.contains(bundle_id.as_str()));
        for application in &config.watch.apps {
            self.targets
                .entry(application.bundle_id.clone())
                .or_insert_with(|| {
                    let fingerprint = self
                        .initial_fingerprints
                        .get(&format!("app:{}", application.bundle_id));
                    ApplicationWatchState {
                        coordinator: ApplicationTriggerCoordinator::new(
                            config,
                            &application.bundle_id,
                            &application.name,
                            activity,
                        ),
                        last_hash: fingerprint.map(|value| value.image_hash.clone()),
                        last_ocr: fingerprint.and_then(|value| value.ocr_signature.clone()),
                        last_capture: now
                            .checked_sub(Duration::from_millis(
                                config.watch.triggers.max_interval_ms,
                            ))
                            .unwrap_or(now),
                    }
                });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn poll(
        &mut self,
        state: &Arc<DesktopState>,
        config: &Config,
        activity: Option<&ActivitySnapshot>,
        max_interval_ms: u64,
        immediate_capture: bool,
        ocr_enabled: bool,
        capture: &dyn ApplicationCapturePort,
        ocr: &dyn OcrPort,
        semaphore: &Arc<Semaphore>,
        memory: &mut WatchMemory,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let now = Instant::now();
        self.sync(config, activity, now);
        for application in &config.watch.apps {
            let foreground =
                application_is_foreground(activity, &application.bundle_id, &application.name);
            update_foreground(state, generation, application, foreground).await;
            if !application.enabled {
                continue;
            }
            if !application_capture_is_needed(config.watch.fullscreen, foreground) {
                continue;
            }
            let Some(target) = self.targets.get_mut(&application.bundle_id) else {
                continue;
            };
            let trigger = capture_trigger(
                immediate_capture,
                target.coordinator.evaluate(
                    config,
                    &application.bundle_id,
                    &application.name,
                    activity,
                    target.last_capture.elapsed().as_millis() as u64,
                    max_interval_ms,
                ),
            );
            let Some(trigger) = trigger else { continue };
            if !capture_is_allowed(
                immediate_capture,
                target.last_capture.elapsed().as_millis() as u64,
                config.watch.triggers.min_spacing_ms,
            ) {
                record_gate(
                    state,
                    config,
                    trigger,
                    None,
                    None,
                    false,
                    "最低間隔",
                    &memory.publication,
                )?;
                update_target_result(
                    state,
                    generation,
                    application,
                    trigger,
                    CaptureDisposition::MinSpacing,
                    None,
                )
                .await;
                continue;
            }
            let result = capture_application(
                state,
                config,
                application,
                trigger,
                ocr_enabled,
                capture,
                ocr,
                semaphore,
                target,
                memory,
                generation,
                cancellation.clone(),
            )
            .await?;
            update_target_result(state, generation, application, trigger, result.0, result.1).await;
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn capture_application(
    state: &Arc<DesktopState>,
    config: &Config,
    application: &WatchAppConfig,
    trigger: ActivityTriggerKind,
    ocr_enabled: bool,
    capture: &dyn ApplicationCapturePort,
    ocr: &dyn OcrPort,
    semaphore: &Arc<Semaphore>,
    target: &mut ApplicationWatchState,
    memory: &mut WatchMemory,
    generation: u64,
    cancellation: CancellationToken,
) -> Result<(CaptureDisposition, Option<String>)> {
    state
        .publish_watch_view(generation, WatchResult::CaptureStarted)
        .await;
    let scope_generation = state.core_runtime().watch_scope_generation();
    ensure_capture_active(&cancellation)?;
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("application.png");
    let Some(_screen_gate) =
        crate::screen_capture_gate::acquire_screen_capture_gate(state, &cancellation).await
    else {
        return Err(anyhow::anyhow!("見守りの撮影が取り消されました"));
    };
    let captured = capture
        .capture_application(&application.bundle_id, &source, cancellation.clone())
        .await
        .map_err(anyhow::Error::new)?;
    ensure_capture_active(&cancellation)?;
    let Some(captured) = captured else {
        target.last_capture = Instant::now();
        return Ok((CaptureDisposition::WindowUnavailable, None));
    };
    if cancellation.is_cancelled() {
        return Err(anyhow::anyhow!("見守りの撮影が取り消されました"));
    }
    let captured_at = chrono::Utc::now();
    let bytes = tokio::fs::read(&captured.path).await?;
    ensure_capture_active(&cancellation)?;
    let (width, height) = png_dimensions(&bytes).context("アプリの画面 PNG が不正です")?;
    let processed = process_png(
        bytes,
        config.watch.downscale_width,
        0,
        Vec::new(),
        semaphore.clone(),
    )
    .await?;
    ensure_capture_active(&cancellation)?;
    let provider_path = directory.path().join("provider.png");
    tokio::fs::write(&provider_path, &processed.provider_png).await?;
    ensure_capture_active(&cancellation)?;
    let ocr_text = if ocr_enabled {
        let ocr_path = directory.path().join("ocr.png");
        tokio::fs::write(&ocr_path, &processed.masked_png).await?;
        ensure_capture_active(&cancellation)?;
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
    ensure_capture_active(&cancellation)?;
    let changed_by_ocr = matches!((&ocr_text, &target.last_ocr), (Some(_), Some(_)));
    let changed = match (&ocr_text, &target.last_ocr) {
        (Some(current), Some(previous)) => current.signature != *previous,
        _ => target.last_hash.as_deref() != Some(processed.comparison_hash.as_str()),
    };
    let context_id = DebugStore::new_id();
    let debug_id = config.debug.enabled.then(|| context_id.clone());
    ensure_capture_active(&cancellation)?;
    if let Some(id) = &debug_id {
        DebugStore::from_paths(&state.paths)
            .with_publication_gate(memory.publication.clone())
            .record_frame(
                id,
                captured_at,
                &processed.provider_png,
                ocr_text.as_ref().map(|value| value.text.as_str()),
            )?;
    }
    target.last_capture = Instant::now();
    if !changed {
        record_gate(
            state,
            config,
            trigger,
            debug_id.as_deref(),
            ocr_text.as_ref().map(|value| value.text.as_str()),
            false,
            if changed_by_ocr {
                "OCR 一致"
            } else {
                "画素一致"
            },
            &memory.publication,
        )?;
        return Ok((
            CaptureDisposition::Unchanged,
            Some(captured_at.to_rfc3339()),
        ));
    }
    target.last_hash = Some(processed.comparison_hash);
    target.last_ocr = ocr_text.as_ref().map(|value| value.signature.clone());
    let frame_target = format!("app:{}", application.bundle_id);
    mark_meaningful_change(
        memory,
        &frame_target,
        target.last_hash.clone().unwrap_or_default(),
        target.last_ocr.clone(),
        captured_at,
    )?;
    if !coosenpai_core::watch_coordinator::frame_target_is_enabled(
        &state.runtime_config(),
        &frame_target,
    ) {
        return Ok((CaptureDisposition::Suppressed, None));
    }
    let ocr_text = ocr_text.map(|value| value.text);
    ensure_capture_active(&cancellation)?;
    state.core_runtime().register_pending_frame_context(
        PendingFrameContext::bounded(
            context_id.clone(),
            captured_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            trigger,
            memory.front_app.clone(),
            Some(application.name.clone()),
            frame_target.clone(),
            ocr_text.clone(),
        ),
        &memory.publication,
    )?;
    memory.frames.push(ObservationFrameInput {
        display: None,
        scope_generation,
        context_id,
        captured_at,
        debug_id: debug_id.clone(),
        relative_seconds: target
            .last_capture
            .duration_since(memory.window_start)
            .as_secs_f64(),
        trigger,
        front_app: memory.front_app.clone(),
        app: Some(application.name.clone()),
        target: frame_target,
        ocr_text,
        image_path: provider_path,
    });
    memory.last_accepted = Some(target.last_capture);
    memory.directories.push(directory);
    record_gate(
        state,
        config,
        trigger,
        debug_id.as_deref(),
        memory
            .frames
            .last()
            .and_then(|frame| frame.ocr_text.as_deref()),
        true,
        "送った",
        &memory.publication,
    )?;
    let _ = state.logger.write(
        "INFO",
        &format!(
            "撮影判断: target=app:{} trigger={} result=撮影",
            application.bundle_id,
            trigger_name(trigger)
        ),
    );
    Ok((CaptureDisposition::Accepted, Some(captured_at.to_rfc3339())))
}

async fn update_foreground(
    state: &DesktopState,
    generation: u64,
    application: &WatchAppConfig,
    value: bool,
) {
    state
        .publish_watch_view(
            generation,
            WatchResult::TargetForeground {
                bundle_id: application.bundle_id.clone(),
                foreground: value,
            },
        )
        .await;
}

async fn update_target_result(
    state: &Arc<DesktopState>,
    generation: u64,
    application: &WatchAppConfig,
    trigger: ActivityTriggerKind,
    disposition: CaptureDisposition,
    captured_at: Option<String>,
) {
    state
        .publish_watch_view(
            generation,
            WatchResult::TargetCaptured {
                bundle_id: application.bundle_id.clone(),
                trigger,
                disposition,
                captured_at,
            },
        )
        .await;
}
