use crate::hearing_lifecycle::{
    AttachOutcome, HearingLifecycle, HearingSessionSettings, StopOutcome,
};
use crate::state::DesktopState;
use coosenpai_core::config::ConfigPaths;
use coosenpai_core::ports::{HearingEvent, HearingPort, HelperResolverPort, RuntimeLogger};
use coosenpai_core::state::AudioObservationSource;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tokio_util::sync::CancellationToken;

const MAX_RESTART_ATTEMPTS: u8 = 3;
const AUDIO_INGESTION_QUEUE_CAPACITY: usize = 128;

struct AudioIngestionHandle {
    sender: mpsc::Sender<(AudioObservationSource, String)>,
    done: oneshot::Receiver<()>,
    terminal_barrier: Option<AudioTerminalBarrier>,
}

struct AudioIngestionBarrier {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

struct AudioTerminalBarrier {
    received: Arc<Notify>,
    release: Arc<Notify>,
}

struct InitializationCompletion(Option<oneshot::Sender<()>>);

impl Drop for InitializationCompletion {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

pub(crate) struct HearingController {
    hearing_port: Mutex<Option<Arc<dyn HearingPort>>>,
    ingestion_barrier: Mutex<Option<AudioIngestionBarrier>>,
    terminal_barrier: Mutex<Option<AudioTerminalBarrier>>,
    lifecycle: Mutex<HearingLifecycle>,
    projection: Mutex<()>,
    restart_attempts: Mutex<u8>,
}

impl HearingController {
    pub(crate) fn new(paths: &ConfigPaths, logger: Arc<dyn RuntimeLogger>) -> Self {
        let executable_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(ToOwned::to_owned));
        let helper = executable_dir.as_deref().and_then(|directory| {
            crate::platform::MacHelperResolver.resolve_hearing_helper(directory, &paths.root)
        });
        Self {
            hearing_port: Mutex::new(
                helper.map(|helper| crate::platform::hearing_port(helper, logger.clone())),
            ),
            ingestion_barrier: Mutex::new(None),
            terminal_barrier: Mutex::new(None),
            lifecycle: Mutex::new(HearingLifecycle::default()),
            projection: Mutex::new(()),
            restart_attempts: Mutex::new(0),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub async fn install_audio_ingestion_barrier_for_test(
        &self,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    ) {
        *self.ingestion_barrier.lock().await = Some(AudioIngestionBarrier { entered, release });
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub async fn install_audio_terminal_barrier_for_test(
        &self,
        received: Arc<Notify>,
        release: Arc<Notify>,
    ) {
        *self.terminal_barrier.lock().await = Some(AudioTerminalBarrier { received, release });
    }

    pub(crate) fn sync(self: &Arc<Self>, state: Arc<DesktopState>) {
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            controller.reconcile(state).await;
        });
    }

    pub(crate) async fn cancel_and_wait(&self, state: &DesktopState) {
        let _projection = self.projection.lock().await;
        let outcome = self.take_stop().await;
        self.finish_stop(state, outcome).await;
    }

    async fn reconcile(self: Arc<Self>, state: Arc<DesktopState>) {
        if state.is_shutting_down() {
            return;
        }
        let _projection = self.projection.lock().await;
        let config = state.runtime_config();
        let settings = hearing_session_settings(&config);
        if !config.audio.enabled || !state.is_runtime_active() {
            *self.restart_attempts.lock().await = 0;
            self.stop_locked(&state).await;
            state
                .publish(|snapshot| {
                    snapshot.audio.phase = "off".to_owned();
                    snapshot.audio.warning_kind = None;
                    snapshot.audio.message = None;
                })
                .await;
            return;
        }
        if settings.sources.is_empty() {
            self.stop_locked(&state).await;
            publish_audio_error(&state, None, "input-source", "スピーカーを選択してください").await;
            return;
        }
        if self.lifecycle.lock().await.same_settings(&settings) {
            return;
        }
        self.stop_locked(&state).await;
        self.start_locked(state, settings).await;
    }

