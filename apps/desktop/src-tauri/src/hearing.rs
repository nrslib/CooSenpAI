use crate::hearing_lifecycle::{
    AttachOutcome, HearingLifecycle, HearingSessionSettings, StopOutcome,
};
use crate::hearing_presenter::HearingResult;
use crate::state::DesktopState;
use coosenpai_core::config::ConfigPaths;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::{
    HearingEvent, HearingPort, HelperResolverPort, RuntimeLogger, SpeechPermissionKind,
    SpeechPermissionPort,
};
use coosenpai_core::state::{AudioObservation, AudioObservationSource, ObservationRecord};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tokio_util::sync::CancellationToken;

const MAX_RESTART_ATTEMPTS: u8 = 3;
const AUDIO_INGESTION_QUEUE_CAPACITY: usize = 128;

fn hearing_context_for_event(
    session_id: &str,
    event: &HearingEvent,
) -> Option<coosenpai_core::hearing_context::HearingContext> {
    let (source, generation, sequence, text, confirmed) = match event {
        HearingEvent::Recognizing {
            source,
            generation,
            sequence,
            text,
        } => (*source, *generation, *sequence, text.clone(), false),
        HearingEvent::Final {
            source,
            generation,
            sequence,
            text,
        } => (*source, *generation, *sequence, text.clone(), true),
        HearingEvent::NoSpeech {
            source,
            generation,
            sequence,
        } => (*source, *generation, *sequence, String::new(), true),
        _ => return None,
    };
    Some(coosenpai_core::hearing_context::HearingContext {
        session_id: session_id.to_owned(),
        source,
        generation,
        sequence,
        text,
        confirmed,
    })
}

async fn next_hearing_context_event(
    session: &mut coosenpai_core::ports::HearingSession,
    session_id: &str,
    runtime: &dyn crate::core_runtime_port::CoreRuntimePort,
    logger: &dyn RuntimeLogger,
    generation: u64,
) -> Option<Result<HearingEvent, coosenpai_core::ports::PortError>> {
    while let Some(event) = session.next_event().await {
        if let Ok(event) = &event {
            if let Some(context) = hearing_context_for_event(session_id, event) {
                match runtime.update_hearing_context(context) {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = logger.write("INFO", &format!(
                            "聴覚観察: 世代または更新順序の古いイベントを破棄しました: error-type=audio-stale-generation generation={generation}"
                        ));
                        continue;
                    }
                    Err(error) => {
                        return Some(Err(coosenpai_core::ports::PortError::Unavailable(format!(
                            "音声文脈を保存できませんでした: {error}"
                        ))))
                    }
                }
            }
        }
        return Some(event);
    }
    None
}

struct AudioIngestionHandle {
    sender: mpsc::Sender<(
        AudioObservationSource,
        String,
        chrono::DateTime<chrono::Utc>,
    )>,
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
    permission_port: Mutex<Arc<dyn SpeechPermissionPort>>,
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
            permission_port: Mutex::new(Arc::new(crate::platform::MacSpeechPermissions)),
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

