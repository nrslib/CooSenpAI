use super::{trigger_label, CaptureDisposition, CaptureEnvironment, TargetStatus, WatchState};
use anyhow::Result;
use coosenpai_core::application_frames::{
    compare_application_signatures, prepare_application_frames,
};
use coosenpai_core::companion_storage::PendingFrameContextChange;
use coosenpai_core::config::{Config, WatchAppConfig};
use coosenpai_core::debug::{
    ocr_preview, DebugCaptureBatch, DebugFrameRecord, DebugGateRecord, DebugStore,
};
use coosenpai_core::persistence::DirectoryLocks;
use coosenpai_core::ports::ActivitySnapshot;
use coosenpai_core::state::{ActivityTriggerKind, PendingFrameContext};
use coosenpai_core::watch_coordinator::{
    application_capture_is_needed, application_is_foreground, ApplicationTriggerCoordinator,
    StagnationChange, StagnationFingerprint,
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
    let source_directory = directory.path().join("captures");
    let captured = environment
        .application_capture
        .capture_application(
            &application.bundle_id,
            &source_directory,
            config.watch.app_window_limit,
            environment.cancellation.clone(),
        )
        .await?;
    if captured.is_empty() {
        target.last_capture = Instant::now();
        return Ok(CaptureDisposition::Suppressed);
    }
    let captured_at = environment.clock.now();
    let prepared = prepare_application_frames(
        captured,
        &directory.path().join("processed"),
        config,
        environment.ocr_port,
        config.watch.ocr_gate.enabled && ocr_available,
        environment.semaphore.clone(),
        environment.cancellation,
    )
    .await?;
    let captured_at_instant = Instant::now();
    let comparison = compare_application_signatures(
        target.last_hash.as_deref(),
        target.last_ocr.as_deref(),
        &prepared.signatures,
    );
    let frame_target = format!("app:{}", application.bundle_id);
    let mut frames = Vec::with_capacity(prepared.frames.len());
    let mut debug_frames = Vec::new();
    for prepared_frame in prepared.frames {
        let frame = prepared_frame.observation_frame(
            environment.runtime.watch_scope_generation(),
            captured_at,
            captured_at_instant
                .duration_since(watch.window_start)
                .as_secs_f64(),
            trigger,
            front_app.clone(),
            application.name.clone(),
            frame_target.clone(),
            config.debug.enabled,
        );
        if let Some(id) = &frame.debug_id {
            debug_frames.push(DebugFrameRecord {
                id: id.clone(),
                created_at: captured_at,
                provider_png: prepared_frame.image.provider_png.clone(),
                ocr_text: frame.ocr_text.clone(),
            });
        }
        frames.push(frame);
    }
    let debug_store = DebugStore::from_paths(environment.paths);
    if !comparison.changed {
        let prune_store = debug_store.clone();
        let mut debug_batch = record_application_debug_batch(
            &debug_store,
            &frames,
            trigger,
            false,
            if comparison.changed_by_ocr {
                "OCR 一致"
            } else {
                "画素一致"
            },
            captured_at,
            debug_frames,
        )?;
        let directories = debug_batch.directories();
        let mut publication_started = false;
        let publication =
            watch
                .publication
                .publish_batch_with_directories(directories, |locks| -> Result<()> {
                    publication_started = true;
                    let result = (|| {
                        debug_batch.publish(locks)?;
                        debug_batch.commit()?;
                        Ok::<(), anyhow::Error>(())
                    })();
                    match result {
                        Ok(()) => Ok(()),
                        Err(error) => Err(rollback_application_publication_with_directories(
                            error,
                            &mut debug_batch,
                            None,
                            None,
                            locks,
                        )),
                    }
                });
        if let Err(error) = publication {
            if publication_started {
                return Err(error);
            }
            return Err(rollback_application_publication(
                error,
                &mut debug_batch,
                None,
                None,
            ));
        }
        if let Err(error) = prune_store.prune(captured_at) {
            eprintln!("debug 記録の整理に失敗しました: {error}");
        }
        target.last_capture = captured_at_instant;
        return Ok(CaptureDisposition::Unchanged);
    }
    let contexts = frames
        .iter()
        .map(|frame| {
            PendingFrameContext::bounded(
                frame.context_id.clone(),
                captured_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                trigger,
                front_app.clone(),
                Some(application.name.clone()),
                frame.target.clone(),
                frame.ocr_text.clone(),
            )
        })
        .collect::<Vec<_>>();
    let comparison_hash = prepared.signatures.comparison_hash.clone();
    let ocr_signature = prepared.signatures.ocr_signature.clone();
    let prune_store = debug_store.clone();
    let mut debug_batch = record_application_debug_batch(
        &debug_store,
        &frames,
        trigger,
        true,
        "送った",
        captured_at,
        debug_frames,
    )?;
    let fingerprint_store = watch.stagnation_store.without_publication_gate();
    let mut fingerprint_change = match fingerprint_store.prepare_meaningful_change(
        &frame_target,
        coosenpai_core::watch_coordinator::StagnationFingerprint {
            image_hash: comparison_hash.clone(),
            ocr_signature: ocr_signature.clone(),
        },
        captured_at,
    ) {
        Ok(Some(change)) => Some(change),
        Ok(None) => None,
        Err(error) => {
            return Err(rollback_application_publication(
                anyhow::Error::from(error),
                &mut debug_batch,
                None,
                None,
            ));
        }
    };
    let mut pending_change = match environment.runtime.prepare_pending_frame_contexts(contexts) {
        Ok(change) => change,
        Err(error) => {
            return Err(rollback_application_publication(
                anyhow::Error::from(error),
                &mut debug_batch,
                fingerprint_change.as_mut(),
                None,
            ));
        }
    };
    let mut directories = debug_batch.directories();
    if let Some(change) = fingerprint_change.as_ref() {
        directories.extend(change.directories());
    }
    if let Some(change) = pending_change.as_ref() {
        directories.extend(change.directories());
    }
    let mut publication_started = false;
    let publication =
        watch
            .publication
            .publish_batch_with_directories(directories, |locks| -> Result<()> {
                publication_started = true;
                let result = (|| {
                    debug_batch.publish(locks)?;
                    if let Some(change) = fingerprint_change.as_mut() {
                        change.publish(locks)?;
                    }
                    if let Some(change) = pending_change.as_mut() {
                        change.publish(locks)?;
                    }
                    debug_batch.validate_commit()?;
                    if let Some(change) = fingerprint_change.as_ref() {
                        change.validate_commit()?;
                    }
                    if let Some(change) = pending_change.as_ref() {
                        change.validate_commit()?;
                    }
                    debug_batch.commit()?;
                    if let Some(change) = fingerprint_change.as_mut() {
                        change.commit()?;
                        watch.stagnation.mark_meaningful_change(Instant::now());
                        watch.pending_stagnation_report = None;
                        watch.last_meaningful_change_at = captured_at;
                    }
                    if let Some(change) = pending_change.as_mut() {
                        change.commit()?;
                    }
                    target.last_hash = Some(comparison_hash.clone());
                    target.last_ocr = ocr_signature.clone();
                    target.last_capture = captured_at_instant;
                    watch.pending_frames.extend(frames);
                    watch.last_accepted = Some(target.last_capture);
                    watch.last_captured_at = Some(captured_at);
                    watch.temporary_directories.push(directory);
                    Ok::<(), anyhow::Error>(())
                })();
                match result {
                    Ok(()) => Ok(()),
                    Err(error) => Err(rollback_application_publication_with_directories(
                        error,
                        &mut debug_batch,
                        fingerprint_change.as_mut(),
                        pending_change.as_mut(),
                        locks,
                    )),
                }
            });
    if let Err(error) = publication {
        if publication_started {
            return Err(error);
        }
        return Err(rollback_application_publication(
            error,
            &mut debug_batch,
            fingerprint_change.as_mut(),
            pending_change.as_mut(),
        ));
    }
    if let Err(error) = prune_store.prune(captured_at) {
        eprintln!("debug 記録の整理に失敗しました: {error}");
    }
    Ok(CaptureDisposition::Accepted)
}