    async fn start_locked(
        self: &Arc<Self>,
        state: Arc<DesktopState>,
        settings: HearingSessionSettings,
    ) {
        let cancellation = CancellationToken::new();
        let (initialization_completed, initialization_finished) = oneshot::channel();
        let generation = {
            let mut lifecycle = self.lifecycle.lock().await;
            lifecycle.start(
                cancellation.clone(),
                settings.clone(),
                initialization_finished,
            )
        };
        let Some(generation) = generation else { return };
        state
            .publish(|snapshot| {
                snapshot.audio.generation = generation;
                snapshot.audio.phase = "starting".to_owned();
                snapshot.audio.warning_kind = None;
                snapshot.audio.message = None;
            })
            .await;

        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            controller
                .initialize_session(
                    state,
                    generation,
                    cancellation,
                    settings,
                    initialization_completed,
                )
                .await;
        });
    }

    async fn initialize_session(
        self: Arc<Self>,
        state: Arc<DesktopState>,
        generation: u64,
        cancellation: CancellationToken,
        settings: HearingSessionSettings,
        initialization_completed: oneshot::Sender<()>,
    ) {
        let _initialization_completion = InitializationCompletion(Some(initialization_completed));
        let permission = state.request_screen_permission_for_audio().await;
        if cancellation.is_cancelled() {
            self.complete_stop(generation).await;
            return;
        }
        if permission.presentation().status != "granted" {
            let message = permission
                .presentation()
                .message
                .unwrap_or("スピーカーの音を聞くには画面収録の許可が必要です");
            publish_audio_failure(
                &self,
                state.clone(),
                generation,
                "screen-capture",
                message,
                settings.clone(),
            )
            .await;
            return;
        }
        if cancellation.is_cancelled() {
            self.complete_stop(generation).await;
            return;
        }
        let Some(port) = self.hearing_port.lock().await.clone() else {
            publish_audio_failure(
                &self,
                state.clone(),
                generation,
                "helper-unavailable",
                "coosenpai-hearing が見つかりません",
                settings.clone(),
            )
            .await;
            return;
        };
        let session = match port
            .start(
                &settings.locale,
                &settings.input_device,
                settings.sources.clone(),
                settings.debug_dump_dir.as_deref(),
                cancellation.clone(),
            )
            .await
        {
            Ok(session) => session,
            Err(_error) if cancellation.is_cancelled() => {
                self.complete_stop(generation).await;
                return;
            }
            Err(error) => {
                publish_audio_failure(
                    &self,
                    state.clone(),
                    generation,
                    "helper-start",
                    &error.to_string(),
                    settings.clone(),
                )
                .await;
                return;
            }
        };
        let control = session.control();
        let attach_outcome = {
            let mut lifecycle = self.lifecycle.lock().await;
            lifecycle.attach_session(generation, cancellation.clone(), control.clone())
        };
        match attach_outcome {
            AttachOutcome::Listening => {
                publish_audio_listening(&state, generation).await;
                let (ingestion_tx, ingestion_rx) = mpsc::channel(AUDIO_INGESTION_QUEUE_CAPACITY);
                let (result_tx, result_rx) = mpsc::channel(AUDIO_INGESTION_QUEUE_CAPACITY);
                let (ingestion_done_tx, ingestion_done_rx) = oneshot::channel();
                let ingestion_barrier = self.ingestion_barrier.lock().await.take();
                let terminal_barrier = self.terminal_barrier.lock().await.take();
                let ingestion_state = state.clone();
                let ingestion_cancellation = cancellation.clone();
                tauri::async_runtime::spawn(async move {
                    run_audio_ingestion(
                        ingestion_state.core_runtime(),
                        ingestion_cancellation,
                        ingestion_rx,
                        result_tx,
                        ingestion_barrier,
                    )
                    .await;
                });
                let (delivery_tx, delivery_rx) = mpsc::channel(1);
                let delivery_state = state.clone();
                let delivery_cancellation = cancellation.clone();
                tauri::async_runtime::spawn(async move {
                    run_delivery_worker(delivery_state, delivery_cancellation, delivery_rx).await;
                });
                let result_controller = self.clone();
                let restart_settings = settings.clone();
                let result_state = state.clone();
                let result_cancellation = cancellation.clone();
                let result_delivery = delivery_tx.clone();
                tauri::async_runtime::spawn(async move {
                    run_audio_result_worker(
                        result_controller,
                        result_state,
                        generation,
                        result_cancellation,
                        result_rx,
                        result_delivery,
                        ingestion_done_tx,
                    )
                    .await;
                });
                let session_controller = self.clone();
                tauri::async_runtime::spawn(async move {
                    session_controller
                        .run_session(
                            state,
                            generation,
                            cancellation,
                            session,
                            restart_settings,
                            AudioIngestionHandle {
                                sender: ingestion_tx,
                                done: ingestion_done_rx,
                                terminal_barrier,
                            },
                        )
                        .await;
                });
            }
            AttachOutcome::Cancel(control) => {
                cancellation.cancel();
                let _ = control.cancel().await;
                self.complete_stop(generation).await;
            }
        }
    }

