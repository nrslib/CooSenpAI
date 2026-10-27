use crate::hearing_lifecycle::{
    AttachOutcome, HearingLifecycle, HearingSessionSettings, StopOutcome,
};
use crate::hearing_presenter::{HearingResult, StopFailureKind};
use crate::snapshot::SpeakerCapture;
use crate::state::DesktopState;
use coosenpai_core::config::ConfigPaths;
use coosenpai_core::hearing_ingestion::{HearingAudioIngestion, HearingAudioRecord};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::{
    HearingEvent, HearingPort, HearingSpeakerCorrection, HearingStartOptions, HelperResolverPort,
    RuntimeLogger, SpeakerIdentificationPreparationStatus, SpeechPermissionKind,
    SpeechPermissionPort,
};
use coosenpai_core::state::{AudioObservation, AudioObservationSource, ObservationRecord};
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tokio_util::sync::CancellationToken;

const MAX_RESTART_ATTEMPTS: u8 = 3;
const AUDIO_INGESTION_QUEUE_CAPACITY: usize = 128;
const HEARING_STOP_LIMIT: std::time::Duration = std::time::Duration::from_secs(4);
const HEARING_FORCE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

fn stop_failure_kind(error: &coosenpai_core::ports::PortError) -> StopFailureKind {
    use coosenpai_core::ports::PortError;
    match error {
        PortError::HearingStopTimeout => StopFailureKind::TimedOut,
        PortError::HearingStopCleanupFailed => StopFailureKind::CleanupFailed,
        PortError::HearingStopUnconfirmed => StopFailureKind::Unconfirmed,
        _ => StopFailureKind::Other,
    }
}

struct StopSessionResult {
    helper: Result<(), coosenpai_core::ports::PortError>,
    initialization_timed_out: bool,
    ingestion_timed_out: bool,
}

impl StopSessionResult {
    fn warning(&self, generation: u64) -> Option<HearingResult> {
        let helper_failure = self.helper.as_ref().err().map(stop_failure_kind);
        (helper_failure.is_some() || self.initialization_timed_out || self.ingestion_timed_out)
            .then_some(HearingResult::StopWarning {
                generation,
                helper_failure,
                initialization_timed_out: self.initialization_timed_out,
                ingestion_timed_out: self.ingestion_timed_out,
            })
    }
}

async fn output_devices_event(
    port: Option<Arc<dyn HearingPort>>,
    monitor_failed: bool,
) -> crate::snapshot_presenter::SnapshotEvent {
    let result = match port {
        Some(port) => port
            .output_devices()
            .await
            .map_err(|_| "出力デバイス一覧を取得できません".to_owned()),
        None => Err("出力デバイス一覧を取得できません".to_owned()),
    };
    crate::snapshot_presenter::SnapshotEvent::OutputDevicesLoaded {
        result,
        monitor_failed,
    }
}

enum AudioIngestionRequest {
    Final {
        generation: u64,
        observation: Box<AudioObservation>,
        corrections: Vec<HearingSpeakerCorrection>,
    },
}

struct AudioIngestionResult {
    generation: u64,
    result: Result<HearingAudioRecord, coosenpai_core::runtime::RuntimeError>,
}

fn hearing_context_for_event(
    session_id: &str,
    event: &HearingEvent,
) -> Option<coosenpai_core::hearing_context::HearingContext> {
    let (source, generation, sequence, text, confirmed, speaker) = match event {
        HearingEvent::Recognizing {
            source,
            generation,
            sequence,
            text,
        } => (*source, *generation, *sequence, text.clone(), false, None),
        HearingEvent::Final {
            source,
            generation,
            sequence,
            text,
            speaker,
            ..
        } => (
            *source,
            *generation,
            *sequence,
            text.clone(),
            true,
            speaker.as_ref().map(|metadata| (**metadata).clone()),
        ),
        HearingEvent::NoSpeech {
            source,
            generation,
            sequence,
        } => (*source, *generation, *sequence, String::new(), true, None),
        _ => return None,
    };
    Some(coosenpai_core::hearing_context::HearingContext {
        session_id: session_id.to_owned(),
        source,
        generation,
        sequence,
        text,
        confirmed,
        speaker,
    })
}

fn log_stale_hearing_event(logger: &dyn RuntimeLogger, generation: u64) {
    let _ = logger.write("INFO", &format!(
        "聴覚観察: 世代または更新順序の古いイベントを破棄しました: error-type=audio-stale-generation generation={generation}"
    ));
}

async fn next_hearing_context_event<F, Fut>(
    session: &mut coosenpai_core::ports::HearingSession,
    session_id: &str,
    runtime: &dyn crate::core_runtime_port::CoreRuntimePort,
    logger: &dyn RuntimeLogger,
    generation: u64,
    accept_final: F,
) -> Option<Result<HearingEvent, coosenpai_core::ports::PortError>>
where
    F: Fn(AudioObservationSource, u64) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    while let Some(event) = session.next_event().await {
        if let Ok(event) = &event {
            if let HearingEvent::Final {
                source,
                input_generation,
                ..
            } = event
            {
                if !accept_final(*source, *input_generation).await {
                    log_stale_hearing_event(logger, generation);
                    continue;
                }
            }
            if let Some(context) = hearing_context_for_event(session_id, event) {
                let mut result = runtime.update_hearing_context(context.clone());
                if matches!(result, Ok(false))
                    && matches!(
                        event,
                        HearingEvent::Final {
                            source: AudioObservationSource::Speaker,
                            ..
                        }
                    )
                {
                    result = runtime.accept_late_speaker_final(context);
                }
                match result {
                    Ok(true) => {}
                    Ok(false) => {
                        log_stale_hearing_event(logger, generation);
                        continue;
                    }
                    Err(coosenpai_core::runtime::RuntimeError::Companion(
                        coosenpai_core::companion::CompanionError::AudioPendingOverflow,
                    )) => {
                        return Some(Ok(HearingEvent::Warning {
                            kind: "audio-pending-overflow".to_owned(),
                            message: "保存待ちの音声が上限に達したため、新しい確定発話を受け付けられません".to_owned(),
                        }));
                    }
                    Err(error) => {
                        return Some(Err(coosenpai_core::ports::PortError::Unavailable(format!(
                            "音声文脈を保存できませんでした: {error}"
                        ))));
                    }
                }
            }
        }
        return Some(event);
    }
    None
}

