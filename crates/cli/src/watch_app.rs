use super::{
    mark_meaningful_change, record_cli_gate, trigger_label, CaptureDisposition, CaptureEnvironment,
    TargetStatus, WatchState,
};
use anyhow::{Context, Result};
use coosenpai_core::config::{Config, WatchAppConfig};
use coosenpai_core::debug::DebugStore;
use coosenpai_core::image_processing::{png_dimensions, process_png};
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::ports::ActivitySnapshot;
use coosenpai_core::state::{ActivityTriggerKind, PendingFrameContext};
use coosenpai_core::watch_coordinator::{
    application_capture_is_needed, application_is_foreground, normalize_ocr_blocks,
    ApplicationTriggerCoordinator, StagnationFingerprint,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

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

    fn sync(&mut self, config: &Config, activity: Option<&ActivitySnapshot>, now: Instant) {
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
        config: &Config,
        environment: &CaptureEnvironment<'_>,
        ocr_available: bool,
        activity: Option<&ActivitySnapshot>,
        max_interval_ms: u64,
        front_app: Option<String>,
        watch: &mut WatchState,
    ) -> Result<()> {
        self.sync(config, activity, Instant::now());
        watch.target_statuses = config
            .watch
            .apps
            .iter()
            .map(|application| target_status(application, activity, watch))
            .collect();
        for application in config.watch.apps.iter().filter(|target| target.enabled) {
            let foreground =
                application_is_foreground(activity, &application.bundle_id, &application.name);
            if !application_capture_is_needed(config.watch.fullscreen, foreground) {
                continue;
            }
            let Some(target) = self.targets.get_mut(&application.bundle_id) else {
                continue;
            };
            let trigger = target.coordinator.evaluate(
                config,
                &application.bundle_id,
                &application.name,
                activity,
                target.last_capture.elapsed().as_millis() as u64,
                max_interval_ms,
            );
            let Some(trigger) = trigger else { continue };
            if (target.last_capture.elapsed().as_millis() as u64)
                < config.watch.triggers.min_spacing_ms
            {
                continue;
            }
            let disposition = capture_application(
                config,
                environment,
                ocr_available,
                application,
                front_app.clone(),
                trigger,
                target,
                watch,
            )
            .await?;
            if let Some(status) = watch
                .target_statuses
                .iter_mut()
                .find(|status| status.name == application.name)
            {
                status.trigger = Some(trigger_label(trigger));
                if disposition != CaptureDisposition::Suppressed {
                    status.last_captured_at = Some(environment.clock.now());
                }
            }
        }
        Ok(())
    }
}

fn target_status(
    application: &WatchAppConfig,
    activity: Option<&ActivitySnapshot>,
    watch: &WatchState,
) -> TargetStatus {
    let previous = watch
        .target_statuses
        .iter()
        .find(|status| status.name == application.name);
    TargetStatus {
        name: application.name.clone(),
        enabled: application.enabled,
        foreground: application_is_foreground(activity, &application.bundle_id, &application.name),
        last_captured_at: previous.and_then(|status| status.last_captured_at),
        trigger: previous.and_then(|status| status.trigger),
    }
}

#[allow(clippy::too_many_arguments)]
async fn capture_application(
    config: &Config,
    environment: &CaptureEnvironment<'_>,
    ocr_available: bool,
    application: &WatchAppConfig,
    front_app: Option<String>,
    trigger: ActivityTriggerKind,
    target: &mut ApplicationWatchState,
    watch: &mut WatchState,
) -> Result<CaptureDisposition> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("application.png");
    let Some(captured) = environment
        .application_capture
        .capture_application(
            &application.bundle_id,
            &source,
            environment.cancellation.clone(),
        )
        .await?
    else {
        target.last_capture = Instant::now();
        return Ok(CaptureDisposition::Suppressed);
    };
    let captured_at = environment.clock.now();
    let bytes = tokio::fs::read(captured.path).await?;
    let (width, height) = png_dimensions(&bytes).context("アプリの画面 PNG が不正です")?;
    let processed = process_png(
        bytes,
        config.watch.downscale_width,
        0,
        Vec::new(),
        environment.semaphore.clone(),
    )
    .await?;
    let provider_path = directory.path().join("provider.png");
    tokio::fs::write(&provider_path, &processed.provider_png).await?;
    let ocr = if config.watch.ocr_gate.enabled && ocr_available {
        let ocr_path = directory.path().join("ocr.png");
        tokio::fs::write(&ocr_path, &processed.masked_png).await?;
        environment
            .ocr_port
            .recognize(
                &ocr_path,
                &config.watch.ocr_gate.level,
                Duration::from_millis(config.watch.ocr_gate.timeout_ms),
                environment.cancellation.clone(),
            )
            .await
            .ok()
            .map(|blocks| normalize_ocr_blocks(&blocks, width, height, &[], 0))
    } else {
        None
    };
    let changed_by_ocr = matches!((&ocr, &target.last_ocr), (Some(_), Some(_)));
    let changed = match (&ocr, &target.last_ocr) {
        (Some(current), Some(previous)) => current.signature != *previous,
        _ => target.last_hash.as_deref() != Some(processed.comparison_hash.as_str()),
    };
    let context_id = DebugStore::new_id();
    let debug_id = config.debug.enabled.then(|| context_id.clone());
    if let Some(id) = &debug_id {
        DebugStore::from_paths(environment.paths).record_frame(
            id,
            captured_at,
            &processed.provider_png,
            ocr.as_ref().map(|value| value.text.as_str()),
        )?;
    }
    target.last_capture = Instant::now();
    if !changed {
        record_cli_gate(
            environment.paths,
            config,
            trigger,
            debug_id.as_deref(),
            ocr.as_ref().map(|value| value.text.as_str()),
            false,
            if changed_by_ocr {
                "OCR 一致"
            } else {
                "画素一致"
            },
        )?;
        return Ok(CaptureDisposition::Unchanged);
    }
    target.last_hash = Some(processed.comparison_hash);
    target.last_ocr = ocr.as_ref().map(|value| value.signature.clone());
    let frame_target = format!("app:{}", application.bundle_id);
    mark_meaningful_change(
        watch,
        &frame_target,
        target.last_hash.clone().unwrap_or_default(),
        target.last_ocr.clone(),
        captured_at,
    )?;
    let ocr_text = ocr.map(|value| value.text);
    environment
        .runtime
        .register_pending_frame_context(PendingFrameContext::bounded(
            context_id.clone(),
            captured_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            trigger,
            front_app.clone(),
            Some(application.name.clone()),
            frame_target.clone(),
            ocr_text.clone(),
        ))?;
    watch.pending_frames.push(ObservationFrameInput {
        scope_generation: environment.runtime.watch_scope_generation(),
        context_id,
        captured_at,
        debug_id: debug_id.clone(),
        relative_seconds: target
            .last_capture
            .duration_since(watch.window_start)
            .as_secs_f64(),
        trigger,
        front_app,
        app: Some(application.name.clone()),
        target: frame_target,
        ocr_text,
        image_path: provider_path,
    });
    watch.last_accepted = Some(target.last_capture);
    watch.last_captured_at = Some(captured_at);
    watch.temporary_directories.push(directory);
    record_cli_gate(
        environment.paths,
        config,
        trigger,
        debug_id.as_deref(),
        watch
            .pending_frames
            .last()
            .and_then(|frame| frame.ocr_text.as_deref()),
        true,
        "送った",
    )?;
    Ok(CaptureDisposition::Accepted)
}