    // 停止は取消と off 投影で完了とし、helper の終了待ちはバックグラウンドに委ねる。
    pub(crate) async fn cancel(&self, state: &DesktopState) {
        let _projection = self.projection.lock().await;
        let Some(outcome) = self.take_stop().await else {
            return;
        };
        if !outcome.changed {
            return;
        }
        outcome.cancellation.cancel();
        self.complete_stop(outcome.generation).await;
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Cancelled(outcome.generation),
            ))
            .await;
        let logger = state.logger.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(control) = outcome.control {
                if let Err(error) = control.cancel().await {
                    let _ = logger.write(
                        "WARN",
                        &format!(
                            "聴覚観察 helper の停止を確認できませんでした: error-type=audio-cancel error={error}"
                        ),
                    );
                }
            }
            if let Some(initialization_completed) = outcome.initialization_completed {
                let _ = initialization_completed.await;
            }
        });
    }

    async fn reconcile(self: Arc<Self>, state: Arc<DesktopState>) {
        if state.is_shutting_down() {
            return;
        }
        let _projection = self.projection.lock().await;
        let config = state.runtime_config();
        let settings = hearing_session_settings(&config);
        if !config.audio.enabled || !state.is_runtime_active() || state.voice_output.is_active() {
            *self.restart_attempts.lock().await = 0;
            self.stop_locked(&state).await;
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                    HearingResult::Inactive,
                ))
                .await;
            return;
        }
        if settings.sources.is_empty() {
            self.stop_locked(&state).await;
            publish_audio_error(
                &state,
                None,
                "input-source",
                text(
                    TextKey::AudioSourceRequired,
                    Locale::from_config(&state.runtime_config().ui.language),
                ),
            )
            .await;
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
        let publication_started = std::time::Instant::now();
        let _ = state.logger.write(
            "INFO",
            &format!(
                "hearing-start: generation={generation} stage=starting-publication phase=begin"
            ),
        );
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Started(generation),
            ))
            .await;

        let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=starting-publication phase=end elapsed-ms={}", publication_started.elapsed().as_millis()));
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
        let started = std::time::Instant::now();
        let _ = state.logger.write(
            "INFO",
            &format!(
                "hearing-start: generation={generation} stage=recognition-permission phase=begin"
            ),
        );
        let permission_port = self.permission_port.lock().await.clone();
        let recognition = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return,
            result = permission_port.request_recognition(cancellation.child_token()) => result,
        };
        if cancellation.is_cancelled() {
            return;
        }
        let (recognition, permission_error) = match recognition {
            Ok(permission) => (permission, None),
            Err(error) => (SpeechPermissionKind::Unavailable, Some(error.to_string())),
        };
        let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=recognition-permission phase=end elapsed-ms={} result={recognition:?}", started.elapsed().as_millis()));
        if recognition != SpeechPermissionKind::Granted {
            let locale = Locale::from_config(&state.runtime_config().ui.language);
            let key = match recognition {
                SpeechPermissionKind::Denied => TextKey::SpeechRecognitionDenied,
                SpeechPermissionKind::Restricted => TextKey::SpeechRecognitionRestricted,
                _ => TextKey::SpeechRecognitionUnavailable,
            };
            let message = permission_error
                .as_deref()
                .unwrap_or_else(|| text(key, locale));
            publish_audio_failure(
                &self,
                state.clone(),
                generation,
                "permission-speech",
                message,
                settings,
                Some(recognition),
            )
            .await;
            return;
        }
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::PermissionLoaded {
                    generation,
                    recognition,
                },
            ))
            .await;
        if cancellation.is_cancelled() {
            return;
        }
        let mut helper_sources = settings.sources.clone();
        if settings.sources.contains(&AudioObservationSource::Speaker) {
            let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=screen-permission phase=begin elapsed-ms={}", started.elapsed().as_millis()));
            let permission = state.request_screen_permission_for_audio().await;
            let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=screen-permission phase=end elapsed-ms={}", started.elapsed().as_millis()));
            if cancellation.is_cancelled() {
                self.complete_stop(generation).await;
                return;
            }
            let locale = Locale::from_config(&state.runtime_config().ui.language);
            if permission.presentation_for_locale(locale).status != "granted" {
                helper_sources = sources_without_speaker(&settings.sources);
                let message = permission
                    .presentation_for_locale(locale)
                    .message
                    .unwrap_or(text(TextKey::AudioScreenPermissionRequired, locale));
                if helper_sources.is_empty() {
                    publish_audio_failure(
                        &self,
                        state.clone(),
                        generation,
                        "screen-capture",
                        message,
                        settings.clone(),
                        None,
                    )
                    .await;
                    return;
                }
                let _ = state.logger.write(
                    "WARN",
                    &format!("聴覚観察 source-local error: source=speaker {message}"),
                );
            }
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
                None,
            )
            .await;
            return;
        };
        let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=helper-start phase=begin elapsed-ms={}", started.elapsed().as_millis()));
        let session = match port
            .start(
                &settings.locale,
                &settings.input_device,
                helper_sources,
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
                    None,
                )
                .await;
                return;
            }
        };
        let _ = state.logger.write(
            "INFO",
            &format!(
                "hearing-start: generation={generation} stage=helper-start phase=end elapsed-ms={}",
                started.elapsed().as_millis()
            ),
        );
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
                        ingestion_state.paths.clone(),
                        ingestion_state.runtime_config().retention.observation_days,
                        ingestion_cancellation,
                        ingestion_rx,
                        result_tx,
                        ingestion_barrier,
                    )
                    .await;
                });
                let (delivery_tx, delivery_rx) = mpsc::channel(AUDIO_INGESTION_QUEUE_CAPACITY);
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
        let context_session = match state
            .core_runtime()
            .begin_hearing_context(generation, cancellation.clone())
        {
            Ok(session_id) => session_id,
            Err(error) => {
                finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
                self.handle_session_failure(
                    &state,
                    generation,
                    "audio-context",
                    &error.to_string(),
                    settings,
                )
                .await;
                return;
            }
        };
        while let Some(event) = next_hearing_context_event(
            &mut session,
            &context_session,
            state.core_runtime(),
            state.logger.as_ref(),
            generation,
        )
        .await
        {
            match event {
                Ok(HearingEvent::Ready {
                    microphone,
                    recognition,
                    ..
                }) => {
                    if self.accepts_events(generation).await {
                        self.reset_restart_attempts().await;
                        state
                            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                                HearingResult::Ready {
                                    generation,
                                    microphone,
                                    recognition,
                                },
                            ))
                            .await;
                    }
                }
                Ok(HearingEvent::Recognizing {
                    source, sequence, ..
                }) => {
                    if sequence == 1 {
                        self.publish_recognition_event(
                            &state,
                            generation,
                            source,
                            crate::snapshot::AudioLogStage::Recognizing,
                        )
                        .await;
                    }
                }
                Ok(HearingEvent::NoSpeech { source, .. }) => {
                    self.publish_recognition_event(
                        &state,
                        generation,
                        source,
                        crate::snapshot::AudioLogStage::NoSpeech,
                    )
                    .await;
                }
                Ok(HearingEvent::Final { source, text, .. }) => {
                    if !self.accepts_events(generation).await || cancellation.is_cancelled() {
                        let _ = state.logger.write(
                            "INFO",
                            &format!(
                                "聴覚観察: 世代の古い確定イベントを破棄しました: error-type=audio-stale-generation generation={generation}"
                            ),
                        );
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
                            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                                HearingResult::Warning {
                                    generation,
                                    kind,
                                    message,
                                },
                            ))
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

    async fn publish_recognition_event(
        &self,
        state: &DesktopState,
        generation: u64,
        source: AudioObservationSource,
        stage: crate::snapshot::AudioLogStage,
    ) {
        let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if !self.accepts_events(generation).await {
            return;
        }
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Recognition {
                    generation,
                    created_at,
                    source,
                    stage,
                },
            ))
            .await;
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
        delivery_tx: &mpsc::Sender<AudioObservation>,
    ) {
        if let Ok(coosenpai_core::state::ObservationRecord::Audio(observation)) = &result {
            if self.accepts_events(generation).await && !cancellation.is_cancelled() {
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                        HearingResult::Observed {
                            generation,
                            observation: observation.clone(),
                        },
                    ))
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
        let Ok(ObservationRecord::Audio(observation)) = result else {
            return;
        };
        let delivered = tokio::select! {
            result = delivery_tx.send(observation) => result.is_ok(),
            _ = cancellation.cancelled() => return,
        };
        if !delivered {
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
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Stopping(outcome.generation),
            ))
            .await;
        if let Some(control) = outcome.control {
            let _ = control.cancel().await;
        }
        if let Some(initialization_completed) = outcome.initialization_completed {
            let _ = initialization_completed.await;
        }
        self.complete_stop(outcome.generation).await;
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Stopped(outcome.generation),
            ))
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
                    if state.is_shutting_down() || !state.is_runtime_active() || state.voice_output.is_active() {
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
    paths: ConfigPaths,
    retention_days: u64,
    cancellation: CancellationToken,
    mut requests: mpsc::Receiver<(
        AudioObservationSource,
        String,
        chrono::DateTime<chrono::Utc>,
    )>,
    results: mpsc::Sender<
        Result<coosenpai_core::state::ObservationRecord, coosenpai_core::runtime::RuntimeError>,
    >,
    barrier: Option<AudioIngestionBarrier>,
) {
    let mut barrier = barrier;
    while let Some((source, text, confirmed_at)) = requests.recv().await {
        if cancellation.is_cancelled() {
            return;
        }
        if let Some(barrier) = barrier.take() {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
        let paths = paths.clone();
        let recording_cancellation = cancellation.clone();
        let result = tokio::task::spawn_blocking(move || {
            if recording_cancellation.is_cancelled() {
                return Err(coosenpai_core::runtime::RuntimeError::Closed);
            }
            let observation = AudioObservation::from_confirmed_text(source, &text, confirmed_at)
                .map_err(|_| coosenpai_core::observer::ObserverError::Output)?;
            coosenpai_core::observer::record_audio_observation(
                &paths,
                retention_days,
                &observation,
                chrono::Utc::now(),
            )
            .map_err(coosenpai_core::observer::ObserverError::from)?;
            Ok(ObservationRecord::Audio(observation))
        })
        .await
        .unwrap_or_else(|error| {
            Err(coosenpai_core::observer::ObserverError::Persistence(
                coosenpai_core::persistence::PersistenceError::Invalid(format!(
                    "音声の保存タスクが停止しました: {error}"
                )),
            )
            .into())
        });
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
    ingestion_tx: &mpsc::Sender<(
        AudioObservationSource,
        String,
        chrono::DateTime<chrono::Utc>,
    )>,
    source: AudioObservationSource,
    text: String,
    cancellation: &CancellationToken,
) -> bool {
    let confirmed_at = chrono::Utc::now();
    tokio::select! {
        result = ingestion_tx.send((source, text, confirmed_at)) => result.is_ok(),
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
    delivery_tx: mpsc::Sender<AudioObservation>,
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
    ingestion_tx: &mut Option<
        mpsc::Sender<(
            AudioObservationSource,
            String,
            chrono::DateTime<chrono::Utc>,
        )>,
    >,
    ingestion_done: &mut Option<oneshot::Receiver<()>>,
) {
    ingestion_tx.take();
    if let Some(done) = ingestion_done.take() {
        let _ = done.await;
    }
}

async fn run_delivery_worker(
    state: Arc<DesktopState>,
    cancellation: CancellationToken,
    mut requests: mpsc::Receiver<AudioObservation>,
) {
    while let Some(observation) = requests.recv().await {
        if cancellation.is_cancelled() {
            return;
        }
        if state
            .core_runtime()
            .audio_observation(observation, cancellation.child_token())
            .await
            .is_err()
        {
            let _ = state.logger.write(
                "WARN",
                "保存済みの音声観察を AI へ渡せませんでした: error-type=audio-delivery",
            );
            continue;
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

fn sources_without_speaker(sources: &[AudioObservationSource]) -> Vec<AudioObservationSource> {
    sources
        .iter()
        .copied()
        .filter(|source| *source != AudioObservationSource::Speaker)
        .collect()
}

pub(crate) fn selected_sources(
    config: &coosenpai_core::config::Config,
) -> Vec<AudioObservationSource> {
    let mut sources = Vec::with_capacity(2);
    if config.audio.mic {
        sources.push(AudioObservationSource::Microphone);
    }
    if config.audio.speaker {
        sources.push(AudioObservationSource::Speaker);
    }
    sources
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
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
            HearingResult::Restarted(generation),
        ))
        .await;
}

async fn publish_audio_failure(
    controller: &Arc<HearingController>,
    state: Arc<DesktopState>,
    generation: u64,
    kind: &str,
    message: &str,
    settings: HearingSessionSettings,
    recognition: Option<SpeechPermissionKind>,
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
        publish_audio_error_with_permission(&state, Some(generation), kind, message, recognition)
            .await;
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
    publish_audio_error_with_permission(state, generation, kind, message, None).await;
}

async fn publish_audio_error_with_permission(
    state: &DesktopState,
    generation: Option<u64>,
    kind: &str,
    message: &str,
    recognition: Option<SpeechPermissionKind>,
) {
    let _ = state.logger.write("WARN", &format!("聴覚観察: {message}"));
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
            HearingResult::Failed {
                generation,
                kind: kind.to_owned(),
                message: message.to_owned(),
                recognition,
            },
        ))
        .await;
}