struct AudioIngestionHandle {
    session_id: String,
    scope: HearingAudioIngestion,
    sender: mpsc::Sender<AudioIngestionRequest>,
    done: oneshot::Receiver<()>,
    terminal_barrier: Option<AudioTerminalBarrier>,
}

struct ActiveHearingSession {
    session: coosenpai_core::ports::HearingSession,
    sources: Vec<AudioObservationSource>,
    stop_events: oneshot::Receiver<()>,
}

struct SourceErrorTracker {
    sources: Vec<AudioObservationSource>,
    terminal_sources: Vec<AudioObservationSource>,
    pending_source_error: bool,
}

impl SourceErrorTracker {
    fn new(sources: Vec<AudioObservationSource>) -> Self {
        Self {
            sources,
            terminal_sources: Vec::new(),
            pending_source_error: false,
        }
    }

    fn observe(
        &mut self,
        event: &Result<HearingEvent, coosenpai_core::ports::PortError>,
        control: &coosenpai_core::ports::HearingSessionControl,
        terminal_failure: &mut Option<(String, String)>,
        cancel_task: &mut Option<
            tauri::async_runtime::JoinHandle<Result<(), coosenpai_core::ports::PortError>>,
        >,
    ) -> bool {
        let failure = match event {
            Ok(HearingEvent::Ready { .. }) => {
                self.pending_source_error = false;
                None
            }
            Ok(HearingEvent::SourceState {
                source,
                state:
                    coosenpai_core::ports::HearingSourceState::Failed {
                        kind,
                        message,
                        retrying,
                    },
            }) => {
                self.pending_source_error = true;
                if *retrying {
                    self.terminal_sources.retain(|existing| existing != source);
                    None
                } else {
                    if !self.terminal_sources.contains(source) {
                        self.terminal_sources.push(*source);
                    }
                    self.sources
                        .iter()
                        .all(|source| self.terminal_sources.contains(source))
                        .then(|| (kind.clone(), message.clone()))
                }
            }
            Ok(HearingEvent::SourceState {
                source,
                state: coosenpai_core::ports::HearingSourceState::Ready { .. },
            }) => {
                self.pending_source_error = false;
                self.terminal_sources.retain(|existing| existing != source);
                None
            }
            Ok(HearingEvent::Error { kind, message }) => {
                let source_local = std::mem::take(&mut self.pending_source_error);
                if source_local {
                    return true;
                }
                Some((kind.clone(), message.clone()))
            }
            Err(error) => {
                self.pending_source_error = false;
                let (kind, message) = hearing_port_error(error);
                Some((kind.to_owned(), message))
            }
            _ => {
                self.pending_source_error = false;
                None
            }
        };
        if let Some((kind, message)) = failure {
            if terminal_failure.is_none() {
                *terminal_failure = Some((kind, message));
                let control = control.clone();
                *cancel_task = Some(tauri::async_runtime::spawn(async move {
                    control.cancel().await
                }));
            }
        }
        false
    }
}

impl ActiveHearingSession {
    async fn next_context_event<F, Fut>(
        &mut self,
        session_id: &str,
        runtime: &dyn crate::core_runtime_port::CoreRuntimePort,
        logger: &dyn RuntimeLogger,
        generation: u64,
        accept_final: F,
    ) -> Option<Result<HearingEvent, coosenpai_core::ports::PortError>>
    where
        F: Fn(AudioObservationSource, u64) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        tokio::select! {
            biased;
            _ = &mut self.stop_events => None,
            event = next_hearing_context_event(
                &mut self.session,
                session_id,
                runtime,
                logger,
                generation,
                accept_final,
            ) => event,
        }
    }
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
    output_devices_refresh: Mutex<()>,
    permission_port: Mutex<Arc<dyn SpeechPermissionPort>>,
    ingestion_barrier: Mutex<Option<AudioIngestionBarrier>>,
    terminal_barrier: Mutex<Option<AudioTerminalBarrier>>,
    lifecycle: Mutex<HearingLifecycle>,
    projection: Mutex<()>,
    restart_attempts: Mutex<u8>,
}

