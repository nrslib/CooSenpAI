use crate::platform;
use crate::state::DesktopState;
use crate::watch_presenter::WatchResult;
use anyhow::{Context, Result};
use coosenpai_core::capture_mask::{capture_with_window_mask, CaptureWithMaskError};
use coosenpai_core::config::Config;
use coosenpai_core::debug::{ocr_preview, DebugGateRecord, DebugStore};
use coosenpai_core::frame_buffer::FrameBuffer;
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::onboarding::TutorialStep;
use coosenpai_core::ports::{
    ActivityPort, FocusElementPort, HelperResolverPort, PortError, RuntimeLogger, ScreenCapturePort,
};
use coosenpai_core::screen_frames::{attach_own_window_context, prepare_screen_frames};
use coosenpai_core::state::{
    ActivityTriggerKind, ObservationRecord, PendingFrameContext, StagnationObservation,
};
use coosenpai_core::watch_coordinator::{
    effective_max_interval_ms, evaluate_activity_poll, frame_target_is_enabled,
    is_self_application, watch_send_due, StagnationFingerprint, StagnationReportIntent,
    StagnationTracker, TriggerCoordinator, WatchStagnationStore,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[path = "watch_app.rs"]
mod watch_app;
use watch_app::ApplicationWatchSet;
#[path = "watch_heartbeat.rs"]
mod watch_heartbeat;
use watch_heartbeat::{heartbeat_if_due, mark_meaningful_change, process_mailbox_if_possible};
#[path = "watch_recovery.rs"]
mod watch_recovery;
pub(crate) use watch_recovery::WatchErrorPublisher;
use watch_recovery::{
    record_watch_exit, record_watch_failure, WatchRecovery, WatchRecoveryDecision, WatchSessionKind,
};
#[path = "watch_worker.rs"]
mod watch_worker;

#[async_trait::async_trait]
pub(crate) trait WatchWorker: Send {
    async fn poll(&mut self) -> Result<ControlFlow<()>>;

    async fn shutdown(&mut self) {}
}

#[async_trait::async_trait]
pub(crate) trait WatchHost: WatchErrorPublisher + 'static {
    type Worker: WatchWorker;

    fn logger(&self) -> &dyn RuntimeLogger;
    fn cancellation(&self) -> &CancellationToken;
    fn frame_buffer(&self) -> FrameBuffer;
    async fn create_worker(
        self: Arc<Self>,
        generation: u64,
        cancellation: CancellationToken,
        publication: coosenpai_core::persistence::PublicationGate,
    ) -> Result<Self::Worker>;
    async fn clear_watch_error(&self, generation: u64);
    async fn watch_finished(&self, generation: u64, failed: bool);
}

pub struct WatchTask {
    publication: coosenpai_core::persistence::PublicationGate,
    task: tauri::async_runtime::JoinHandle<()>,
}

impl WatchTask {
    pub fn cancel(&self) {
        self.publication.cancel();
    }

    pub async fn wait(self) {
        let _ = self.task.await;
    }

    // 停止を待っても完了しない worker を再現する
}

pub(crate) fn spawn<H: WatchHost>(
    state: Arc<H>,
    generation: u64,
    tutorial_active: bool,
) -> Result<WatchTask> {
    let session_kind = WatchSessionKind::for_tutorial(tutorial_active);
    let cancellation = state.cancellation().child_token();
    let frame_buffer = state.frame_buffer();
    let publication = coosenpai_core::persistence::PublicationGate::new(cancellation.clone());
    let task_publication = publication.clone();
    let task_cancellation = cancellation.clone();
    let worker = tauri::async_runtime::spawn({
        let state = state.clone();
        async move {
            run(
                state,
                generation,
                task_cancellation,
                task_publication,
                session_kind,
            )
            .await
        }
    });
    let task =
        tauri::async_runtime::spawn(supervise_watch_worker(worker, move |result| async move {
            let failed = record_watch_exit(&state, generation, result, session_kind).await;
            state.watch_finished(generation, failed).await;
            let _ = frame_buffer.cleanup_expired(chrono::Utc::now());
        }));
    Ok(WatchTask { publication, task })
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
    publication: coosenpai_core::persistence::PublicationGate,
    pending: PendingCaptureSets,
    active_work: Option<JoinHandle<WatchWorkCompletion>>,
    last_hash: Option<String>,
    last_ocr: Option<String>,
    last_capture: Instant,
    last_observation: Instant,
    window_start: Instant,
    front_app: Option<String>,
    stagnation: StagnationTracker,
    stagnation_store: WatchStagnationStore,
    pending_stagnation_report: Option<StagnationReportIntent>,
    last_meaningful_change_at: chrono::DateTime<chrono::Utc>,
}

