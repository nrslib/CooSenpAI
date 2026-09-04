use anyhow::{Context, Result};
use chrono::Utc;
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::debug::{ocr_preview, DebugGateRecord, DebugStore};
use coosenpai_core::frame_buffer::FrameBuffer;
use coosenpai_core::image_processing::{own_window_exclusions, png_dimensions, process_png};
use coosenpai_core::logging::FileLogger;
use coosenpai_core::notification::NotificationConsumer;
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::ports::{
    ActivityPort, ApplicationCapturePort, Clock, HelperResolverPort, OcrPort, OwnWindowBoundsPort,
    PowerEvent, PowerEventPort, RuntimeLogger, ScreenCapturePort, SystemClock,
};
use coosenpai_core::runtime::RuntimeActor;
use coosenpai_core::state::{ActivityTriggerKind, PendingFrameContext, StagnationObservation};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::platform;
#[path = "watch_bootstrap.rs"]
mod watch_bootstrap;
use self::watch_bootstrap::WatchBootstrap;
#[path = "watch_config.rs"]
mod watch_config;
use self::watch_config::ConfigReload;
use coosenpai_core::watch_coordinator::{
    effective_max_interval_ms, evaluate_activity_poll, is_self_application,
    next_send_seconds as shared_next_send_seconds, normalize_ocr_blocks, retain_enabled_frames,
    watch_send_due, RetryBackoff, StagnationFingerprint, StagnationReportIntent, StagnationTracker,
    TriggerCoordinator, WatchStagnationStore,
};
#[path = "watch_status.rs"]
mod watch_status;
use self::watch_status::WatchStatus;
#[path = "watch_heartbeat.rs"]
mod watch_heartbeat;
use self::watch_heartbeat::{heartbeat_if_due, mark_meaningful_change};
#[path = "watch_app.rs"]
mod watch_app;
use self::watch_app::ApplicationWatchSet;