    async fn run_session(
        self: Arc<Self>,
        state: Arc<DesktopState>,
        generation: u64,
        cancellation: CancellationToken,
        mut session: coosenpai_core::ports::HearingSession,
        settings: HearingSessionSettings,
        ingestion: AudioIngestionHandle,
    ) {
        let AudioIngestionHandle {
            sender: ingestion_sender,
            done: ingestion_completion,
            terminal_barrier,
        } = ingestion;
        let mut ingestion_tx = Some(ingestion_sender);
        let mut ingestion_done = Some(ingestion_completion);
        let mut terminal_barrier = terminal_barrier;
        while let Some(event) = session.next_event().await {
            match event {
                Ok(HearingEvent::Ready {
                    microphone,
                    recognition,
                    ..
                }) => {
                    if self.accepts_events(generation).await {
                        self.reset_restart_attempts().await;
                        state
                            .publish(|snapshot| {
                                snapshot.audio.phase = "listening".to_owned();
                                snapshot.audio.microphone_permission =
                                    crate::speech::permission_name(microphone);
                                snapshot.audio.recognition_permission =
                                    crate::speech::permission_name(recognition);
                            })
                            .await;
                    }
                }
                Ok(HearingEvent::Final { source, text }) => {
                    if !self.accepts_events(generation).await || cancellation.is_cancelled() {
                        continue;
                    }
                    let Some(ingestion_sender) = ingestion_tx.as_ref() else {
                        return;
                    };
                    if !queue_audio_final(ingestion_sender, source, text, &cancellation).await {
                        if !cancellation.is_cancelled() {
                            let _ = state.logger.write(
                                "WARN",
                                "確定した音声観察の取り込み worker が停止しています: error-type=audio-ingestion",
                            );
                        }
                        finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
                        return;
                    }
                }
                Ok(HearingEvent::Warning { kind, message }) => {
                    if self.accepts_events(generation).await {
                        state
                            .publish(|snapshot| {
                                snapshot.audio.warning_kind = Some(kind);
                                snapshot.audio.message = Some(message);
                            })
                            .await;
                    }
                }
                Ok(HearingEvent::Error { kind, message }) => {
                    if is_non_fatal_audio_source_error(&kind) {
                        if self.accepts_events(generation).await && !cancellation.is_cancelled() {
                            let _ = state
                                .logger
                                .write("WARN", &format!("聴覚観察 source-local error: {message}"));
                        }
                        continue;
                    }
                    finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
                    if !cancellation.is_cancelled() {
                        self.handle_session_failure(
                            &state,
                            generation,
                            &kind,
                            &message,
                            settings.clone(),
                        )
                        .await;
                    }
                    return;
                }
                Ok(HearingEvent::Closed) => {
                    if let Some(barrier) = terminal_barrier.take() {
                        barrier.received.notify_one();
                        barrier.release.notified().await;
                    }
                    finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
                    if cancellation.is_cancelled() {
                        self.complete_stop(generation).await;
                    } else {
                        self.handle_session_failure(
                            &state,
                            generation,
                            "helper-closed",
                            "聴覚観察 helper が予期せず終了しました",
                            settings.clone(),
                        )
                        .await;
                    }
                    return;
                }
                Err(error) => {
                    finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
                    if !cancellation.is_cancelled() {
                        self.handle_session_failure(
                            &state,
                            generation,
                            "helper",
                            &error.to_string(),
                            settings.clone(),
                        )
                        .await;
                    }
                    return;
                }
            }
        }
        finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
        if !cancellation.is_cancelled() {
            self.handle_session_failure(
                &state,
                generation,
                "helper-closed",
                "聴覚観察 helper が予期せず終了しました",
                settings,
            )
            .await;
        }
    }