struct PendingCaptureSet {
    scope_generation: u64,
    config_revision: u64,
    user_input_sequence: u64,
    accepted_at: Instant,
    frames: Vec<ObservationFrameInput>,
    directory: tempfile::TempDir,
}

pub(super) struct PendingCaptureSetInput {
    pub(super) target: String,
    pub(super) scope_generation: u64,
    pub(super) config_revision: u64,
    pub(super) user_input_sequence: u64,
    pub(super) accepted_at: Instant,
    pub(super) frames: Vec<ObservationFrameInput>,
    pub(super) directory: tempfile::TempDir,
}

#[derive(Default)]
struct PendingCaptureSets {
    by_target: BTreeMap<String, PendingCaptureSet>,
    last_accepted: Option<Instant>,
}

impl PendingCaptureSets {
    fn enqueue(&mut self, input: PendingCaptureSetInput) {
        if input.frames.is_empty() {
            return;
        }
        self.by_target.insert(
            input.target,
            PendingCaptureSet {
                scope_generation: input.scope_generation,
                config_revision: input.config_revision,
                user_input_sequence: input.user_input_sequence,
                accepted_at: input.accepted_at,
                frames: input.frames,
                directory: input.directory,
            },
        );
        self.last_accepted = self.by_target.values().map(|set| set.accepted_at).max();
    }

    fn retain_current_capture_sets(
        &mut self,
        config: &Config,
        scope_generation: u64,
        config_revision: u64,
        user_input_sequence: u64,
    ) {
        self.by_target.retain(|target, set| {
            set.scope_generation == scope_generation
                && set.config_revision == config_revision
                && set.user_input_sequence == user_input_sequence
                && frame_target_is_enabled(config, target)
                && set
                    .frames
                    .iter()
                    .all(|frame| frame.scope_generation == scope_generation)
        });
        self.last_accepted = self.by_target.values().map(|set| set.accepted_at).max();
    }

    fn pending_frame_count(&self) -> usize {
        self.by_target.values().map(|set| set.frames.len()).sum()
    }

    fn take_pending_capture_sets(
        &mut self,
    ) -> (Vec<ObservationFrameInput>, Vec<tempfile::TempDir>) {
        let mut frames = Vec::new();
        let mut directories = Vec::new();
        for (_, mut set) in std::mem::take(&mut self.by_target) {
            frames.append(&mut set.frames);
            directories.push(set.directory);
        }
        self.last_accepted = None;
        (frames, directories)
    }

    fn clear(&mut self) {
        self.by_target.clear();
        self.last_accepted = None;
    }
}

impl WatchMemory {
    pub(super) fn enqueue_capture_set(&mut self, input: PendingCaptureSetInput) {
        self.pending.enqueue(input);
    }

    fn retain_current_capture_sets(
        &mut self,
        config: &Config,
        scope_generation: u64,
        config_revision: u64,
        user_input_sequence: u64,
    ) {
        self.pending.retain_current_capture_sets(
            config,
            scope_generation,
            config_revision,
            user_input_sequence,
        );
    }

    fn pending_frame_count(&self) -> usize {
        self.pending.pending_frame_count()
    }

    fn has_pending_capture_sets(&self) -> bool {
        !self.pending.by_target.is_empty()
    }

    fn start_work(
        &mut self,
        work: impl Future<Output = WatchWorkCompletion> + Send + 'static,
    ) -> bool {
        if self.active_work.is_some() {
            return false;
        }
        self.active_work = Some(tokio::spawn(work));
        true
    }

    fn last_accepted_capture(&self) -> Option<Instant> {
        self.pending.last_accepted
    }

    fn take_pending_capture_sets(
        &mut self,
    ) -> (Vec<ObservationFrameInput>, Vec<tempfile::TempDir>) {
        self.pending.take_pending_capture_sets()
    }
}