impl HearingController {
    pub(crate) async fn refresh_output_devices(&self, state: &DesktopState) {
        let _refresh = self.output_devices_refresh.lock().await;
        let port = self.hearing_port.lock().await.clone();
        state
            .publish_event(
                output_devices_event(port, crate::output_device_monitor::registration_failed())
                    .await,
            )
            .await;
    }

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
            output_devices_refresh: Mutex::new(()),
            permission_port: Mutex::new(Arc::new(crate::platform::MacSpeechPermissions)),
            ingestion_barrier: Mutex::new(None),
            terminal_barrier: Mutex::new(None),
            lifecycle: Mutex::new(HearingLifecycle::default()),
            projection: Mutex::new(()),
            restart_attempts: Mutex::new(0),
        }
    }

    pub(crate) fn sync(self: &Arc<Self>, state: Arc<DesktopState>) {
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            controller.reconcile(state).await;
        });
    }

    pub(crate) async fn reset_restart_attempts_if_disabled(
        &self,
        audio: &coosenpai_core::config::AudioConfig,
    ) {
        if !audio.enabled {
            *self.restart_attempts.lock().await = 0;
        }
    }

    pub(crate) async fn resume_speaker(
        self: &Arc<Self>,
        state: Arc<DesktopState>,
    ) -> Result<(), String> {
        let _projection = self.projection.lock().await;
        let snapshot = state.snapshot().await;
        if !snapshot.config.audio.enabled
            || !snapshot.config.audio.speaker
            || !matches!(
                snapshot.audio.speaker_capture,
                SpeakerCapture::Stopped { .. }
            )
        {
            return Err("再開できるスピーカー入力がありません".to_owned());
        }
        self.stop_locked(&state).await;
        *self.restart_attempts.lock().await = 0;
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::SpeakerResumed,
            ))
            .await;
        drop(_projection);
        self.sync(state);
        Ok(())
    }

    pub(crate) async fn cancel_and_wait(&self, state: &DesktopState) {
        let _projection = self.projection.lock().await;
        self.stop_and_wait_locked(state).await;
    }

    pub(crate) async fn delete_conversation_log_day(
        &self,
        state: &DesktopState,
        paths: ConfigPaths,
        date: chrono::NaiveDate,
    ) -> Result<(), coosenpai_core::runtime::RuntimeError> {
        let projection = self.projection.lock().await;
        self.stop_and_wait_locked(state).await;
        let result = state
            .core_runtime()
            .delete_conversation_log_day(paths, date)
            .await;
        drop(projection);
        result
    }

    async fn stop_and_wait_locked(&self, state: &DesktopState) {
        let outcome = self.take_stop().await;
        self.finish_stop(state, outcome).await;
    }

    pub(crate) async fn cancel(&self, state: &DesktopState) {
        let _projection = self.projection.lock().await;
        let Some(outcome) = self.take_stop().await else {
            return;
        };
        if !outcome.changed {
            return;
        }
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Cancelled(outcome.generation),
            ))
            .await;
        let generation = outcome.generation;
        let stopped = self.stop_session(outcome).await;
        if let Some(warning) = stopped.warning(generation) {
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(warning))
                .await;
            let _ = state.logger.write(
                "WARN",
                &format!(
                    "聴覚観察の停止が完了しませんでした: error-type=audio-cancel helper={:?} initialization-timeout={} ingestion-timeout={}",
                    stopped.helper, stopped.initialization_timed_out, stopped.ingestion_timed_out
                ),
            );
        }
    }

    async fn reconcile(self: Arc<Self>, state: Arc<DesktopState>) {
        if state.is_shutting_down() {
            return;
        }
        let _projection = self.projection.lock().await;
        let config = state.runtime_config();
        let (settings, model_source) = hearing_session_settings_for_state(&config, &state);
        let _ = state.logger.write(
            "INFO",
            &format!("hearing-speaker-model: source={model_source}"),
        );
        self.reset_restart_attempts_if_disabled(&config.audio).await;
        if !config.audio.enabled || !state.is_runtime_active() || state.voice_output.is_active() {
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
        if settings.sources == [AudioObservationSource::Speaker]
            && matches!(
                state.snapshot().await.audio.speaker_capture,
                SpeakerCapture::Stopped { .. }
            )
        {
            self.stop_locked(&state).await;
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
        let pending_exit = {
            let mut lifecycle = self.lifecycle.lock().await;
            if lifecycle.helper_exit_pending() {
                lifecycle.defer_start_until_helper_exits()
            } else {
                None
            }
        };
        if let Some(completed) = pending_exit {
            let controller = self.clone();
            tauri::async_runtime::spawn(async move {
                completed.cancelled().await;
                controller.lifecycle.lock().await.helper_exit_confirmed();
                controller.sync(state);
            });
            return;
        }
        if self.lifecycle.lock().await.helper_exit_pending() {
            return;
        }
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
        let mut helper_sources = sources_for_capture(
            &settings.sources,
            &state.snapshot().await.audio.speaker_capture,
        );
        if helper_sources.contains(&AudioObservationSource::Speaker)
            && crate::platform::speaker_requires_screen_recording()
        {
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
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                        HearingResult::Warning {
                            generation,
                            kind: "screen-capture".to_owned(),
                            message: message.to_owned(),
                        },
                    ))
                    .await;
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
        let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=audio-context phase=begin elapsed-ms={}", started.elapsed().as_millis()));
        let (context_session, audio_ingestion) = match state
            .core_runtime()
            .begin_hearing_session(generation, cancellation.clone())
            .await
        {
            Ok(session) => session,
            Err(_) if cancellation.is_cancelled() || state.is_shutting_down() => return,
            Err(error) => {
                publish_audio_failure(
                    &self,
                    state.clone(),
                    generation,
                    "audio-context",
                    &error.to_string(),
                    settings,
                    None,
                )
                .await;
                return;
            }
        };
        let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=audio-context phase=end elapsed-ms={}", started.elapsed().as_millis()));
        if cancellation.is_cancelled() {
            return;
        }
        let _ = state.logger.write("INFO", &format!("hearing-start: generation={generation} stage=helper-start phase=begin elapsed-ms={}", started.elapsed().as_millis()));
        let session = match port
            .start_with_options(
                &settings.locale,
                &settings.input_device,
                helper_sources.clone(),
                settings.debug_dump_dir.as_deref(),
                HearingStartOptions {
                    speaker_devices: settings.speaker_devices.clone(),
                    speaker_identification_enabled: settings.speaker_identification_enabled,
                    speaker_model: settings
                        .speaker_model_path
                        .as_deref()
                        .map(std::path::PathBuf::from),
                    speaker_ledger: settings
                        .speaker_ledger_path
                        .as_deref()
                        .map(std::path::PathBuf::from),
                },
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
        let (ingestion_done_tx, ingestion_done_rx) = oneshot::channel();
        let (stop_ingestion_tx, stop_ingestion_rx) = oneshot::channel();
        let (stop_events_tx, stop_events_rx) = oneshot::channel();
        let attach_outcome = {
            let mut lifecycle = self.lifecycle.lock().await;
            lifecycle.attach_session(
                generation,
                cancellation.clone(),
                control.clone(),
                stop_events_tx,
                stop_ingestion_rx,
            )
        };
        match attach_outcome {
            AttachOutcome::Listening => {
                let (ingestion_tx, ingestion_rx) = mpsc::channel(AUDIO_INGESTION_QUEUE_CAPACITY);
                let (result_tx, result_rx) = mpsc::channel(AUDIO_INGESTION_QUEUE_CAPACITY);
                let ingestion_barrier = self.ingestion_barrier.lock().await.take();
                let terminal_barrier = self.terminal_barrier.lock().await.take();
                let ingestion_state = state.clone();
                let ingestion_scope = audio_ingestion.clone();
                tauri::async_runtime::spawn(async move {
                    run_audio_ingestion(
                        ingestion_state.paths.clone(),
                        ingestion_state.runtime_config().retention.observation_days,
                        ingestion_scope,
                        ingestion_rx,
                        result_tx,
                        ingestion_barrier,
                    )
                    .await;
                });
                let delivery_state = state.clone();
                let session_ingestion = audio_ingestion.clone();
                let delivery_cancellation = cancellation.clone();
                let observation_interval = std::time::Duration::from_millis(
                    state.runtime_config().observer.hearing.interval_ms.max(1),
                );
                tauri::async_runtime::spawn(async move {
                    run_observation_worker(
                        delivery_state,
                        generation,
                        delivery_cancellation,
                        observation_interval,
                        audio_ingestion,
                    )
                    .await;
                });
                let result_controller = self.clone();
                let restart_settings = settings.clone();
                let result_state = state.clone();
                let result_cancellation = cancellation.clone();
                tauri::async_runtime::spawn(async move {
                    run_audio_result_worker(
                        result_controller,
                        result_state,
                        result_cancellation,
                        result_rx,
                        ingestion_done_tx,
                    )
                    .await;
                    // 結果 worker は保存 worker が結果の送信口を閉じるまで読み切るため、
                    // ここでの完了は保存と停止中の失敗記録の両方が終わったことを意味する。
                    let _ = stop_ingestion_tx.send(());
                });
                let session_controller = self.clone();
                tauri::async_runtime::spawn(async move {
                    session_controller
                        .run_session(
                            state,
                            generation,
                            cancellation,
                            ActiveHearingSession {
                                session,
                                sources: helper_sources,
                                stop_events: stop_events_rx,
                            },
                            restart_settings,
                            AudioIngestionHandle {
                                session_id: context_session,
                                scope: session_ingestion,
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
                if tokio::time::timeout(HEARING_STOP_LIMIT, control.cancel())
                    .await
                    .is_err()
                {
                    control.force_cancel();
                }
                control.wait_for_exit().await;
                self.complete_stop(generation).await;
            }
        }
    }

    async fn run_session(
        self: Arc<Self>,
        state: Arc<DesktopState>,
        generation: u64,
        cancellation: CancellationToken,
        active_session: ActiveHearingSession,
        settings: HearingSessionSettings,
        ingestion: AudioIngestionHandle,
    ) {
        let mut active_session = active_session;
        let AudioIngestionHandle {
            session_id: context_session,
            scope: ingestion_scope,
            sender: ingestion_sender,
            done: ingestion_completion,
            terminal_barrier,
        } = ingestion;
        let mut ingestion_tx = Some(ingestion_sender);
        let mut ingestion_done = Some(ingestion_completion);
        let mut terminal_barrier = terminal_barrier;
        let mut terminal_failure: Option<(String, String)> = None;
        let mut cancel_task = None;
        let mut source_errors = SourceErrorTracker::new(active_session.sources.clone());
        let mut recognition_locale = settings.locale.clone();
        let accept_controller = self.as_ref();
        let capture_state = state.as_ref();
        self.publish_active_result(&state, generation, HearingResult::Restarted(generation))
            .await;
        while let Some(next) = active_session
            .next_context_event(
                &context_session,
                state.core_runtime(),
                state.logger.as_ref(),
                generation,
                |source, input_generation| async move {
                    accept_controller.accepts_events(generation).await
                        && (source != AudioObservationSource::Speaker || {
                            let snapshot = capture_state.snapshot().await;
                            snapshot.audio.generation == generation
                                && snapshot
                                    .audio
                                    .accepts_speaker_input_generation(input_generation)
                        })
                },
            )
            .await
        {
            let event = next;
            let source_local_error = source_errors.observe(
                &event,
                &active_session.session.control(),
                &mut terminal_failure,
                &mut cancel_task,
            );
            match event {
                Ok(HearingEvent::Ready {
                    locale,
                    microphone,
                    recognition,
                    ready_sources,
                    ..
                }) => {
                    recognition_locale = locale;
                    self.publish_active_result(
                        &state,
                        generation,
                        HearingResult::Ready {
                            generation,
                            microphone,
                            recognition,
                            ready_sources,
                        },
                    )
                    .await;
                }
                Ok(HearingEvent::SourceState {
                    source,
                    state: source_state,
                }) => {
                    self.publish_active_result(
                        &state,
                        generation,
                        HearingResult::SourceState {
                            generation,
                            source,
                            state: source_state,
                        },
                    )
                    .await;
                }
                Ok(HearingEvent::SpeakerIdentification { status }) => {
                    if status == SpeakerIdentificationPreparationStatus::Unavailable
                        && self.accepts_events(generation).await
                    {
                        state
                            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                                HearingResult::Warning {
                                    generation,
                                    kind: "speaker-identification-unavailable".to_owned(),
                                    message: "話者識別を利用できないため、話者 ID なしで文字起こしを続けます".to_owned(),
                                },
                            ))
                            .await;
                    }
                }
                Ok(HearingEvent::Recognizing {
                    source,
                    generation: recognition_generation,
                    sequence,
                    text,
                }) => {
                    let wake_identity = crate::ui_events::WakeIdentity {
                        session_id: context_session.clone(),
                        source,
                        recognition_generation,
                        sequence,
                    };
                    if self
                        .forward_wake_recognizing(&state, wake_identity.clone(), text)
                        .await
                    {
                        let accepted = match ingestion_scope.invalidate_microphone_command_results()
                        {
                            Ok(()) => true,
                            Err(error) => {
                                let _ = state.logger.write(
                                    "WARN",
                                    &format!("マイク指示の分類世代を更新できません: {error}"),
                                );
                                false
                            }
                        };
                        state.ui.input(
                            crate::ui_events::UiView::Application,
                            crate::ui_events::UiEvent::WakeStart {
                                identity: wake_identity,
                                accepted,
                            },
                        );
                    }
                    if self.accepts_events(generation).await {
                        state
                            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                                HearingResult::Recovered {
                                    generation,
                                    source,
                                    final_result: false,
                                },
                            ))
                            .await;
                    }
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
                    if self.accepts_events(generation).await {
                        state
                            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                                HearingResult::Recovered {
                                    generation,
                                    source,
                                    final_result: false,
                                },
                            ))
                            .await;
                    }
                    self.publish_recognition_event(
                        &state,
                        generation,
                        source,
                        crate::snapshot::AudioLogStage::NoSpeech,
                    )
                    .await;
                }
                Ok(HearingEvent::Idle {
                    source,
                    generation: recognition_generation,
                }) => {
                    state.ui.input(
                        crate::ui_events::UiView::Application,
                        crate::ui_events::UiEvent::WakeIdle(crate::ui_events::WakeIdleIdentity {
                            session_id: context_session.clone(),
                            source,
                            recognition_generation,
                        }),
                    );
                }
                Ok(HearingEvent::WakeIdleRevoked {
                    source,
                    generation: recognition_generation,
                }) => {
                    state.ui.input(
                        crate::ui_events::UiView::Application,
                        crate::ui_events::UiEvent::WakeIdleFailed(
                            crate::ui_events::WakeIdleIdentity {
                                session_id: context_session.clone(),
                                source,
                                recognition_generation,
                            },
                        ),
                    );
                }
                Ok(HearingEvent::Final {
                    source,
                    text,
                    generation: recognition_generation,
                    sequence,
                    input_generation: _,
                    end_reason,
                    speaker,
                }) => {
                    let wake_identity = crate::ui_events::WakeIdentity {
                        session_id: context_session.clone(),
                        source,
                        recognition_generation,
                        sequence,
                    };
                    let wake_disposition = self
                        .forward_wake_final(
                            &state,
                            wake_identity,
                            text.clone(),
                            recognition_locale.clone(),
                            end_reason,
                        )
                        .await;
                    if wake_disposition == crate::ui_events::WakeFinalDisposition::WakeOnly {
                        let idle_identity = crate::ui_events::WakeIdleIdentity {
                            session_id: context_session.clone(),
                            source,
                            recognition_generation,
                        };
                        if let Err(error) = active_session
                            .session
                            .control()
                            .arm_wake_idle(recognition_generation)
                            .await
                        {
                            let _ = state.logger.write(
                                "WARN",
                                &format!("呼びかけ後の無言待機を開始できません: {error}"),
                            );
                            state.ui.input(
                                crate::ui_events::UiView::Application,
                                crate::ui_events::UiEvent::WakeIdleFailed(idle_identity),
                            );
                        }
                    }
                    // 文脈への登録前に取り込み状態と helper 世代を検証済み。
                    let Some(ingestion_sender) = ingestion_tx.as_ref() else {
                        return;
                    };
                    let context = coosenpai_core::hearing_context::HearingContext {
                        session_id: context_session.clone(),
                        source,
                        generation: recognition_generation,
                        sequence,
                        text,
                        confirmed: true,
                        speaker: speaker.map(|metadata| *metadata),
                    };
                    let corrections = final_corrections(&context);
                    let classification =
                        if wake_disposition == crate::ui_events::WakeFinalDisposition::Rejected {
                            ingestion_scope.register_microphone_final(&context)
                        } else {
                            ingestion_scope.exclude_wake_final(&context)
                        };
                    if let Err(error) = classification {
                        let _ = state
                            .logger
                            .write("WARN", &format!("マイク指示の受付に失敗しました: {error}"));
                    }
                    if !queue_audio_final_with_corrections(
                        ingestion_sender,
                        generation,
                        &context,
                        &corrections,
                    )
                    .await
                    {
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
                        let audio = state.snapshot().await.audio;
                        let _ = state.logger.write(
                            "INFO",
                            &format!(
                                "聴覚観察 warning UI: phase={} kind={:?} generation={}",
                                audio.phase, audio.warning_kind, audio.generation
                            ),
                        );
                    }
                }
                Ok(HearingEvent::Error { kind, message }) => {
                    if source_local_error {
                        let _ = state.logger.write(
                            "WARN",
                            &format!("聴覚観察 source-local error: kind={kind} generation={generation} {message}"),
                        );
                    }
                }
                Ok(HearingEvent::Closed) => {
                    if let Some(barrier) = terminal_barrier.take() {
                        barrier.received.notify_one();
                        barrier.release.notified().await;
                    }
                    finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
                    if let Some(task) = cancel_task.take() {
                        let _ = task.await;
                    }
                    if !cancellation.is_cancelled() && !self.is_stopping(generation).await {
                        let (kind, message) = terminal_failure.as_ref().map_or(
                            ("helper-closed", "聴覚観察 helper が予期せず終了しました"),
                            |(kind, message)| (kind.as_str(), message.as_str()),
                        );
                        self.handle_session_failure(
                            &state,
                            generation,
                            kind,
                            message,
                            settings.clone(),
                        )
                        .await;
                    }
                    return;
                }
                Ok(HearingEvent::MicrophoneDevice { .. } | HearingEvent::SpeakerFormat { .. }) => {
                    let _ = state.logger.write(
                        "WARN",
                        "聴覚観察の内部デバイスイベントが集約層を通過しました",
                    );
                }
                Err(_) => {}
            }
        }
        finish_audio_ingestion(&mut ingestion_tx, &mut ingestion_done).await;
        if let Some(task) = cancel_task {
            let _ = task.await;
        }
        if !cancellation.is_cancelled() && !self.is_stopping(generation).await {
            let (kind, message) = terminal_failure.as_ref().map_or(
                ("helper-closed", "聴覚観察 helper が予期せず終了しました"),
                |(kind, message)| (kind.as_str(), message.as_str()),
            );
            self.handle_session_failure(&state, generation, kind, message, settings)
                .await;
        }
    }

    async fn wake_display_name(
        &self,
        state: &DesktopState,
        identity: &crate::ui_events::WakeIdentity,
    ) -> Option<String> {
        if identity.source != AudioObservationSource::Microphone {
            return None;
        }
        let snapshot = state.snapshot().await;
        (snapshot.config.audio.enabled
            && snapshot.config.audio.mic
            && snapshot.config.audio.wake_enabled)
            .then_some(snapshot.companion_display_name)
    }

    async fn forward_wake_recognizing(
        &self,
        state: &DesktopState,
        identity: crate::ui_events::WakeIdentity,
        text: String,
    ) -> bool {
        let Some(display_name) = self.wake_display_name(state, &identity).await else {
            return false;
        };
        let (reply, accepted) = oneshot::channel();
        if state
            .ui
            .request(
                crate::ui_events::UiView::Application,
                crate::ui_events::UiEvent::WakeRecognizing {
                    identity,
                    display_name,
                    text,
                    reply,
                },
            )
            .await
            .is_err()
        {
            return false;
        }
        accepted.await.unwrap_or(false)
    }

    async fn forward_wake_final(
        &self,
        state: &DesktopState,
        identity: crate::ui_events::WakeIdentity,
        text: String,
        locale: String,
        end_reason: Option<coosenpai_core::ports::HearingSegmentEndReason>,
    ) -> crate::ui_events::WakeFinalDisposition {
        use crate::ui_events::WakeFinalDisposition;
        let Some(display_name) = self.wake_display_name(state, &identity).await else {
            return WakeFinalDisposition::Rejected;
        };
        let (reply, accepted) = oneshot::channel();
        if state
            .ui
            .request(
                crate::ui_events::UiView::Application,
                crate::ui_events::UiEvent::WakeFinal {
                    identity,
                    display_name,
                    text,
                    locale,
                    end_reason,
                    reply,
                },
            )
            .await
            .is_err()
        {
            return WakeFinalDisposition::Rejected;
        }
        accepted.await.unwrap_or(WakeFinalDisposition::Rejected)
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
        completed: AudioIngestionResult,
        cancellation: &CancellationToken,
    ) {
        let AudioIngestionResult { generation, result } = completed;
        if let Ok(HearingAudioRecord {
            observation,
            transcript_path,
        }) = &result
        {
            let current = state.snapshot().await.audio;
            if self.accepts_events(generation).await
                && !cancellation.is_cancelled()
                && (observation.source != AudioObservationSource::Speaker
                    || matches!(current.speaker_capture, SpeakerCapture::Running))
            {
                self.reset_restart_attempts().await;
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                        HearingResult::Recovered {
                            generation,
                            source: observation.source,
                            final_result: true,
                        },
                    ))
                    .await;
            }
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                    HearingResult::Observed {
                        generation,
                        observation: observation.clone(),
                        transcript_path: transcript_path.clone(),
                    },
                ))
                .await;
        }
        if result.is_err() {
            if cancellation.is_cancelled() || !self.accepts_events(generation).await {
                log_stopped_audio_failures(state.logger.as_ref(), 1);
            } else {
                publish_audio_error(
                    state,
                    Some(generation),
                    "observation",
                    "確定した音声を観察として保存できませんでした",
                )
                .await;
            }
        }
    }

    async fn reset_restart_attempts(&self) {
        *self.restart_attempts.lock().await = 0;
    }

    async fn accepts_events(&self, generation: u64) -> bool {
        self.lifecycle.lock().await.accepts_events(generation)
    }

    async fn publish_active_result(
        &self,
        state: &DesktopState,
        generation: u64,
        result: HearingResult,
    ) {
        if self.accepts_events(generation).await {
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(result))
                .await;
        }
    }

    async fn is_stopping(&self, generation: u64) -> bool {
        self.lifecycle.lock().await.is_stopping(generation)
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
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Stopping(outcome.generation),
            ))
            .await;
        let generation = outcome.generation;
        let stopped = self.stop_session(outcome).await;
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                HearingResult::Stopped(generation),
            ))
            .await;
        if let Some(warning) = stopped.warning(generation) {
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(warning))
                .await;
        }
    }

    async fn stop_session(&self, outcome: StopOutcome) -> StopSessionResult {
        let mut helper_timed_out = false;
        let force_control = outcome.control.clone();
        let helper = if let Some(control) = outcome.control {
            match tokio::time::timeout(HEARING_STOP_LIMIT, control.cancel()).await {
                Ok(result) => result,
                Err(_) => {
                    helper_timed_out = true;
                    if let Some(control) = &force_control {
                        control.force_cancel();
                        let _ = tokio::time::timeout(HEARING_FORCE_WAIT, control.cancel()).await;
                    }
                    Err(coosenpai_core::ports::PortError::HearingStopTimeout)
                }
            }
        } else {
            Ok(())
        };
        if let Some(stop_events) = outcome.stop_events {
            let _ = stop_events.send(());
        }
        let mut pending_initialization = None;
        let mut initialization_timed_out = false;
        if let Some(mut initialization_completed) = outcome.initialization_completed {
            if tokio::time::timeout(HEARING_STOP_LIMIT, &mut initialization_completed)
                .await
                .is_err()
            {
                initialization_timed_out = true;
                pending_initialization = Some(initialization_completed);
            }
        }
        let mut ingestion_timed_out = false;
        if let Some(ingestion_completed) = outcome.ingestion_completed {
            if tokio::time::timeout(HEARING_STOP_LIMIT, ingestion_completed)
                .await
                .is_err()
            {
                ingestion_timed_out = true;
            }
        }
        if (helper_timed_out || initialization_timed_out)
            && (force_control.is_some() || pending_initialization.is_some())
        {
            let completed = CancellationToken::new();
            self.lifecycle
                .lock()
                .await
                .record_pending_helper_exit(completed.clone());
            tokio::spawn(async move {
                if let Some(control) = force_control {
                    control.wait_for_exit().await;
                } else if let Some(initialization) = pending_initialization {
                    let _ = initialization.await;
                }
                completed.cancel();
            });
        }
        self.complete_stop(outcome.generation).await;
        StopSessionResult {
            helper,
            initialization_timed_out,
            ingestion_timed_out,
        }
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
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                    HearingResult::Failed {
                        generation: None,
                        kind: "restart-limit".to_owned(),
                        message: "聴覚観察 helper の自動再起動上限に達しました".to_owned(),
                        recognition: None,
                    },
                ))
                .await;
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
                    let (current, _) = hearing_session_settings_for_state(&config, &state);
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
    ingestion: HearingAudioIngestion,
    mut requests: mpsc::Receiver<AudioIngestionRequest>,
    results: mpsc::Sender<AudioIngestionResult>,
    barrier: Option<AudioIngestionBarrier>,
) {
    let mut barrier = barrier;
    let mut results = Some(results);
    while let Some(request) = requests.recv().await {
        if let Some(barrier) = barrier.take() {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
        let paths = paths.clone();
        let ingestion = ingestion.clone();
        let generation = match &request {
            AudioIngestionRequest::Final { generation, .. } => *generation,
        };
        let result = tokio::task::spawn_blocking(move || {
            let result = match request {
                AudioIngestionRequest::Final {
                    generation: _,
                    observation,
                    corrections,
                } => ingestion.record_with_corrections(
                    &paths,
                    retention_days,
                    *observation,
                    &corrections,
                ),
            };
            result
                .map_err(coosenpai_core::observer::ObserverError::from)
                .map_err(coosenpai_core::runtime::RuntimeError::from)
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
        let result = match result {
            Ok(Some(record)) => Ok(record),
            Ok(None) => continue,
            Err(error) => Err(error),
        };
        if let Some(sender) = results.as_ref() {
            if sender
                .send(AudioIngestionResult { generation, result })
                .await
                .is_err()
            {
                results = None;
            }
        }
    }
}

async fn queue_audio_final_with_corrections(
    ingestion_tx: &mpsc::Sender<AudioIngestionRequest>,
    generation: u64,
    context: &coosenpai_core::hearing_context::HearingContext,
    corrections: &[HearingSpeakerCorrection],
) -> bool {
    let Ok(observation) = context.confirmed_audio(chrono::Utc::now()) else {
        return false;
    };
    ingestion_tx
        .send(AudioIngestionRequest::Final {
            generation,
            observation: Box::new(observation),
            corrections: corrections.to_vec(),
        })
        .await
        .is_ok()
}

fn final_corrections(
    context: &coosenpai_core::hearing_context::HearingContext,
) -> Vec<HearingSpeakerCorrection> {
    context
        .speaker
        .as_ref()
        .map_or_else(Vec::new, |metadata| metadata.speaker_corrections.clone())
}

async fn run_audio_result_worker(
    controller: Arc<HearingController>,
    state: Arc<DesktopState>,
    cancellation: CancellationToken,
    mut results: mpsc::Receiver<AudioIngestionResult>,
    ingestion_done: oneshot::Sender<()>,
) {
    while let Some(result) = results.recv().await {
        controller
            .handle_audio_result(&state, result, &cancellation)
            .await;
    }
    let _ = ingestion_done.send(());
}

fn log_stopped_audio_failures(logger: &dyn RuntimeLogger, failed: usize) {
    if failed > 0 {
        let _ = logger.write(
            "WARN",
            &format!("hearing-audio-ingestion: failed={failed} reason=persist-after-stop"),
        );
    }
}

async fn finish_audio_ingestion(
    ingestion_tx: &mut Option<mpsc::Sender<AudioIngestionRequest>>,
    ingestion_done: &mut Option<oneshot::Receiver<()>>,
) {
    ingestion_tx.take();
    if let Some(done) = ingestion_done.take() {
        let _ = done.await;
    }
}

async fn run_observation_worker(
    state: Arc<DesktopState>,
    generation: u64,
    cancellation: CancellationToken,
    observation_interval: std::time::Duration,
    ingestion: HearingAudioIngestion,
) {
    let mut ticks = tokio::time::interval_at(
        tokio::time::Instant::now() + observation_interval,
        observation_interval,
    );
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => break,
            _ = ticks.tick() => {}
        }
        match observe_pending_audio(
            state.core_runtime(),
            &state.paths,
            state.runtime_config().retention.observation_days,
            &ingestion,
            &cancellation,
        )
        .await
        {
            Ok(Some(observation)) if !cancellation.is_cancelled() => {
                match coosenpai_core::usage::today_observer_usage(&state.paths.usage) {
                    Ok(usage) => {
                        state
                            .publish_event(crate::snapshot_presenter::SnapshotEvent::Hearing(
                                HearingResult::Analyzed {
                                    generation,
                                    observation,
                                    calls: usage.ai_calls,
                                },
                            ))
                            .await;
                    }
                    Err(_) => {
                        let _ = state.logger.write(
                            "WARN",
                            "音声観察の使用量を読み込めませんでした: error-type=audio-usage",
                        );
                    }
                }
            }
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => {
                if cancellation.is_cancelled() {
                    break;
                }
                let _ = state.logger.write("WARN", &format!("音声の観察を次の周期へ延期しました: error-type=audio-observation error={error}"));
            }
        }
        if cancellation.is_cancelled() {
            break;
        }
        if let Err(error) = state
            .core_runtime()
            .process_mailbox(cancellation.child_token())
            .await
        {
            let _ = state.logger.write(
                "WARN",
                &format!(
                    "音声観察の配達を次回へ延期しました: error-type=audio-delivery error={error}"
                ),
            );
        }
    }
}

async fn observe_pending_audio(
    runtime: &dyn crate::core_runtime_port::CoreRuntimePort,
    paths: &ConfigPaths,
    retention_days: u64,
    ingestion: &HearingAudioIngestion,
    cancellation: &CancellationToken,
) -> Result<Option<ObservationRecord>, coosenpai_core::runtime::RuntimeError> {
    let paths = paths.clone();
    let scope = ingestion.clone();
    let audio = tokio::task::spawn_blocking(move || scope.pending_audio(&paths, retention_days))
        .await
        .map_err(|error| coosenpai_core::runtime::RuntimeError::Factory(error.to_string()))?
        .map_err(coosenpai_core::observer::ObserverError::from)?;
    if audio.is_empty() {
        return Ok(None);
    }
    runtime
        .audio_observation(audio, cancellation.child_token())
        .await
        .map(Some)
}

pub(crate) fn retryable_audio_error(kind: &str) -> bool {
    matches!(
        kind,
        "recognition"
            | "recognition-microphone"
            | "audio-microphone"
            | "helper"
            | "helper-closed"
            | "helper-start"
    )
}

fn hearing_port_error(error: &coosenpai_core::ports::PortError) -> (&'static str, String) {
    match error {
        coosenpai_core::ports::PortError::SpeakerOverflow(message) => {
            ("system-audio-overflow", message.clone())
        }
        coosenpai_core::ports::PortError::SpeakerCaptureExhausted(message) => {
            ("system-audio-exhausted", message.clone())
        }
        coosenpai_core::ports::PortError::SpeakerProtocol(message) => {
            ("speaker-protocol", message.clone())
        }
        coosenpai_core::ports::PortError::Protocol(message) => {
            ("hearing-protocol", message.clone())
        }
        _ => ("helper", error.to_string()),
    }
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

fn sources_for_capture(
    sources: &[AudioObservationSource],
    capture: &SpeakerCapture,
) -> Vec<AudioObservationSource> {
    if matches!(capture, SpeakerCapture::Stopped { .. }) {
        sources_without_speaker(sources)
    } else {
        sources.to_vec()
    }
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
    let speaker_devices = if config.audio.speaker {
        config.audio.speaker_devices.clone()
    } else {
        Vec::new()
    };
    HearingSessionSettings::new(
        config.speech.locale.clone(),
        config.speech.input_device.clone(),
        selected_sources(config),
    )
    .with_debug_dump_dir(config.audio.debug_dump_dir.clone())
    .with_speaker_devices(speaker_devices)
}

fn hearing_session_settings_for_state(
    config: &coosenpai_core::config::Config,
    state: &DesktopState,
) -> (HearingSessionSettings, &'static str) {
    let resource_directory = state.app.path().resource_dir().ok();
    let (model_path, source) = resolve_speaker_model_path(
        std::env::var("COOSENPAI_SPEAKER_MODEL").ok(),
        config.audio.speaker_identification.model_path.as_deref(),
        resource_directory.as_deref(),
    );
    (
        hearing_session_settings(config).with_speaker_identification(
            config.audio.speaker_identification.enabled,
            model_path,
            Some(
                state
                    .paths
                    .speakers
                    .join("registry.enc")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ),
        source,
    )
}

const SPEAKER_MODEL_RESOURCE_PATH: &str = "models/speaker-id/WespeakerResNet34LM.mlpackage";

fn resolve_speaker_model_path(
    environment_path: Option<String>,
    configured_path: Option<&str>,
    resource_directory: Option<&Path>,
) -> (Option<String>, &'static str) {
    if let Some(path) = environment_path.filter(|path| !path.is_empty()) {
        return (Some(path), "env");
    }
    if let Some(path) = configured_path {
        return (Some(path.to_owned()), "config");
    }
    let bundled_path = resource_directory
        .map(|directory| directory.join(SPEAKER_MODEL_RESOURCE_PATH))
        .filter(|path| path.is_dir());
    if let Some(path) = bundled_path {
        return (Some(path.to_string_lossy().into_owned()), "bundled");
    }
    (None, "none")
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