pub(crate) async fn run(
    home: &Path,
    paths: &ConfigPaths,
    config: Config,
    logger: Arc<FileLogger>,
) -> Result<()> {
    let _ = home;
    let mut active_config = config.clone();
    let Some(mut bootstrap) = WatchBootstrap::start(paths, &active_config, logger.clone()).await?
    else {
        println!("watch stopped");
        return Ok(());
    };
    let cancellation = bootstrap.cancellation();
    if cancellation.is_cancelled() {
        bootstrap.stop().await;
        println!("watch stopped");
        return Ok(());
    }
    let own_windows = platform::MacOwnWindowBounds::empty();
    let agents = bootstrap.take_agents()?;
    let runtime = RuntimeActor::spawn_agents_with_factory_logger_and_cancellation(
        active_config.clone(),
        agents,
        bootstrap.factory(),
        logger.clone(),
        cancellation.clone(),
    );
    let mut runtime_snapshots = runtime.subscribe_snapshots();
    let semaphore = Arc::new(Semaphore::new(2));
    let executable_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .context("coosenpai 実行ファイルのディレクトリを取得できません")?;
    let resolver = platform::MacHelperResolver;
    let mut helper = resolver.resolve_ocr_helper(
        &executable_dir,
        &paths.root,
        active_config.watch.ocr_gate.executable.as_deref(),
    );
    let screen_capture = platform::MacScreenCapture;
    let application_capture = platform::MacApplicationCapture;
    let activity = platform::MacActivity;
    let ocr = platform::MacOcr::new(helper.clone());
    let clock = SystemClock;
    let mut cli_notifications = NotificationConsumer::new(
        paths.mailbox.clone(),
        "cli",
        paths.notification_processed.clone(),
        active_config.notification.min_priority.clone(),
    )?
    .with_logger(logger.clone());
    let mut power_events = platform::MacPowerEvents::new()
        .map_err(|_| anyhow::anyhow!("macOS のスリープ・ロック通知を購読できません"))?;
    let display_geometry = platform::read_display_geometry().await;
    let capture_environment = CaptureEnvironment {
        paths,
        runtime: &runtime,
        semaphore: &semaphore,
        screen_capture: &screen_capture,
        application_capture: &application_capture,
        ocr_port: &ocr,
        own_window_bounds: &own_windows,
        display_geometry: display_geometry.as_ref(),
        clock: &clock,
        cancellation: &cancellation,
    };
    let mut on_battery = active_config.watch.battery.enabled && platform::is_on_battery().await;
    let now = Instant::now();
    let now_utc = Utc::now();
    let initial_activity = activity.read_activity().await.ok();
    let stagnation_store = WatchStagnationStore::new(paths.watch_stagnation.clone());
    let stagnation_snapshot = stagnation_store.load(now_utc).unwrap_or(
        coosenpai_core::watch_coordinator::StagnationSnapshot {
            last_meaningful_change_at: now_utc,
            reported: false,
            pending_report: None,
            fingerprints: Default::default(),
        },
    );
    let mut watch_state = WatchState {
        pending_frames: Vec::new(),
        last_hash: stagnation_snapshot
            .fingerprints
            .get("fullscreen")
            .map(|value| value.image_hash.clone()),
        last_ocr_signature: stagnation_snapshot
            .fingerprints
            .get("fullscreen")
            .and_then(|value| value.ocr_signature.clone()),
        last_capture: now
            .checked_sub(Duration::from_millis(effective_max_interval_ms(
                &active_config,
                on_battery,
            )))
            .unwrap_or(now),
        last_observation: now,
        window_start: now,
        last_accepted: None,
        last_captured_at: None,
        temporary_directories: Vec::new(),
        target_statuses: Vec::new(),
        stagnation: StagnationTracker::resume(
            now,
            stagnation_snapshot.elapsed(now_utc),
            initial_activity.as_ref(),
            stagnation_snapshot.reported,
        ),
        stagnation_store,
        pending_stagnation_report: stagnation_snapshot.pending_report,
        last_meaningful_change_at: stagnation_snapshot.last_meaningful_change_at,
    };
    let mut status = WatchStatus::new(&active_config, helper.is_some());
    status.update_runtime_snapshot(&runtime_snapshots.borrow().clone());
    let mut persistence_retry = RetryBackoff::default();
    status.ai_calls_today = coosenpai_core::usage::today_observer_usage(&paths.usage)
        .map(|usage| usage.ai_calls)
        .unwrap_or(0);
    println!("watch started");
    status.report(&active_config, &watch_state);
    let mut trigger_coordinator =
        TriggerCoordinator::new(&active_config, initial_activity.as_ref());
    let mut application_watch = ApplicationWatchSet::new(
        &active_config,
        initial_activity.as_ref(),
        now,
        &stagnation_snapshot.fingerprints,
    );
    let mut front_app = initial_activity
        .as_ref()
        .and_then(|value| value.front_app.clone());
    let mut power_suspended = false;
    let mut config_reload = ConfigReload::new();
    if active_config.watch.fullscreen && !trigger_coordinator.input_active() {
        let disposition = capture_and_deliver(
            &active_config,
            &capture_environment,
            helper.is_some(),
            front_app.clone(),
            &mut watch_state,
            ActivityTriggerKind::Timer,
        )
        .await?;
        status.report_capture(
            &active_config,
            &watch_state,
            disposition,
            ActivityTriggerKind::Timer,
        );
        if flush_pending(
            &active_config,
            paths,
            &runtime,
            &mut watch_state,
            &mut status,
            &cli_notifications,
        )
        .await
        .is_err()
        {
            persistence_retry.defer(Instant::now());
        }
    }
    let watch_result: Result<()> = {
        let watch_loop = async {
            loop {
                config_reload
                    .refresh(paths, &runtime, logger.as_ref())
                    .await?;
                let latest_config = runtime.config();
                if latest_config != active_config {
                    active_config = latest_config;
                    helper = resolver.resolve_ocr_helper(
                        &executable_dir,
                        &paths.root,
                        active_config.watch.ocr_gate.executable.as_deref(),
                    );
                    ocr.set_helper(helper.clone())?;
                    status.update_config(&active_config, helper.is_some());
                    cli_notifications
                        .update_minimum_priority(active_config.notification.min_priority.clone())?;
                    status.report(&active_config, &watch_state);
                }
                if config_reload.is_degraded() {
                    tokio::select! {
                        _ = cancellation.cancelled() => break,
                        snapshot_change = runtime_snapshots.changed() => {
                            snapshot_change.context("runtime の状態通知が終了しました")?;
                            let snapshot = runtime_snapshots.borrow_and_update().clone();
                            status.update_runtime_snapshot(&snapshot);
                            status.report(&active_config, &watch_state);
                        }
                        _ = tokio::time::sleep(Duration::from_millis(active_config.watch.triggers.poll_ms)) => {}
                    }
                    continue;
                }
                tokio::select! {
                _ = cancellation.cancelled() => break,
                snapshot_change = runtime_snapshots.changed() => {
                        snapshot_change.context("runtime の状態通知が終了しました")?;
                        let snapshot = runtime_snapshots.borrow_and_update().clone();
                        status.update_runtime_snapshot(&snapshot);
                        status.report(&active_config, &watch_state);
                    }
                    event = power_events.next() => {
                        match event? {
                            Some(PowerEvent::Sleep | PowerEvent::Lock) => {
                                power_suspended = true;
                                status.phase = "一時停止";
                                status.report(&active_config, &watch_state);
                            }
                            Some(PowerEvent::Wake | PowerEvent::Unlock) if power_suspended => {
                                power_suspended = false;
                                trigger_coordinator.reset_after_resume();
                                let now = Instant::now();
                                watch_state.last_capture = now
                                    .checked_sub(Duration::from_millis(effective_max_interval_ms(&active_config, on_battery)))
                                    .unwrap_or(now);
                                status.report_idle(&active_config, &watch_state);
                            }
                            Some(_) | None => {}
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(active_config.watch.triggers.poll_ms)) => {
                        if power_suspended {
                            continue;
                        }
                        if active_config.watch.battery.enabled {
                            on_battery = platform::is_on_battery().await;
                        }
                        let now = Instant::now();
                        let activity_snapshot = activity.read_activity().await.ok();
                        let fresh_activity = watch_state.stagnation.observe_activity(
                            activity_snapshot.as_ref(),
                            active_config.watch.triggers.active_threshold_ms,
                        );
                        if fresh_activity && watch_state.stagnation.is_reported() {
                            let reacted_at = Utc::now();
                            match watch_state.stagnation_store.record_reaction(reacted_at) {
                                Ok(true) => {
                                    watch_state.stagnation.mark_meaningful_change(Instant::now());
                                    watch_state.pending_stagnation_report = None;
                                    watch_state.last_meaningful_change_at = reacted_at;
                                }
                                Ok(false) => {}
                                Err(error) => eprintln!(
                                    "停滞エピソードの操作反応を保存できませんでした: {error}"
                                ),
                            }
                        }
                        let max_interval_ms = effective_max_interval_ms(&active_config, on_battery);
                        let decision = evaluate_activity_poll(
                            &mut trigger_coordinator,
                            &active_config,
                            activity_snapshot.as_ref(),
                            now,
                            now.duration_since(watch_state.last_capture).as_millis() as u64,
                            max_interval_ms,
                        );
                        front_app = decision.front_app;
                        let trigger = active_config.watch.fullscreen.then_some(decision.trigger).flatten();
                        if let Some(trigger) = trigger {
                            if coosenpai_core::timing::min_spacing_reached(
                                    now.duration_since(watch_state.last_capture).as_millis() as u64,
                                    active_config.watch.triggers.min_spacing_ms,
                                ) {
                                let disposition = capture_and_deliver(
                                    &active_config,
                                    &capture_environment,
                                    helper.is_some(),
                                    front_app.clone(),
                                    &mut watch_state,
                                    trigger,
                                )
                                .await?;
                                status.report_capture(&active_config, &watch_state, disposition, trigger);
                            } else {
                                record_cli_gate(
                                    paths,
                                    &active_config,
                                    trigger,
                                    None,
                                    None,
                                    false,
                                    "最低間隔",
                                )?;
                            }
                        }
                        application_watch.poll(
                            &active_config,
                            &capture_environment,
                            helper.is_some(),
                            activity_snapshot.as_ref(),
                            max_interval_ms,
                            front_app.clone(),
                            &mut watch_state,
                        ).await?;
                        if persistence_retry.is_due(Instant::now()) {
                            match flush_pending(
                                &active_config,
                                paths,
                                &runtime,
                                &mut watch_state,
                                &mut status,
                                &cli_notifications,
                            )
                            .await
                            {
                                Ok(()) => {
                                    match heartbeat_if_due(
                                        &active_config,
                                        &runtime,
                                        &mut watch_state,
                                        max_interval_ms,
                                    )
                                    .await
                                    {
                                        Ok(()) => persistence_retry.reset(),
                                        Err(_) => persistence_retry.defer(Instant::now()),
                                    }
                                }
                                Err(_) => persistence_retry.defer(Instant::now()),
                            }
                        }
                        status.ai_calls_today = coosenpai_core::usage::today_observer_usage(&paths.usage)
                            .map(|usage| usage.ai_calls)
                            .unwrap_or(status.ai_calls_today);
                    }
                }
            }
            Ok(())
        };
        tokio::pin!(watch_loop);
        tokio::select! {
            result = &mut watch_loop => result,
            _ = cancellation.cancelled() => Ok(()),
        }
    };
    status.phase = "終了中";
    status.trigger = "なし";
    status.report(&active_config, &watch_state);
    let interrupted = cancellation.is_cancelled();
    match super::shutdown::wait_for_cleanup(
        runtime.shutdown(),
        super::shutdown::WATCH_SHUTDOWN_TIMEOUT,
    )
    .await
    {
        Some(result) => result?,
        None => {
            let _ = logger.write("WARN", "終了処理がtimeoutしました: error-type=shutdown");
        }
    }
    bootstrap.stop().await;
    let _ = FrameBuffer::new(paths.frame_buffer.clone()).cleanup_expired(Utc::now());
    if !interrupted {
        watch_result?;
    }
    println!("watch stopped");
    Ok(())
}

fn ocr_disabled_reason(config: &Config, helper_available: bool) -> Option<&'static str> {
    match (config.watch.ocr_gate.enabled, helper_available) {
        (true, false) => Some("coosenpai-ocr が見つかりません"),
        _ => None,
    }
}

