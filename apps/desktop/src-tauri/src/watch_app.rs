use super::{
    capture_is_allowed, capture_trigger, ensure_capture_active, record_gate, trigger_name,
    CaptureDisposition, WatchMemory,
};
use crate::state::DesktopState;
use crate::watch_presenter::WatchResult;
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
use coosenpai_core::ports::{ActivitySnapshot, ApplicationCapturePort, OcrPort, RuntimeLogger};
use coosenpai_core::state::{ActivityTriggerKind, PendingFrameContext};
use coosenpai_core::watch_coordinator::{
    application_capture_is_needed, application_is_foreground, ApplicationTriggerCoordinator,
    StagnationChange, StagnationFingerprint,
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
                    0,
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
            update_target_result(
                state,
                generation,
                application,
                trigger,
                result.0,
                result.1,
                result.2,
                result.3,
            )
            .await;
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
) -> Result<(CaptureDisposition, Option<String>, usize, Option<String>)> {
    state
        .publish_watch_view(generation, WatchResult::CaptureStarted)
        .await;
    let scope_generation = state.core_runtime().watch_scope_generation();
    ensure_capture_active(&cancellation)?;
    let directory = tempfile::tempdir()?;
    let source_directory = directory.path().join("captures");
    let Some(_screen_gate) =
        crate::screen_capture_gate::acquire_screen_capture_gate(state, &cancellation).await
    else {
        return Err(anyhow::anyhow!("見守りの撮影が取り消されました"));
    };
    let captured = capture
        .capture_application(
            &application.bundle_id,
            &source_directory,
            config.watch.app_window_limit,
            cancellation.clone(),
        )
        .await
        .map_err(anyhow::Error::new)?;
    ensure_capture_active(&cancellation)?;
    if captured.is_empty() {
        target.last_capture = Instant::now();
        return Ok((CaptureDisposition::WindowUnavailable, None, 0, None));
    }
    let captured_at = chrono::Utc::now();
    let prepared = prepare_application_frames(
        captured,
        &directory.path().join("processed"),
        config,
        ocr,
        ocr_enabled,
        semaphore.clone(),
        &cancellation,
    )
    .await?;
    let captured_at_instant = Instant::now();
    let comparison = compare_application_signatures(
        target.last_hash.as_deref(),
        target.last_ocr.as_deref(),
        &prepared.signatures,
    );
    let frame_count = prepared.frames.len();
    let frame_target = format!("app:{}", application.bundle_id);
    let mut frames = Vec::with_capacity(prepared.frames.len());
    let mut debug_frames = Vec::new();
    for prepared_frame in prepared.frames {
        let frame = prepared_frame.observation_frame(
            scope_generation,
            captured_at,
            captured_at_instant
                .duration_since(memory.window_start)
                .as_secs_f64(),
            trigger,
            memory.front_app.clone(),
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
    ensure_capture_active(&cancellation)?;
    let debug_store = DebugStore::from_paths(&state.paths);
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
            true,
            captured_at,
            debug_frames,
        )?;
        let directories = debug_batch.directories();
        let mut publication_started = false;
        let publication =
            memory
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
            let _ = state
                .logger
                .write("WARN", &format!("debug 記録の整理に失敗しました: {error}"));
        }
        target.last_capture = captured_at_instant;
        return Ok((
            CaptureDisposition::Unchanged,
            Some(captured_at.to_rfc3339()),
            0,
            None,
        ));
    }
    let target_enabled = coosenpai_core::watch_coordinator::frame_target_is_enabled(
        &state.runtime_config(),
        &frame_target,
    );
    if !target_enabled {
        let comparison_hash = prepared.signatures.comparison_hash.clone();
        let ocr_signature = prepared.signatures.ocr_signature.clone();
        let prune_store = debug_store.clone();
        let mut debug_batch = record_application_debug_batch(
            &debug_store,
            &frames,
            trigger,
            false,
            "対象が無効です",
            true,
            captured_at,
            debug_frames,
        )?;
        let fingerprint_store = memory.stagnation_store.without_publication_gate();
        let mut fingerprint_change = match fingerprint_store.prepare_meaningful_change(
            &frame_target,
            coosenpai_core::watch_coordinator::StagnationFingerprint {
                image_hash: comparison_hash.clone(),
                ocr_signature: ocr_signature.clone(),
            },
            captured_at,
        ) {
            Ok(change) => change,
            Err(error) => {
                return Err(rollback_application_publication(
                    anyhow::Error::from(error),
                    &mut debug_batch,
                    None,
                    None,
                ));
            }
        };
        let mut directories = debug_batch.directories();
        if let Some(change) = fingerprint_change.as_ref() {
            directories.extend(change.directories());
        }
        let mut publication_started = false;
        let publication =
            memory
                .publication
                .publish_batch_with_directories(directories, |locks| -> Result<()> {
                    publication_started = true;
                    let result = (|| {
                        debug_batch.publish(locks)?;
                        if let Some(change) = fingerprint_change.as_mut() {
                            change.publish(locks)?;
                        }
                        debug_batch.validate_commit()?;
                        if let Some(change) = fingerprint_change.as_ref() {
                            change.validate_commit()?;
                        }
                        debug_batch.commit()?;
                        if let Some(change) = fingerprint_change.as_mut() {
                            change.commit()?;
                            memory.stagnation.mark_meaningful_change(Instant::now());
                            memory.pending_stagnation_report = None;
                            memory.last_meaningful_change_at = captured_at;
                        }
                        target.last_hash = Some(comparison_hash.clone());
                        target.last_ocr = ocr_signature.clone();
                        target.last_capture = captured_at_instant;
                        Ok::<(), anyhow::Error>(())
                    })();
                    match result {
                        Ok(()) => Ok(()),
                        Err(error) => Err(rollback_application_publication_with_directories(
                            error,
                            &mut debug_batch,
                            fingerprint_change.as_mut(),
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
                fingerprint_change.as_mut(),
                None,
            ));
        }
        if let Err(error) = prune_store.prune(captured_at) {
            let _ = state
                .logger
                .write("WARN", &format!("debug 記録の整理に失敗しました: {error}"));
        }
        return Ok((CaptureDisposition::Suppressed, None, 0, None));
    }
    ensure_capture_active(&cancellation)?;
    let contexts = frames
        .iter()
        .map(|frame| {
            PendingFrameContext::bounded(
                frame.context_id.clone(),
                captured_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                trigger,
                memory.front_app.clone(),
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
        true,
        captured_at,
        debug_frames,
    )?;
    let fingerprint_store = memory.stagnation_store.without_publication_gate();
    let mut fingerprint_change = match fingerprint_store.prepare_meaningful_change(
        &frame_target,
        coosenpai_core::watch_coordinator::StagnationFingerprint {
            image_hash: comparison_hash.clone(),
            ocr_signature: ocr_signature.clone(),
        },
        captured_at,
    ) {
        Ok(change) => change,
        Err(error) => {
            return Err(rollback_application_publication(
                anyhow::Error::from(error),
                &mut debug_batch,
                None,
                None,
            ));
        }
    };
    let mut pending_change = match state
        .core_runtime()
        .prepare_pending_frame_contexts(contexts)
    {
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
        memory
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
                        memory.stagnation.mark_meaningful_change(Instant::now());
                        memory.pending_stagnation_report = None;
                        memory.last_meaningful_change_at = captured_at;
                    }
                    if let Some(change) = pending_change.as_mut() {
                        change.commit()?;
                    }
                    target.last_hash = Some(comparison_hash.clone());
                    target.last_ocr = ocr_signature.clone();
                    target.last_capture = captured_at_instant;
                    memory.frames.extend(frames);
                    memory.last_accepted = Some(target.last_capture);
                    memory.directories.push(directory);
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
        let _ = state
            .logger
            .write("WARN", &format!("debug 記録の整理に失敗しました: {error}"));
    }
    let _ = state.logger.write(
        "INFO",
        &format!(
            "撮影判断: target=app:{} trigger={} result=撮影",
            application.bundle_id,
            trigger_name(trigger)
        ),
    );
    let next_send_at =
        chrono::Utc::now() + chrono::Duration::milliseconds(config.watch.send_debounce_ms as i64);
    Ok((
        CaptureDisposition::Accepted,
        Some(captured_at.to_rfc3339()),
        frame_count,
        Some(next_send_at.to_rfc3339()),
    ))
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
    include_gates: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    debug_frames: Vec<DebugFrameRecord>,
) -> Result<DebugCaptureBatch> {
    let gates = include_gates
        .then(|| {
            frames
                .iter()
                .filter_map(|frame| frame.debug_id.as_ref().map(|id| (id, frame)))
                .map(|(id, frame)| DebugGateRecord {
                    id: id.clone(),
                    created_at: created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    trigger: trigger_name(trigger).to_owned(),
                    sent,
                    reason: reason.to_owned(),
                    image_file: Some(format!("frame-{id}.png")),
                    ocr_preview: ocr_preview(frame.ocr_text.as_deref()),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    store
        .prepare_capture_batch(debug_frames, gates)
        .map_err(anyhow::Error::from)
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
    frame_count: usize,
    next_send_at: Option<String>,
) {
    state
        .publish_watch_view(
            generation,
            WatchResult::TargetCaptured {
                bundle_id: application.bundle_id.clone(),
                trigger,
                disposition,
                captured_at,
                frame_count,
                next_send_at,
            },
        )
        .await;
}