    async fn handle_audio_result(
        &self,
        state: &Arc<DesktopState>,
        generation: u64,
        result: Result<
            coosenpai_core::state::ObservationRecord,
            coosenpai_core::runtime::RuntimeError,
        >,
        cancellation: &CancellationToken,
        delivery_tx: &mpsc::Sender<()>,
    ) {
        if let Ok(coosenpai_core::state::ObservationRecord::Audio(observation)) = &result {
            if self.accepts_events(generation).await && !cancellation.is_cancelled() {
                state
                    .publish(|snapshot| {
                        if snapshot.audio.generation == generation {
                            snapshot.audio.latest_observation =
                                Some(crate::snapshot::AudioObservationView::from_observation(
                                    observation,
                                ));
                        }
                    })
                    .await;
            }
        }
        if result.is_err() && self.accepts_events(generation).await && !cancellation.is_cancelled()
        {
            publish_audio_error(
                state,
                Some(generation),
                "observation",
                "確定した音声を観察として保存できませんでした",
            )
            .await;
            return;
        }
        if cancellation.is_cancelled() || !self.accepts_events(generation).await {
            return;
        }
        if delivery_tx
            .try_send(())
            .is_err_and(|error| !matches!(error, mpsc::error::TrySendError::Full(())))
        {
            let _ = state.logger.write(
                "WARN",
                "確定した音声観察の配達 worker が停止しています: error-type=audio-delivery",
            );
        }
    }

    async fn reset_restart_attempts(&self) {
        *self.restart_attempts.lock().await = 0;
    }

    async fn accepts_events(&self, generation: u64) -> bool {
        self.lifecycle.lock().await.accepts_events(generation)
    }

    async fn stop_locked(&self, state: &DesktopState) {
        let outcome = self.take_stop().await;
        self.finish_stop(state, outcome).await;
    }

    async fn take_stop(&self) -> Option<StopOutcome> {
        self.lifecycle.lock().await.stop()
    }

    async fn finish_stop(&self, state: &DesktopState, outcome: Option<StopOutcome>) {
        let Some(outcome) = outcome else { return };
        if !outcome.changed {
            return;
        }
        outcome.cancellation.cancel();
        state
            .publish(|snapshot| {
                if snapshot.audio.generation == outcome.generation {
                    snapshot.audio.phase = "stopping".to_owned();
                }
            })
            .await;
        if let Some(control) = outcome.control {
            let _ = control.cancel().await;
        }
        if let Some(initialization_completed) = outcome.initialization_completed {
            let _ = initialization_completed.await;
        }
        self.complete_stop(outcome.generation).await;
        state
            .publish(|snapshot| {
                if snapshot.audio.generation == outcome.generation
                    && snapshot.audio.phase == "stopping"
                {
                    snapshot.audio.phase = "off".to_owned();
                    snapshot.audio.warning_kind = None;
                    snapshot.audio.message = None;
                }
            })
            .await;
    }

    async fn complete_stop(&self, generation: u64) {
        self.lifecycle.lock().await.complete_stop(generation);
    }

    async fn take_failure(&self, generation: u64) -> Option<StopOutcome> {
        let outcome = self.lifecycle.lock().await.fail(generation);
        outcome
    }

