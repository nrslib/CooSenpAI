use crate::platform;
use crate::snapshot::ObserverViewPhase;
use crate::state::DesktopState;
use anyhow::{Context, Result};
use coosenpai_core::config::Config;
use coosenpai_core::debug::{ocr_preview, DebugGateRecord, DebugStore};
use coosenpai_core::frame_buffer::FrameBuffer;
use coosenpai_core::image_processing::{own_window_exclusions, png_dimensions, process_png};
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::onboarding::TutorialStep;
use coosenpai_core::ports::{
    ActivityPort, HelperResolverPort, OcrPort, OwnWindowBounds, OwnWindowBoundsPort, PortError,
    RuntimeLogger, ScreenCapturePort,
};
use coosenpai_core::state::{ActivityTriggerKind, PendingFrameContext, StagnationObservation};
use coosenpai_core::watch_coordinator::{
    effective_max_interval_ms, evaluate_activity_poll, frame_target_is_enabled,
    is_self_application, normalize_ocr_blocks, retain_enabled_frames, watch_send_due,
    StagnationFingerprint, StagnationReportIntent, StagnationTracker, TriggerCoordinator,
    WatchStagnationStore,
};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[path = "watch_app.rs"]
mod watch_app;
use watch_app::ApplicationWatchSet;
#[path = "watch_heartbeat.rs"]
mod watch_heartbeat;
use watch_heartbeat::{heartbeat_if_due, mark_meaningful_change};
#[path = "watch_recovery.rs"]
mod watch_recovery;
use watch_recovery::{record_watch_failure, WatchRecovery, WatchRecoveryDecision};

pub struct WatchTask {
    cancellation: CancellationToken,
    task: tauri::async_runtime::JoinHandle<()>,
}

impl WatchTask {
    pub async fn stop(self) {
        self.cancellation.cancel();
        let _ = self.task.await;
    }

}

pub fn spawn(state: Arc<DesktopState>, generation: u64) -> Result<WatchTask> {
    let cancellation = state.cancellation.child_token();
    let frame_buffer = FrameBuffer::new(state.paths.frame_buffer.clone());
    let task_cancellation = cancellation.clone();
    let worker = tauri::async_runtime::spawn({
        let state = state.clone();
        async move { run(state, task_cancellation).await }
    });
    let task =
        tauri::async_runtime::spawn(supervise_watch_worker(worker, move |result| async move {
            let failed = result.is_err();
            if let Err(error) = result {
                let detail = watch_error_detail(&error);
                let _ = state.logger.write(
                    "ERROR",
                    &format!("見守りに失敗しました: error-type=watch error={detail}"),
                );
                state
                    .publish(|snapshot| {
                        snapshot.observer.record_error(detail);
                    })
                    .await;
            }
            state.watch_finished(generation, failed).await;
            let _ = frame_buffer.cleanup_expired(chrono::Utc::now());
        }));
    Ok(WatchTask { cancellation, task })
}

async fn await_watch_worker(worker: tauri::async_runtime::JoinHandle<Result<()>>) -> Result<()> {
    match worker.await {
        Ok(result) => result,
        Err(error) => Err(anyhow::anyhow!("見守りtaskがpanicしました: {error}")),
    }
}

async fn supervise_watch_worker<F, Fut>(
    worker: tauri::async_runtime::JoinHandle<Result<()>>,
    finished: F,
) where
    F: FnOnce(Result<()>) -> Fut,
    Fut: Future<Output = ()>,
{
    finished(await_watch_worker(worker).await).await;
}

pub(super) struct WatchMemory {
    frames: Vec<ObservationFrameInput>,
    directories: Vec<tempfile::TempDir>,
    last_hash: Option<String>,
    last_ocr: Option<String>,
    last_capture: Instant,
    last_observation: Instant,
    window_start: Instant,
    last_accepted: Option<Instant>,
    front_app: Option<String>,
    stagnation: StagnationTracker,
    stagnation_store: WatchStagnationStore,
    pending_stagnation_report: Option<StagnationReportIntent>,
    last_meaningful_change_at: chrono::DateTime<chrono::Utc>,
}