struct WatchState {
    pending_frames: Vec<ObservationFrameInput>,
    last_hash: Option<String>,
    last_ocr_signature: Option<String>,
    last_capture: Instant,
    last_observation: Instant,
    window_start: Instant,
    last_accepted: Option<Instant>,
    last_captured_at: Option<chrono::DateTime<Utc>>,
    temporary_directories: Vec<tempfile::TempDir>,
    target_statuses: Vec<TargetStatus>,
    stagnation: StagnationTracker,
    stagnation_store: WatchStagnationStore,
    pending_stagnation_report: Option<StagnationReportIntent>,
    last_meaningful_change_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetStatus {
    name: String,
    enabled: bool,
    foreground: bool,
    last_captured_at: Option<chrono::DateTime<Utc>>,
    trigger: Option<&'static str>,
}

struct CaptureEnvironment<'a> {
    paths: &'a ConfigPaths,
    runtime: &'a coosenpai_core::runtime::RuntimeHandle,
    semaphore: &'a Arc<Semaphore>,
    screen_capture: &'a dyn ScreenCapturePort,
    application_capture: &'a dyn ApplicationCapturePort,
    ocr_port: &'a dyn OcrPort,
    own_window_bounds: &'a dyn OwnWindowBoundsPort,
    display_geometry: Option<&'a platform::DisplayGeometry>,
    clock: &'a dyn Clock,
    cancellation: &'a tokio_util::sync::CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureDisposition {
    Accepted,
    Unchanged,
    Suppressed,
}