fn rollback_application_publication_with_directories(
    error: anyhow::Error,
    debug_batch: &mut DebugCaptureBatch,
    fingerprint_change: Option<&mut StagnationChange>,
    pending_change: Option<&mut PendingFrameContextChange>,
    directories: &DirectoryLocks,
) -> anyhow::Error {
    let mut rollback_errors = Vec::new();
    if let Some(change) = fingerprint_change {
        if let Err(error) = change.rollback_with_directories(directories) {
            rollback_errors.push(format!("fingerprint={error}"));
        }
    }
    if let Some(change) = pending_change {
        if let Err(error) = change.rollback_with_directories(directories) {
            rollback_errors.push(format!("pending={error}"));
        }
    }
    if let Err(error) = debug_batch.rollback_with_directories(directories) {
        rollback_errors.push(format!("debug={error}"));
    }
    if rollback_errors.is_empty() {
        error
    } else {
        eprintln!(
            "アプリ撮影セットのロールバックに失敗しました: {}",
            rollback_errors.join(", ")
        );
        error.context(format!(
            "アプリ撮影セットのロールバックに失敗しました: {}",
            rollback_errors.join(", ")
        ))
    }
}

fn rollback_application_publication(
    error: anyhow::Error,
    debug_batch: &mut DebugCaptureBatch,
    fingerprint_change: Option<&mut StagnationChange>,
    pending_change: Option<&mut PendingFrameContextChange>,
) -> anyhow::Error {
    let mut rollback_errors = Vec::new();
    if let Some(change) = fingerprint_change {
        if let Err(error) = change.rollback() {
            rollback_errors.push(format!("fingerprint={error}"));
        }
    }
    if let Some(change) = pending_change {
        if let Err(error) = change.rollback() {
            rollback_errors.push(format!("pending={error}"));
        }
    }
    if let Err(error) = debug_batch.rollback() {
        rollback_errors.push(format!("debug={error}"));
    }
    if rollback_errors.is_empty() {
        error
    } else {
        eprintln!(
            "アプリ撮影セットのロールバックに失敗しました: {}",
            rollback_errors.join(", ")
        );
        error.context(format!(
            "アプリ撮影セットのロールバックに失敗しました: {}",
            rollback_errors.join(", ")
        ))
    }
}

fn record_application_debug_batch(
    store: &DebugStore,
    frames: &[coosenpai_core::observer::ObservationFrameInput],
    trigger: ActivityTriggerKind,
    sent: bool,
    reason: &str,
    created_at: chrono::DateTime<chrono::Utc>,
    debug_frames: Vec<DebugFrameRecord>,
) -> Result<DebugCaptureBatch> {
    let gates = frames
        .iter()
        .filter_map(|frame| frame.debug_id.as_ref().map(|id| (id, frame)))
        .map(|(id, frame)| DebugGateRecord {
            id: id.clone(),
            created_at: created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            trigger: trigger_label(trigger).to_owned(),
            sent,
            reason: reason.to_owned(),
            image_file: Some(format!("frame-{id}.png")),
            ocr_preview: ocr_preview(frame.ocr_text.as_deref()),
        })
        .collect::<Vec<_>>();
    store
        .prepare_capture_batch(debug_frames, gates)
        .map_err(anyhow::Error::from)
}