fn watch_error_detail(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

async fn run<H: WatchHost>(
    state: Arc<H>,
    generation: u64,
    cancellation: CancellationToken,
    publication: coosenpai_core::persistence::PublicationGate,
    session_kind: WatchSessionKind,
) -> Result<()> {
    let mut worker = state
        .clone()
        .create_worker(generation, cancellation.clone(), publication)
        .await?;
    let mut recovery = WatchRecovery::new(session_kind);
    loop {
        match worker.poll().await {
            Ok(ControlFlow::Break(())) => {
                worker.shutdown().await;
                break;
            }
            Ok(ControlFlow::Continue(())) => {
                recovery.reset();
                state.clear_watch_error(generation).await;
            }
            Err(error) => {
                if cancellation.is_cancelled() {
                    worker.shutdown().await;
                    break;
                }
                match record_watch_failure(&state, &mut recovery, &error, generation, &cancellation)
                    .await
                {
                    WatchRecoveryDecision::Retry | WatchRecoveryDecision::ConfigUpdateCancelled => {
                    }
                    WatchRecoveryDecision::Stop => {
                        worker.shutdown().await;
                        return Err(error);
                    }
                    WatchRecoveryDecision::Cancelled => {
                        worker.shutdown().await;
                        break;
                    }
                }
            }
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
    focus_element: &Arc<dyn FocusElementPort>,
    ocr: &platform::MacOcr,
    semaphore: &Arc<Semaphore>,
    memory: &mut WatchMemory,
    trigger: ActivityTriggerKind,
    generation: u64,
    cancellation: CancellationToken,
) -> Result<CaptureDisposition> {
    let tutorial_watch = state.tutorial_current_step().await == Some(TutorialStep::Watch);
    if should_skip_self_application(memory.front_app.as_deref(), tutorial_watch) {
        memory.last_capture = Instant::now();
        record_gate(
            state,
            config,
            trigger,
            None,
            None,
            false,
            "自ウィンドウ",
            &memory.publication,
        )?;
        return Ok(CaptureDisposition::SelfApplication);
    }
    state
        .publish_watch_view(generation, WatchResult::CaptureStarted)
        .await;
    // 画面全体とアプリ対象の見守り撮影を直列化する。
    let Some(_screen_gate) =
        crate::screen_capture_gate::acquire_screen_capture_gate(state, &cancellation).await
    else {
        return Err(anyhow::anyhow!("見守りの撮影が取り消されました"));
    };
    let runtime = state.core_runtime();
    let scope_generation = runtime.watch_scope_generation();
    let user_input_sequence = runtime.user_input_sequence();
    ensure_capture_active(&cancellation)?;
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("screens");
    let focus_task = config.watch.focus_element.then(|| {
        let focus_element = focus_element.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            coosenpai_core::focus::read_focused_element(focus_element.as_ref(), &cancellation, None)
                .await
        })
    });
    let mut user_response_in_progress = None;
    let stable = capture_with_window_mask(state.own_bounds.as_ref(), async {
        let capture_started = Instant::now();
        let _ = state.logger.write("INFO", "見守り: 段階=capture-start target=fullscreen backend=in-process");
        user_response_in_progress =
            Some(state.snapshot().await.active_user_message_id.is_some());
        let captured = screen_capture.capture(&source, cancellation.clone()).await;
        let _ = state.logger.write("INFO", &format!(
            "見守り: 段階=capture-done target=fullscreen backend=in-process elapsed-ms={} success={} display-count={}",
            capture_started.elapsed().as_millis(), captured.is_ok(), captured.as_ref().map_or(0, Vec::len),
        ));
        if cancellation.is_cancelled() {
            return Err(PortError::Unavailable("見守りの撮影が取り消されました".to_owned()));
        }
        match &captured {
            Ok(_) => state.record_screen_capture_result(true).await,
            Err(PortError::ScreenCapturePermission(_)) => state.record_screen_capture_result(false).await,
            Err(_) => {},
        }
        captured
    }).await;
    let focus = match focus_task {
        Some(task) => task.await.ok().flatten(),
        None => None,
    };
    let stable = match stable {
        Ok(stable) => stable,
        Err(CaptureWithMaskError::Capture(error)) => return Err(anyhow::Error::new(error)),
        Err(CaptureWithMaskError::OwnWindows) => {
            state
                .publish_watch_view(generation, WatchResult::CaptureSkipped)
                .await;
            record_gate(
                state,
                config,
                trigger,
                None,
                None,
                false,
                "自ウィンドウ",
                &memory.publication,
            )?;
            return Ok(CaptureDisposition::OwnBoundsUnavailable);
        }
    };
    let captured_at = stable.captured_at;
    ensure_capture_active(&cancellation)?;
    let mut prepared = prepare_screen_frames(
        stable.screens,
        directory.path(),
        config,
        &stable.exclusions,
        ocr,
        ocr_enabled,
        semaphore.clone(),
        &cancellation,
    )
    .await?;
    attach_own_window_context(
        &mut prepared.frames,
        &stable.own_windows,
        user_response_in_progress,
    );
    let changed_by_ocr = prepared.ocr_signature.is_some() && memory.last_ocr.is_some();
    let changed = match (&prepared.ocr_signature, &memory.last_ocr) {
        (Some(current), Some(previous)) => current != previous,
        _ => memory.last_hash.as_deref() != Some(prepared.comparison_hash.as_str()),
    };
    memory.last_capture = Instant::now();
    if changed && !frame_target_is_enabled(&state.runtime_config(), "fullscreen") {
        return Ok(CaptureDisposition::Suppressed);
    }
    let mut frames = Vec::with_capacity(prepared.frames.len());
    for screen in prepared.frames {
        ensure_capture_active(&cancellation)?;
        let frame = screen.observation_frame(
            scope_generation,
            captured_at,
            memory
                .last_capture
                .duration_since(memory.window_start)
                .as_secs_f64(),
            trigger,
            memory.front_app.clone(),
            focus.clone(),
            config.debug.enabled,
        );
        let debug_id = frame.debug_id.clone();
        let ocr_text = frame.ocr_text.clone();
        if ocr_enabled && ocr_text.is_none() {
            let _ = state
                .logger
                .write("WARN", "Vision OCR に失敗しました: error-type=ocr");
        }
        if let Some(id) = &debug_id {
            DebugStore::from_paths(&state.paths)
                .with_publication_gate(memory.publication.clone())
                .record_frame(
                    id,
                    captured_at,
                    &screen.image.provider_png,
                    ocr_text.as_deref(),
                )?;
        }
        record_gate(
            state,
            config,
            trigger,
            debug_id.as_deref(),
            ocr_text.as_deref(),
            changed,
            if changed {
                "送った"
            } else if changed_by_ocr {
                "OCR 一致"
            } else {
                "画素一致"
            },
            &memory.publication,
        )?;
        frames.push(frame);
    }
    if !changed {
        state
            .publish_watch_view(generation, WatchResult::CaptureSkipped)
            .await;
        return Ok(CaptureDisposition::Unchanged);
    }
    ensure_capture_active(&cancellation)?;
    for frame in &frames {
        state.core_runtime().register_pending_frame_context(
            PendingFrameContext::bounded_with_focus(
                frame.context_id.clone(),
                captured_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                trigger,
                frame.front_app.clone(),
                None,
                frame.target.clone(),
                frame.ocr_text.clone(),
                frame.focus.clone(),
            ),
            Some(&memory.publication),
        )?;
    }
    memory.last_hash = Some(prepared.comparison_hash.clone());
    memory.last_ocr = prepared.ocr_signature.clone();
    mark_meaningful_change(
        memory,
        "fullscreen",
        prepared.comparison_hash,
        prepared.ocr_signature,
        captured_at,
    )?;
    let accepted_at = memory.last_capture;
    memory.enqueue_capture_set(PendingCaptureSetInput {
        target: "fullscreen".to_owned(),
        scope_generation,
        config_revision: config.revision,
        user_input_sequence,
        frames,
        directory,
        accepted_at,
    });
    let next_send =
        chrono::Utc::now() + chrono::Duration::milliseconds(config.watch.send_debounce_ms as i64);
    state
        .publish_watch_view(
            generation,
            WatchResult::Buffered {
                captured_at: captured_at.to_rfc3339(),
                trigger,
                front_app: memory.front_app.clone(),
                frame_count: memory.pending_frame_count(),
                next_send: next_send.to_rfc3339(),
            },
        )
        .await;
    Ok(CaptureDisposition::Accepted)
}

fn ensure_capture_active(cancellation: &CancellationToken) -> Result<()> {
    if cancellation.is_cancelled() {
        anyhow::bail!("見守りの撮影が取り消されました");
    }
    Ok(())
}

fn should_skip_self_application(front_app: Option<&str>, tutorial_watch: bool) -> bool {
    !tutorial_watch && front_app.is_some_and(is_self_application)
}

fn pending_capture_batch_is_current(
    state: &DesktopState,
    config_revision: u64,
    scope_generation: u64,
    user_input_sequence: u64,
    frames: &[ObservationFrameInput],
) -> bool {
    let config = state.runtime_config();
    let runtime = state.core_runtime();
    config.revision == config_revision
        && runtime.watch_scope_generation() == scope_generation
        && runtime.user_input_sequence() == user_input_sequence
        && frames.iter().all(|frame| {
            frame.scope_generation == scope_generation
                && frame_target_is_enabled(&config, &frame.target)
        })
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
    publication: &coosenpai_core::persistence::PublicationGate,
) -> Result<()> {
    if !config.debug.enabled {
        return Ok(());
    }
    let image_file = id.map(|value| format!("frame-{value}.png"));
    let id = id.map_or_else(DebugStore::new_id, str::to_owned);
    DebugStore::from_paths(&state.paths)
        .with_publication_gate(publication.clone())
        .record_gate(&DebugGateRecord {
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
pub(crate) enum CaptureDisposition {
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

    pub(crate) fn display_for_locale(self, locale: coosenpai_core::locale::Locale) -> &'static str {
        let key = match self {
            Self::Accepted => coosenpai_core::locale::TextKey::CaptureDispositionAccepted,
            Self::Unchanged => coosenpai_core::locale::TextKey::CaptureDispositionUnchanged,
            Self::Suppressed => coosenpai_core::locale::TextKey::CaptureDispositionSuppressed,
            Self::SelfApplication => {
                coosenpai_core::locale::TextKey::CaptureDispositionSelfApplication
            }
            Self::OwnBoundsUnavailable => {
                coosenpai_core::locale::TextKey::CaptureDispositionOwnBoundsUnavailable
            }
            Self::WindowUnavailable => {
                coosenpai_core::locale::TextKey::CaptureDispositionWindowUnavailable
            }
            Self::MinSpacing => coosenpai_core::locale::TextKey::CaptureDispositionMinSpacing,
        };
        coosenpai_core::locale::text(key, locale)
    }
}

async fn record_capture_decision(
    state: &Arc<DesktopState>,
    generation: u64,
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
        .publish_watch_view(
            generation,
            WatchResult::CaptureDecision {
                trigger,
                front_app: front_app.clone(),
                disposition,
            },
        )
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

async fn flush_if_due(
    state: &Arc<DesktopState>,
    _config: &Config,
    memory: &mut WatchMemory,
    generation: u64,
    cancellation: CancellationToken,
) -> Result<()> {
    let latest_config = state.runtime_config();
    let runtime = state.core_runtime();
    let current_scope_generation = runtime.watch_scope_generation();
    let user_input_sequence = runtime.user_input_sequence();
    memory.retain_current_capture_sets(
        &latest_config,
        current_scope_generation,
        latest_config.revision,
        user_input_sequence,
    );
    let mut completed_work = false;
    if memory
        .active_work
        .as_ref()
        .is_some_and(|work| work.is_finished())
    {
        let work = memory.active_work.take().expect("finished watch work");
        match work.await {
            Ok(WatchWorkCompletion::Observation { observation, error }) => {
                if observation.is_some() {
                    memory.last_observation = Instant::now();
                }
                memory.window_start = Instant::now();
                if let Some(error) = error {
                    return Err(error);
                }
            }
            Ok(WatchWorkCompletion::Mailbox(result)) => result?,
            Err(error) => {
                return Err(anyhow::Error::new(error).context("観察ワーク task"));
            }
        }
        completed_work = true;
    }

    if memory.active_work.is_some() {
        return Ok(());
    }

    let accepted = memory.last_accepted_capture();
    if !memory.has_pending_capture_sets() {
        if !completed_work {
            let state = state.clone();
            let cancellation = cancellation.clone();
            let started = memory.start_work(async move {
                WatchWorkCompletion::Mailbox(
                    process_mailbox_if_possible(&state, cancellation, "flush").await,
                )
            });
            debug_assert!(
                started,
                "watch work slot was checked before starting mailbox work"
            );
        }
        return Ok(());
    }

    let tutorial_watch = state.tutorial_current_step().await == Some(TutorialStep::Watch);
    let Some(accepted) = accepted else {
        return Ok(());
    };
    let due = tutorial_watch
        || watch_send_due(
            &latest_config,
            memory.pending_frame_count(),
            accepted.elapsed().as_millis() as u64,
            memory.window_start.elapsed().as_millis() as u64,
        );
    if !due {
        return Ok(());
    }
    state
        .publish_watch_view(generation, WatchResult::ObservationStarted)
        .await;
    let current_config = state.runtime_config();
    let runtime = state.core_runtime();
    let current_scope_generation = runtime.watch_scope_generation();
    let user_input_sequence = runtime.user_input_sequence();
    memory.retain_current_capture_sets(
        &current_config,
        current_scope_generation,
        current_config.revision,
        user_input_sequence,
    );
    if !memory.has_pending_capture_sets() {
        state
            .publish_watch_view(generation, WatchResult::NotDelivered)
            .await;
        memory.window_start = Instant::now();
        return Ok(());
    }
    let batch_config_revision = current_config.revision;
    let batch_scope_generation = current_scope_generation;
    let batch_user_input_sequence = user_input_sequence;
    let (frames, directories) = memory.take_pending_capture_sets();
    memory.window_start = Instant::now();
    let state = state.clone();
    let cancellation_for_work = cancellation.clone();
    let started = memory.start_work(async move {
        let _directories = directories;
        if tutorial_watch {
            tokio::select! {
                () = cancellation_for_work.cancelled() => {
                    return WatchWorkCompletion::Observation { observation: None, error: None };
                }
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
        if !pending_capture_batch_is_current(
            &state,
            batch_config_revision,
            batch_scope_generation,
            batch_user_input_sequence,
            &frames,
        ) {
            state
                .publish_watch_view(generation, WatchResult::NotDelivered)
                .await;
            return WatchWorkCompletion::Observation {
                observation: None,
                error: None,
            };
        }
        match state
            .core_runtime()
            .observe_at_user_input_sequence(
                frames,
                cancellation_for_work.clone(),
                batch_user_input_sequence,
            )
            .await
        {
            Ok(observation) => {
                let calls = coosenpai_core::usage::today_observer_usage(&state.paths.usage)
                    .map(|usage| usage.ai_calls)
                    .unwrap_or(0);
                state
                    .publish_watch_view(
                        generation,
                        WatchResult::Observed {
                            observation: observation.clone(),
                            calls,
                        },
                    )
                    .await;
                if tutorial_watch {
                    notify_tutorial_observation(&state);
                }
                let error =
                    process_mailbox_if_possible(&state, cancellation_for_work, "observation")
                        .await
                        .err();
                if error.is_none() {
                    state.refresh_conversation().await;
                }
                WatchWorkCompletion::Observation {
                    observation: Some(Box::new(observation)),
                    error,
                }
            }
            Err(
                coosenpai_core::runtime::RuntimeError::StaleWatchScope
                | coosenpai_core::runtime::RuntimeError::ObservationCancelled,
            ) => {
                state
                    .publish_watch_view(generation, WatchResult::NotDelivered)
                    .await;
                WatchWorkCompletion::Observation {
                    observation: None,
                    error: None,
                }
            }
            Err(error) => WatchWorkCompletion::Observation {
                observation: None,
                error: Some(anyhow::Error::new(error).context("Manager の観察 ACK")),
            },
        }
    });
    debug_assert!(
        started,
        "watch work slot was checked before starting observation work"
    );
    Ok(())
}

enum WatchWorkCompletion {
    Observation {
        observation: Option<Box<ObservationRecord>>,
        error: Option<anyhow::Error>,
    },
    Mailbox(Result<()>),
}

pub(crate) fn trigger_name(trigger: ActivityTriggerKind) -> &'static str {
    match trigger {
        ActivityTriggerKind::TypingPaused => "typing-paused",
        ActivityTriggerKind::AppSwitched => "app-switched",
        ActivityTriggerKind::Timer => "timer",
    }
}