fn trigger_label(trigger: ActivityTriggerKind) -> &'static str {
    match trigger {
        ActivityTriggerKind::TypingPaused => "入力が止まった",
        ActivityTriggerKind::AppSwitched => "アプリが切り替わった",
        ActivityTriggerKind::Timer => "定期撮影",
    }
}

fn next_send_seconds(config: &Config, state: &WatchState) -> u64 {
    let now = Instant::now();
    shared_next_send_seconds(
        config,
        state
            .last_accepted
            .map(|accepted| now.duration_since(accepted).as_millis() as u64),
        now.duration_since(state.window_start).as_millis() as u64,
    )
}

async fn capture_and_deliver(
    config: &Config,
    environment: &CaptureEnvironment<'_>,
    ocr_helper_available: bool,
    front_app: Option<String>,
    state: &mut WatchState,
    trigger: ActivityTriggerKind,
) -> Result<CaptureDisposition> {
    if front_app.as_deref().is_some_and(is_self_application) {
        record_cli_gate(
            environment.paths,
            config,
            trigger,
            None,
            None,
            false,
            "自ウィンドウ",
        )?;
        return Ok(CaptureDisposition::Suppressed);
    }
    let own_windows = match environment.own_window_bounds.read_own_window_bounds().await {
        Ok(own_windows) => own_windows,
        Err(_) => {
            record_cli_gate(
                environment.paths,
                config,
                trigger,
                None,
                None,
                false,
                "自ウィンドウ",
            )?;
            return Ok(CaptureDisposition::Suppressed);
        }
    };
    let captured_at_utc = environment.clock.now();
    let excluded = match own_window_exclusions(&own_windows, captured_at_utc) {
        Some(excluded) => excluded,
        None => {
            record_cli_gate(
                environment.paths,
                config,
                trigger,
                None,
                None,
                false,
                "自ウィンドウ",
            )?;
            return Ok(CaptureDisposition::Suppressed);
        }
    };
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("screen.png");
    let captured_path = environment
        .screen_capture
        .capture(&source, environment.cancellation.clone())
        .await?;
    let bytes = tokio::fs::read(captured_path).await?;
    let (source_width, source_height) =
        png_dimensions(&bytes).context("撮影処理が有効な PNG の寸法を返しませんでした")?;
    let ignored_top_pixels =
        platform::comparison_top_pixels(environment.display_geometry, source_width, source_height);
    let processed = process_png(
        bytes,
        config.watch.downscale_width,
        ignored_top_pixels,
        excluded.clone(),
        environment.semaphore.clone(),
    )
    .await?;
    let provider_path = directory.path().join("provider.png");
    tokio::fs::write(&provider_path, &processed.provider_png).await?;
    let ocr_path = directory.path().join("ocr.png");
    tokio::fs::write(&ocr_path, &processed.masked_png).await?;
    let ocr = if config.watch.ocr_gate.enabled && ocr_helper_available {
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
            .map(|blocks| {
                normalize_ocr_blocks(
                    &blocks,
                    source_width,
                    source_height,
                    &excluded,
                    ignored_top_pixels,
                )
            })
    } else {
        None
    };
    let captured_at = Instant::now();
    let changed_by_ocr = matches!((&ocr, &state.last_ocr_signature), (Some(_), Some(_)));
    let changed = match (&ocr, &state.last_ocr_signature) {
        (Some(current), Some(previous)) => current.signature != *previous,
        _ => state.last_hash.as_deref() != Some(processed.comparison_hash.as_str()),
    };
    let context_id = DebugStore::new_id();
    let debug_id = config.debug.enabled.then(|| context_id.clone());
    if let Some(id) = &debug_id {
        DebugStore::from_paths(environment.paths).record_frame(
            id,
            captured_at_utc,
            &processed.provider_png,
            ocr.as_ref().map(|value| value.text.as_str()),
        )?;
    }
    state.last_capture = captured_at;
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
    state.last_hash = Some(processed.comparison_hash);
    state.last_ocr_signature = ocr.as_ref().map(|value| value.signature.clone());
    let stagnation_hash = state.last_hash.clone().unwrap_or_default();
    let stagnation_ocr = state.last_ocr_signature.clone();
    mark_meaningful_change(
        state,
        "fullscreen",
        stagnation_hash,
        stagnation_ocr,
        captured_at_utc,
    )?;
    let ocr_text = ocr.map(|value| value.text);
    environment
        .runtime
        .register_pending_frame_context(PendingFrameContext::bounded(
            context_id.clone(),
            captured_at_utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            trigger,
            front_app.clone(),
            None,
            "fullscreen".to_owned(),
            ocr_text.clone(),
        ))?;
    let frame = ObservationFrameInput {
        scope_generation: environment.runtime.watch_scope_generation(),
        context_id,
        captured_at: captured_at_utc,
        debug_id: debug_id.clone(),
        relative_seconds: captured_at.duration_since(state.window_start).as_secs_f64(),
        trigger,
        front_app,
        app: None,
        target: "fullscreen".to_owned(),
        ocr_text,
        image_path: provider_path,
    };
    state.pending_frames.push(frame);
    record_cli_gate(
        environment.paths,
        config,
        trigger,
        debug_id.as_deref(),
        state
            .pending_frames
            .last()
            .and_then(|frame| frame.ocr_text.as_deref()),
        true,
        "送った",
    )?;
    state.last_accepted = Some(captured_at);
    state.last_captured_at = Some(captured_at_utc);
    state.temporary_directories.push(directory);
    Ok(CaptureDisposition::Accepted)
}