fn watch_error_detail(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

async fn run(state: Arc<DesktopState>, cancellation: CancellationToken) -> Result<()> {
    let screen_capture = platform::MacScreenCapture;
    let activity = platform::MacActivity;
    let initial_config = state.runtime_config();
    let mut helper = resolve_desktop_ocr_helper(&state, &initial_config);
    state.logger.write(
        "INFO",
        if helper.is_some() {
            "Vision OCR: subprocess-helper"
        } else {
            "Vision OCR: disabled-no-helper"
        },
    )?;
    let ocr = platform::MacOcr::new(helper.clone());
    let mut ocr_enabled = initial_config.watch.ocr_gate.enabled && helper.is_some();
    state
        .publish(|snapshot| snapshot.observer.ocr_gate_enabled = ocr_enabled)
        .await;
    let display = platform::read_display_geometry().await;
    let semaphore = Arc::new(Semaphore::new(2));
    let now = Instant::now();
    let now_utc = chrono::Utc::now();
    let initial = activity.read_activity().await.ok();
    let stagnation_store = WatchStagnationStore::new(state.paths.watch_stagnation.clone());
    let stagnation_snapshot = match stagnation_store.load(now_utc) {
        Ok(value) => value,
        Err(error) => {
            let _ = state.logger.write(
                "WARN",
                &format!("停滞状態を読み込めませんでした: error-type=persistence ({error})"),
            );
            coosenpai_core::watch_coordinator::StagnationSnapshot {
                last_meaningful_change_at: now_utc,
                reported: false,
                pending_report: None,
                fingerprints: Default::default(),
            }
        }
    };
    let mut trigger_coordinator =
        TriggerCoordinator::new(&state.runtime_config(), initial.as_ref());
    let mut application_watch = ApplicationWatchSet::new(
        &initial_config,
        initial.as_ref(),
        now,
        &stagnation_snapshot.fingerprints,
    );
    let application_capture = platform::MacApplicationCapture;
    let mut on_battery =
        state.runtime_config().watch.battery.enabled && platform::is_on_battery().await;
    let initial_interval = effective_max_interval_ms(&state.runtime_config(), on_battery);
    let mut memory = WatchMemory {
        frames: Vec::new(),
        directories: Vec::new(),
        last_hash: stagnation_snapshot
            .fingerprints
            .get("fullscreen")
            .map(|value| value.image_hash.clone()),
        last_ocr: stagnation_snapshot
            .fingerprints
            .get("fullscreen")
            .and_then(|value| value.ocr_signature.clone()),
        last_capture: now
            .checked_sub(Duration::from_millis(initial_interval))
            .unwrap_or(now),
        last_observation: now,
        window_start: now,
        last_accepted: None,
        front_app: initial.as_ref().and_then(|value| value.front_app.clone()),
        stagnation: StagnationTracker::resume(
            now,
            stagnation_snapshot.elapsed(now_utc),
            initial.as_ref(),
            stagnation_snapshot.reported,
        ),
        stagnation_store,
        pending_stagnation_report: stagnation_snapshot.pending_report,
        last_meaningful_change_at: stagnation_snapshot.last_meaningful_change_at,
    };
    let mut tutorial_initial_capture_pending =
        state.tutorial_current_step().await == Some(TutorialStep::Watch);
    let mut recovery = WatchRecovery::default();
    loop {
        let config = state.runtime_config();
        let next_helper = resolve_desktop_ocr_helper(&state, &config);
        if next_helper != helper {
            if let Err(error) = ocr.set_helper(next_helper.clone()) {
                if cancellation.is_cancelled() {
                    break;
                }
                let error = anyhow::Error::new(error);
                match record_watch_failure(&state, &mut recovery, &error, &cancellation).await {
                    WatchRecoveryDecision::Retry => {}
                    WatchRecoveryDecision::Stop => return Err(error),
                    WatchRecoveryDecision::Cancelled => break,
                    WatchRecoveryDecision::ConfigUpdateCancelled => {}
                }
                continue;
            }
            helper = next_helper;
        }
        let next_ocr_enabled = config.watch.ocr_gate.enabled && helper.is_some();
        if next_ocr_enabled != ocr_enabled {
            ocr_enabled = next_ocr_enabled;
            state
                .publish(|snapshot| snapshot.observer.ocr_gate_enabled = ocr_enabled)
                .await;
        }
        let poll_delay = if tutorial_initial_capture_pending {
            Duration::ZERO
        } else {
            Duration::from_millis(config.watch.triggers.poll_ms)
        };
        let iteration = tokio::select! {
            _ = cancellation.cancelled() => break,
            _ = tokio::time::sleep(poll_delay) => (async {
                if config.watch.battery.enabled {
                    on_battery = platform::is_on_battery().await;
                }
                let activity_snapshot = activity.read_activity().await.ok();
                let fresh_activity = memory.stagnation.observe_activity(
                    activity_snapshot.as_ref(),
                    config.watch.triggers.active_threshold_ms,
                );
                if fresh_activity && memory.stagnation.is_reported() {
                    let reacted_at = chrono::Utc::now();
                    match memory.stagnation_store.record_reaction(reacted_at) {
                        Ok(true) => {
                            memory.stagnation.mark_meaningful_change(Instant::now());
                            memory.pending_stagnation_report = None;
                            memory.last_meaningful_change_at = reacted_at;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            let _ = state.logger.write(
                                "WARN",
                                &format!("停滞エピソードの操作反応を保存できませんでした: error-type=persistence ({error})"),
                            );
                        }
                    }
                }
                let effective_interval = effective_max_interval_ms(&config, on_battery);
                let tutorial_initial_capture = tutorial_initial_capture_pending
                    && state.tutorial_current_step().await == Some(TutorialStep::Watch);
                let now = Instant::now();
                let decision = evaluate_activity_poll(
                    &mut trigger_coordinator,
                    &config,
                    activity_snapshot.as_ref(),
                    now,
                    memory.last_capture.elapsed().as_millis() as u64,
                    effective_interval,
                );
                memory.front_app = decision.front_app;
                let trigger = config
                    .watch
                    .fullscreen
                    .then(|| capture_trigger(tutorial_initial_capture, decision.trigger))
                    .flatten();
                if let Some(trigger) = trigger {
                    let elapsed = memory.last_capture.elapsed().as_millis() as u64;
                    if capture_is_allowed(
                        tutorial_initial_capture,
                        elapsed,
                        config.watch.triggers.min_spacing_ms,
                    ) {
                        let disposition = capture(&state, &config, ocr_enabled, &screen_capture, &ocr, display.as_ref(), &semaphore, &mut memory, trigger, cancellation.clone())
                            .await
                            .context("画面の観察準備")?;
                        record_capture_decision(&state, trigger, &memory.front_app, disposition).await;
                    } else {
                        record_gate(
                            &state,
                            &config,
                            trigger,
                            None,
                            None,
                            false,
                            "最低間隔",
                        )?;
                        record_capture_decision(&state, trigger, &memory.front_app, CaptureDisposition::MinSpacing).await;
                    }
                }
                application_watch
                    .poll(
                        &state,
                        &config,
                        activity_snapshot.as_ref(),
                        effective_interval,
                        tutorial_initial_capture,
                        ocr_enabled,
                        &application_capture,
                        &ocr,
                        &semaphore,
                        &mut memory,
                        cancellation.clone(),
                    )
                    .await
                    .context("アプリ観察")?;
                tutorial_initial_capture_pending = false;
                flush_if_due(&state, &config, &mut memory, cancellation.clone())
                    .await
                    .context("観察の送信と配達")?;
                heartbeat_if_due(
                    &state,
                    &config,
                    &mut memory,
                    effective_interval,
                    cancellation.clone(),
                )
                .await
                .context("見守り heartbeat")?;
                Ok(())
            }).await
        };
        if let Err(error) = iteration {
            if cancellation.is_cancelled() {
                break;
            }
            match record_watch_failure(&state, &mut recovery, &error, &cancellation).await {
                WatchRecoveryDecision::Retry => {}
                WatchRecoveryDecision::Stop => return Err(error),
                WatchRecoveryDecision::Cancelled => break,
                WatchRecoveryDecision::ConfigUpdateCancelled => {}
            }
        } else {
            recovery.reset();
            state
                .publish(|snapshot| snapshot.observer.clear_error())
                .await;
        }
    }
    Ok(())
}

fn capture_trigger(
    tutorial_initial_capture: bool,
    regular_trigger: Option<ActivityTriggerKind>,
) -> Option<ActivityTriggerKind> {
    if tutorial_initial_capture {
        Some(ActivityTriggerKind::Timer)
    } else {
        regular_trigger
    }
}

fn capture_is_allowed(immediate: bool, elapsed_ms: u64, min_spacing_ms: u64) -> bool {
    immediate || elapsed_ms >= min_spacing_ms
}

#[allow(clippy::too_many_arguments)]
async fn capture(
    state: &DesktopState,
    config: &Config,
    ocr_enabled: bool,
    screen_capture: &platform::MacScreenCapture,
    ocr: &platform::MacOcr,
    display: Option<&platform::DisplayGeometry>,
    semaphore: &Arc<Semaphore>,
    memory: &mut WatchMemory,
    trigger: ActivityTriggerKind,
    cancellation: CancellationToken,
) -> Result<CaptureDisposition> {
    let tutorial_watch = state.tutorial_current_step().await == Some(TutorialStep::Watch);
    if should_skip_self_application(memory.front_app.as_deref(), tutorial_watch) {
        memory.last_capture = Instant::now();
        record_gate(state, config, trigger, None, None, false, "自ウィンドウ")?;
        return Ok(CaptureDisposition::SelfApplication);
    }
    state
        .publish(|snapshot| snapshot.observer.phase = ObserverViewPhase::Capturing)
        .await;
    let captured_at = chrono::Utc::now();
    let Some(exclusions) =
        capture_exclusions(state.own_bounds.read_own_window_bounds().await, captured_at)
    else {
        state
            .publish(|snapshot| snapshot.observer.phase = ObserverViewPhase::Idle)
            .await;
        record_gate(state, config, trigger, None, None, false, "自ウィンドウ")?;
        return Ok(CaptureDisposition::OwnBoundsUnavailable);
    };
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("screen.png");
    let captured_path = match screen_capture.capture(&source, cancellation.clone()).await {
        Ok(path) => {
            state.record_screen_capture_result(true).await;
            path
        }
        Err(error @ PortError::ScreenCapturePermission(_)) => {
            state.record_screen_capture_result(false).await;
            return Err(anyhow::Error::new(error));
        }
        Err(error) => return Err(anyhow::Error::new(error)),
    };
    let bytes = tokio::fs::read(captured_path).await?;
    let (width, height) = png_dimensions(&bytes).context("画面 PNG が不正です")?;
    let ignored_top = platform::comparison_top_pixels(display, width, height);
    let processed = process_png(
        bytes,
        config.watch.downscale_width,
        ignored_top,
        exclusions.clone(),
        semaphore.clone(),
    )
    .await?;
    let provider_path = directory.path().join("provider.png");
    tokio::fs::write(&provider_path, &processed.provider_png).await?;
    let ocr_signature = if ocr_enabled {
        let ocr_path = directory.path().join("ocr.png");
        tokio::fs::write(&ocr_path, &processed.masked_png).await?;
        match ocr
            .recognize(
                &ocr_path,
                &config.watch.ocr_gate.level,
                Duration::from_millis(config.watch.ocr_gate.timeout_ms),
                cancellation,
            )
            .await
        {
            Ok(blocks) => Some(normalize_ocr_blocks(
                &blocks,
                width,
                height,
                &exclusions,
                ignored_top,
            )),
            Err(_) => {
                let _ = state
                    .logger
                    .write("WARN", "Vision OCR に失敗しました: error-type=ocr");
                None
            }
        }
    } else {
        None
    };
    let changed_by_ocr = matches!((&ocr_signature, &memory.last_ocr), (Some(_), Some(_)));
    let changed = match (&ocr_signature, &memory.last_ocr) {
        (Some(current), Some(previous)) => current.signature != *previous,
        _ => memory.last_hash.as_deref() != Some(processed.comparison_hash.as_str()),
    };
    let context_id = DebugStore::new_id();
    let debug_id = config.debug.enabled.then(|| context_id.clone());
    if let Some(id) = &debug_id {
        DebugStore::from_paths(&state.paths).record_frame(
            id,
            captured_at,
            &processed.provider_png,
            ocr_signature.as_ref().map(|value| value.text.as_str()),
        )?;
    }
    memory.last_capture = Instant::now();
    if !changed {
        record_gate(
            state,
            config,
            trigger,
            debug_id.as_deref(),
            ocr_signature.as_ref().map(|value| value.text.as_str()),
            false,
            if changed_by_ocr {
                "OCR 一致"
            } else {
                "画素一致"
            },
        )?;
        state
            .publish(|snapshot| snapshot.observer.phase = ObserverViewPhase::Idle)
            .await;
        return Ok(CaptureDisposition::Unchanged);
    }
    memory.last_hash = Some(processed.comparison_hash);
    memory.last_ocr = ocr_signature.as_ref().map(|value| value.signature.clone());
    let stagnation_hash = memory.last_hash.clone().unwrap_or_default();
    let stagnation_ocr = memory.last_ocr.clone();
    mark_meaningful_change(
        memory,
        "fullscreen",
        stagnation_hash,
        stagnation_ocr,
        captured_at,
    )?;
    if !frame_target_is_enabled(&state.runtime_config(), "fullscreen") {
        return Ok(CaptureDisposition::Suppressed);
    }
    let ocr_text = ocr_signature.map(|value| value.text);
    state
        .core_runtime()
        .register_pending_frame_context(PendingFrameContext::bounded(
            context_id.clone(),
            captured_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            trigger,
            memory.front_app.clone(),
            None,
            "fullscreen".to_owned(),
            ocr_text.clone(),
        ))?;
    memory.frames.push(ObservationFrameInput {
        scope_generation: state.core_runtime().watch_scope_generation(),
        context_id,
        captured_at,
        debug_id: debug_id.clone(),
        relative_seconds: memory
            .last_capture
            .duration_since(memory.window_start)
            .as_secs_f64(),
        trigger,
        front_app: memory.front_app.clone(),
        app: None,
        target: "fullscreen".to_owned(),
        ocr_text,
        image_path: provider_path,
    });
    memory.last_accepted = Some(memory.last_capture);
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
    let next_send =
        chrono::Utc::now() + chrono::Duration::milliseconds(config.watch.send_debounce_ms as i64);
    state
        .publish(|snapshot| {
            snapshot.observer.phase = ObserverViewPhase::Idle;
            snapshot.observer.last_captured_at = Some(captured_at.to_rfc3339());
            snapshot.observer.last_trigger = Some(trigger_name(trigger).to_owned());
            snapshot.observer.front_app = memory.front_app.clone();
            snapshot.observer.pending_frame_count = memory.frames.len();
            snapshot.observer.next_send_at = Some(next_send.to_rfc3339());
        })
        .await;
    Ok(CaptureDisposition::Accepted)
}

fn should_skip_self_application(front_app: Option<&str>, tutorial_watch: bool) -> bool {
    !tutorial_watch && front_app.is_some_and(is_self_application)
}

#[allow(clippy::too_many_arguments)]
fn record_gate(
    state: &DesktopState,
    config: &Config,
    trigger: ActivityTriggerKind,
    id: Option<&str>,
    ocr_text: Option<&str>,
    sent: bool,
    reason: &str,
) -> Result<()> {
    if !config.debug.enabled {
        return Ok(());
    }
    let image_file = id.map(|value| format!("frame-{value}.png"));
    let id = id.map_or_else(DebugStore::new_id, str::to_owned);
    DebugStore::from_paths(&state.paths).record_gate(&DebugGateRecord {
        id: id.clone(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        trigger: trigger_name(trigger).to_owned(),
        sent,
        reason: reason.to_owned(),
        image_file,
        ocr_preview: ocr_preview(ocr_text),
    })?;
    Ok(())
}

fn resolve_desktop_ocr_helper(state: &DesktopState, config: &Config) -> Option<std::path::PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let executable_dir = executable.parent()?;
    let environment_override =
        std::env::var_os("COOSENPAI_OCR_HELPER").map(std::path::PathBuf::from);
    let configured = environment_override
        .as_deref()
        .and_then(std::path::Path::to_str)
        .or(config.watch.ocr_gate.executable.as_deref());
    crate::platform::MacHelperResolver.resolve_ocr_helper(
        executable_dir,
        &state.paths.root,
        configured,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureDisposition {
    Accepted,
    Unchanged,
    Suppressed,
    SelfApplication,
    OwnBoundsUnavailable,
    WindowUnavailable,
    MinSpacing,
}

impl CaptureDisposition {
    fn display(self) -> &'static str {
        match self {
            Self::Accepted => "撮影",
            Self::Unchanged => "見送り（画面に変化なし）",
            Self::Suppressed => "見送り（対象が無効です）",
            Self::SelfApplication => "見送り（CooSenpAI が前面）",
            Self::OwnBoundsUnavailable => "見送り（自ウィンドウの範囲を取得できません）",
            Self::WindowUnavailable => "見送り（対象アプリのウィンドウがありません）",
            Self::MinSpacing => "見送り（撮影間隔が短すぎます）",
        }
    }
}

async fn record_capture_decision(
    state: &Arc<DesktopState>,
    trigger: ActivityTriggerKind,
    front_app: &Option<String>,
    disposition: CaptureDisposition,
) {
    let _ = state.logger.write(
        "INFO",
        &format!(
            "撮影判断: trigger={} result={}",
            trigger_name(trigger),
            disposition.display()
        ),
    );
    state
        .publish(|snapshot| {
            snapshot.observer.last_trigger = Some(trigger_name(trigger).to_owned());
            snapshot.observer.front_app = front_app.clone();
            snapshot.observer.last_capture_disposition = Some(disposition.display().to_owned());
        })
        .await;
    state.refresh_debug().await;
}

fn notify_tutorial_observation(state: &Arc<DesktopState>) {
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        if !state.tutorial_is_active().await {
            return;
        }
        let handler_state = state.clone();
        let _ = state
            .dispatch(
                crate::command_guard::CommandSource::TutorialAutomation,
                crate::command_guard::DesktopCommand::TutorialAdvance,
                move |context| async move {
                    handler_state
                        .command_tutorial_watch_started(&context)
                        .await
                        .map_err(crate::command_guard::DispatchError::handler)
                },
            )
            .await;
    });
}