    async fn handle_session_failure(
        self: &Arc<Self>,
        state: &Arc<DesktopState>,
        generation: u64,
        kind: &str,
        message: &str,
        settings: HearingSessionSettings,
    ) {
        let Some(cleanup) = self.take_failure(generation).await else {
            return;
        };
        cleanup.cancellation.cancel();
        if let Some(control) = cleanup.control {
            let _ = control.cancel().await;
        }
        publish_audio_error(state, Some(generation), kind, message).await;
        if retryable_audio_error(kind) {
            self.schedule_restart(state.clone(), settings).await;
        } else {
            let _ = state.logger.write(
                "WARN",
                &format!("聴覚観察の自動再起動を行いません: error-type={kind}"),
            );
        }
    }

    pub(crate) async fn schedule_restart(
        self: &Arc<Self>,
        state: Arc<DesktopState>,
        settings: HearingSessionSettings,
    ) {
        let Some(attempt) = self.next_restart_attempt().await else {
            let _ = state.logger.write(
                "WARN",
                "聴覚観察 helper の自動再起動上限に達したため phase=error で停止します: error-type=restart-limit",
            );
            return;
        };
        let delay = restart_delay(attempt);
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::select! {
                _ = state.cancellation.cancelled() => {}
                _ = tokio::time::sleep(delay) => {
                    if state.is_shutting_down() || !state.is_runtime_active() {
                        return;
                    }
                    let config = state.runtime_config();
                    let current = hearing_session_settings(&config);
                    if !config.audio.enabled || current != settings {
                        return;
                    }
                    controller.sync(state);
                }
            }
        });
    }

    async fn next_restart_attempt(&self) -> Option<u8> {
        let mut attempts = self.restart_attempts.lock().await;
        if *attempts >= MAX_RESTART_ATTEMPTS {
            return None;
        }
        *attempts += 1;
        Some(*attempts)
    }
}