#[allow(clippy::too_many_arguments)]
fn record_cli_gate(
    paths: &ConfigPaths,
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
    DebugStore::from_paths(paths).record_gate(&DebugGateRecord {
        id,
        created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        trigger: match trigger {
            ActivityTriggerKind::TypingPaused => "typing-paused",
            ActivityTriggerKind::AppSwitched => "app-switched",
            ActivityTriggerKind::Timer => "timer",
        }
        .to_owned(),
        sent,
        reason: reason.to_owned(),
        image_file,
        ocr_preview: ocr_preview(ocr_text),
    })?;
    Ok(())
}

async fn flush_pending(
    config: &Config,
    paths: &ConfigPaths,
    runtime: &coosenpai_core::runtime::RuntimeHandle,
    state: &mut WatchState,
    status: &mut WatchStatus,
    cli_notifications: &NotificationConsumer,
) -> Result<()> {
    let latest_config = coosenpai_core::config::load_config(paths)?;
    retain_enabled_frames(&latest_config, &mut state.pending_frames);
    if state.pending_frames.is_empty() {
        state.last_accepted = None;
        state.temporary_directories.clear();
    }
    let due = state.last_accepted.is_some_and(|accepted_at| {
        let now = Instant::now();
        watch_send_due(
            config,
            state.pending_frames.len(),
            now.duration_since(accepted_at).as_millis() as u64,
            now.duration_since(state.window_start).as_millis() as u64,
        )
    });
    if due {
        let accepted_at = state
            .last_accepted
            .ok_or_else(|| anyhow::anyhow!("送信待ち時刻がありません"))?;
        let now = Instant::now();
        let frames = std::mem::take(&mut state.pending_frames);
        let previous_window_start = state.window_start;
        state.last_accepted = None;
        state.window_start = now;
        status.report_observation_start(config, state);
        match runtime.observe(frames.clone()).await {
            Ok(_) => {}
            Err(coosenpai_core::runtime::RuntimeError::StaleWatchScope) => {
                state.temporary_directories.clear();
                state.last_observation = now;
            }
            Err(error) => {
                state.pending_frames = frames;
                state.last_accepted = Some(accepted_at);
                state.window_start = previous_window_start;
                return Err(error.into());
            }
        }
        state.temporary_directories.clear();
        state.last_observation = now;
    }
    if due {
        status.ai_calls_today = coosenpai_core::usage::today_observer_usage(&paths.usage)
            .map(|usage| usage.ai_calls)
            .unwrap_or(status.ai_calls_today);
        status.report_idle(config, state);
    }
    runtime.process_companion_mailbox().await?;
    let companion_name = runtime.snapshot().companion_display_name;
    while let Some(notification) = cli_notifications.claim_next()? {
        println!("{companion_name}: {}", notification.record.message);
        cli_notifications.accept(notification)?;
    }
    Ok(())
}