fn capture_exclusions(
    bounds: Result<OwnWindowBounds, PortError>,
    captured_at: chrono::DateTime<chrono::Utc>,
) -> Option<Vec<coosenpai_core::image_processing::ExcludedBounds>> {
    own_window_exclusions(&bounds.ok()?, captured_at)
}

async fn flush_if_due(
    state: &Arc<DesktopState>,
    config: &Config,
    memory: &mut WatchMemory,
    cancellation: CancellationToken,
) -> Result<()> {
    let latest_config = state.runtime_config();
    retain_enabled_frames(&latest_config, &mut memory.frames);
    if memory.frames.is_empty() {
        memory.directories.clear();
        memory.last_accepted = None;
    }
    let Some(accepted) = memory.last_accepted else {
        let _ = state
            .core_runtime()
            .process_mailbox(cancellation)
            .await
            .map_err(anyhow::Error::new)
            .context("Manager の mailbox ACK")?;
        return Ok(());
    };
    let tutorial_watch = state.tutorial_current_step().await == Some(TutorialStep::Watch);
    let due = tutorial_watch
        || watch_send_due(
            config,
            memory.frames.len(),
            accepted.elapsed().as_millis() as u64,
            memory.window_start.elapsed().as_millis() as u64,
        );
    if !due {
        return Ok(());
    }
    state
        .publish(|snapshot| snapshot.observer.phase = ObserverViewPhase::Thinking)
        .await;
    if tutorial_watch {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
    let frames = std::mem::take(&mut memory.frames);
    match state
        .core_runtime()
        .observe(frames.clone(), cancellation.clone())
        .await
    {
        Ok(observation) => {
            memory.directories.clear();
            memory.last_accepted = None;
            memory.window_start = Instant::now();
            memory.last_observation = memory.window_start;
            let calls = coosenpai_core::usage::today_observer_usage(&state.paths.usage)
                .map(|usage| usage.ai_calls)
                .unwrap_or(0);
            state
                .publish(|snapshot| {
                    snapshot.observer.phase = ObserverViewPhase::Idle;
                    snapshot.observer.error_message = None;
                    snapshot.observer.pending_frame_count = 0;
                    snapshot.observer.next_send_at = None;
                    snapshot.observer.record_observation(observation);
                    snapshot.observer.ai_calls_today = calls;
                })
                .await;
            if tutorial_watch {
                notify_tutorial_observation(state);
            }
        }
        Err(coosenpai_core::runtime::RuntimeError::StaleWatchScope) => {
            memory.directories.clear();
            memory.last_accepted = None;
            memory.window_start = Instant::now();
            state
                .publish(|snapshot| {
                    snapshot.observer.phase = ObserverViewPhase::Idle;
                    snapshot.observer.error_message = None;
                    snapshot.observer.pending_frame_count = 0;
                    snapshot.observer.next_send_at = None;
                })
                .await;
        }
        Err(error) => {
            memory.frames = frames;
            return Err(anyhow::Error::new(error).context("Manager の観察 ACK"));
        }
    }
    let _ = state
        .core_runtime()
        .process_mailbox(cancellation)
        .await
        .map_err(anyhow::Error::new)
        .context("Manager の観察後 mailbox ACK")?;
    state.refresh_conversation().await;
    Ok(())
}

fn trigger_name(trigger: ActivityTriggerKind) -> &'static str {
    match trigger {
        ActivityTriggerKind::TypingPaused => "typing-paused",
        ActivityTriggerKind::AppSwitched => "app-switched",
        ActivityTriggerKind::Timer => "timer",
    }
}
