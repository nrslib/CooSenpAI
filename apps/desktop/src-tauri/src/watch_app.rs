use super::{
    capture_is_allowed, capture_trigger, mark_meaningful_change, record_gate, trigger_name,
    CaptureDisposition, WatchMemory,
};
use crate::snapshot::ObserverViewPhase;
use crate::state::DesktopState;
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
        cancellation: CancellationToken,
    ) -> Result<()> {
        let now = Instant::now();
        self.sync(config, activity, now);
        for application in &config.watch.apps {
            let foreground =
                application_is_foreground(activity, &application.bundle_id, &application.name);
            update_foreground(state, application, foreground).await;
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
                record_gate(state, config, trigger, None, None, false, "最低間隔")?;
                update_target_result(
                    state,
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
                cancellation.clone(),
            )
            .await?;
            update_target_result(state, application, trigger, result.0, result.1).await;
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
    cancellation: CancellationToken,
) -> Result<(CaptureDisposition, Option<String>)> {
    state
        .publish(|snapshot| snapshot.observer.phase = ObserverViewPhase::Capturing)
        .await;
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("application.png");
    let Some(captured) = capture
        .capture_application(&application.bundle_id, &source, cancellation.clone())
        .await
        .map_err(anyhow::Error::new)?
    else {
        target.last_capture = Instant::now();
        return Ok((CaptureDisposition::WindowUnavailable, None));
    };
    let captured_at = chrono::Utc::now();
    let bytes = tokio::fs::read(&captured.path).await?;
    let (width, height) = png_dimensions(&bytes).context("アプリの画面 PNG が不正です")?;
    let processed = process_png(
        bytes,
        config.watch.downscale_width,
        0,
        Vec::new(),
        semaphore.clone(),
    )
    .await?;
    let provider_path = directory.path().join("provider.png");
    tokio::fs::write(&provider_path, &processed.provider_png).await?;
    let ocr_text = if ocr_enabled {
        let ocr_path = directory.path().join("ocr.png");
        tokio::fs::write(&ocr_path, &processed.masked_png).await?;
        ocr.recognize(
            &ocr_path,
            &config.watch.ocr_gate.level,
            Duration::from_millis(config.watch.ocr_gate.timeout_ms),
            cancellation,
        )
        .await
        .ok()
        .map(|blocks| normalize_ocr_blocks(&blocks, width, height, &[], 0))
    } else {
        None
    };
    let changed_by_ocr = matches!((&ocr_text, &target.last_ocr), (Some(_), Some(_)));
    let changed = match (&ocr_text, &target.last_ocr) {
        (Some(current), Some(previous)) => current.signature != *previous,
        _ => target.last_hash.as_deref() != Some(processed.comparison_hash.as_str()),
    };
    let context_id = DebugStore::new_id();
    let debug_id = config.debug.enabled.then(|| context_id.clone());
    if let Some(id) = &debug_id {
        DebugStore::from_paths(&state.paths).record_frame(
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
    state
        .core_runtime()
        .register_pending_frame_context(PendingFrameContext::bounded(
            context_id.clone(),
            captured_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            trigger,
            memory.front_app.clone(),
            Some(application.name.clone()),
            frame_target.clone(),
            ocr_text.clone(),
        ))?;
    memory.frames.push(ObservationFrameInput {
        scope_generation: state.core_runtime().watch_scope_generation(),
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

async fn update_foreground(state: &DesktopState, application: &WatchAppConfig, value: bool) {
    state
        .publish(|snapshot| {
            if let Some(target) = snapshot
                .observer
                .targets
                .iter_mut()
                .find(|target| target.target == format!("app:{}", application.bundle_id))
            {
                target.foreground = value;
            }
        })
        .await;
}

async fn update_target_result(
    state: &Arc<DesktopState>,
    application: &WatchAppConfig,
    trigger: ActivityTriggerKind,
    disposition: CaptureDisposition,
    captured_at: Option<String>,
) {
    state
        .publish(|snapshot| {
            snapshot.observer.phase = ObserverViewPhase::Idle;
            snapshot.observer.last_trigger = Some(trigger_name(trigger).to_owned());
            snapshot.observer.last_capture_disposition = Some(disposition.display().to_owned());
            snapshot.observer.pending_frame_count = snapshot
                .observer
                .pending_frame_count
                .saturating_add(usize::from(disposition == CaptureDisposition::Accepted));
            if let Some(target) = snapshot
                .observer
                .targets
                .iter_mut()
                .find(|target| target.target == format!("app:{}", application.bundle_id))
            {
                target.last_trigger = Some(trigger_name(trigger).to_owned());
                if captured_at.is_some() {
                    target.last_captured_at = captured_at;
                }
            }
        })
        .await;
}