async fn run_audio_ingestion(
    runtime: &dyn crate::core_runtime_port::CoreRuntimePort,
    cancellation: CancellationToken,
    mut requests: mpsc::Receiver<(AudioObservationSource, String)>,
    results: mpsc::Sender<
        Result<coosenpai_core::state::ObservationRecord, coosenpai_core::runtime::RuntimeError>,
    >,
    barrier: Option<AudioIngestionBarrier>,
) {
    let mut barrier = barrier;
    while let Some((source, text)) = requests.recv().await {
        if cancellation.is_cancelled() {
            return;
        }
        if let Some(barrier) = barrier.take() {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
        let result =
            enqueue_audio_observation(runtime, source, text, cancellation.child_token()).await;
        tokio::select! {
            result = results.send(result) => {
                if result.is_err() {
                    return;
                }
            }
            _ = cancellation.cancelled() => return,
        }
    }
}

async fn queue_audio_final(
    ingestion_tx: &mpsc::Sender<(AudioObservationSource, String)>,
    source: AudioObservationSource,
    text: String,
    cancellation: &CancellationToken,
) -> bool {
    tokio::select! {
        result = ingestion_tx.send((source, text)) => result.is_ok(),
        _ = cancellation.cancelled() => false,
    }
}

async fn run_audio_result_worker(
    controller: Arc<HearingController>,
    state: Arc<DesktopState>,
    generation: u64,
    cancellation: CancellationToken,
    mut results: mpsc::Receiver<
        Result<coosenpai_core::state::ObservationRecord, coosenpai_core::runtime::RuntimeError>,
    >,
    delivery_tx: mpsc::Sender<()>,
    ingestion_done: oneshot::Sender<()>,
) {
    while let Some(result) = results.recv().await {
        controller
            .handle_audio_result(&state, generation, result, &cancellation, &delivery_tx)
            .await;
    }
    let _ = ingestion_done.send(());
}

async fn finish_audio_ingestion(
    ingestion_tx: &mut Option<mpsc::Sender<(AudioObservationSource, String)>>,
    ingestion_done: &mut Option<oneshot::Receiver<()>>,
) {
    ingestion_tx.take();
    if let Some(done) = ingestion_done.take() {
        let _ = done.await;
    }
}

async fn enqueue_audio_observation(
    runtime: &dyn crate::core_runtime_port::CoreRuntimePort,
    source: AudioObservationSource,
    text: String,
    cancellation: CancellationToken,
) -> Result<coosenpai_core::state::ObservationRecord, coosenpai_core::runtime::RuntimeError> {
    runtime.audio_observation(source, text, cancellation).await
}

async fn run_delivery_worker(
    state: Arc<DesktopState>,
    cancellation: CancellationToken,
    mut requests: mpsc::Receiver<()>,
) {
    while requests.recv().await.is_some() {
        if cancellation.is_cancelled() {
            return;
        }
        if state
            .core_runtime()
            .process_mailbox(cancellation.child_token())
            .await
            .is_err()
        {
            let _ = state.logger.write(
                "WARN",
                "確定した音声観察の配達を次回へ延期しました: error-type=audio-delivery",
            );
        }
    }
}

fn retryable_audio_error(kind: &str) -> bool {
    matches!(
        kind,
        "recognition" | "helper" | "helper-closed" | "helper-start"
    )
}

fn is_non_fatal_audio_source_error(kind: &str) -> bool {
    matches!(
        kind,
        "audio-buffer-copy"
            | "audio-conversion"
            | "audio-format"
            | "audio-input-failure"
            | "audio-microphone"
            | "debug-dump-source"
            | "debug-input"
            | "input-device"
            | "permission-microphone"
            | "screen-capture"
            | "audio-pending-overflow"
    ) || kind.starts_with("recognition-")
}

fn restart_delay(attempt: u8) -> std::time::Duration {
    std::time::Duration::from_millis(match attempt {
        1 => 250,
        2 => 1_000,
        _ => 4_000,
    })
}

pub(crate) fn selected_sources(
    config: &coosenpai_core::config::Config,
) -> Vec<AudioObservationSource> {
    if config.audio.speaker {
        vec![AudioObservationSource::Speaker]
    } else {
        Vec::new()
    }
}

pub(crate) fn hearing_session_settings(
    config: &coosenpai_core::config::Config,
) -> HearingSessionSettings {
    HearingSessionSettings::new(
        config.speech.locale.clone(),
        config.speech.input_device.clone(),
        selected_sources(config),
    )
    .with_debug_dump_dir(config.audio.debug_dump_dir.clone())
}

async fn publish_audio_listening(state: &DesktopState, generation: u64) {
    state
        .publish(|snapshot| {
            if snapshot.audio.generation == generation {
                snapshot.audio.phase = "listening".to_owned();
            }
        })
        .await;
}

async fn publish_audio_failure(
    controller: &Arc<HearingController>,
    state: Arc<DesktopState>,
    generation: u64,
    kind: &str,
    message: &str,
    settings: HearingSessionSettings,
) {
    let Some(cleanup) = controller.take_failure(generation).await else {
        return;
    };
    let cancelled = cleanup.cancellation.is_cancelled();
    cleanup.cancellation.cancel();
    if let Some(control) = cleanup.control {
        let _ = control.cancel().await;
    }
    if !cancelled {
        publish_audio_error(&state, Some(generation), kind, message).await;
        if retryable_audio_error(kind) {
            controller.schedule_restart(state.clone(), settings).await;
        }
    }
}

async fn publish_audio_error(
    state: &DesktopState,
    generation: Option<u64>,
    kind: &str,
    message: &str,
) {
    let _ = state.logger.write("WARN", &format!("聴覚観察: {message}"));
    state
        .publish(|snapshot| {
            if !snapshot.config.audio.enabled {
                return;
            }
            if generation.is_some_and(|expected| snapshot.audio.generation != expected) {
                return;
            }
            snapshot.audio.phase = "error".to_owned();
            snapshot.audio.warning_kind = Some(kind.to_owned());
            snapshot.audio.message = Some(message.to_owned());
            if kind == "permission-microphone" {
                snapshot.audio.microphone_permission = "denied".to_owned();
            } else if kind == "permission-speech" {
                snapshot.audio.recognition_permission = "denied".to_owned();
            }
        })
        .await;
}

