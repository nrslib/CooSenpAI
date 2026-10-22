use super::source_name;
use coosenpai_core::interactive_process::{
    InteractiveProcess, InteractiveProcessControl, InteractiveProcessError,
    InteractiveProcessEvent, InteractiveProcessRequest, SHUTDOWN_GRACE,
};
use coosenpai_core::ports::{decode_hearing_event, HearingEvent, PortError, RuntimeLogger};
use coosenpai_core::state::AudioObservationSource;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc as sync_mpsc, Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MAX_RESTART_ATTEMPTS: u8 = 3;
const DIAGNOSTIC_LOG_CAPACITY: usize = 128;
const SPEAKER_LOAD_STOP_EXIT_CODE: i32 = 21;
const SPEAKER_CLEANUP_TIMEOUT_EXIT_CODE: i32 = 22;
const SPEAKER_CLEANUP_FAILURE_EXIT_CODE: i32 = 23;
const SPEAKER_LOAD_CLEANUP_TIMEOUT_EXIT_CODE: i32 = 24;
const SPEAKER_LOAD_CLEANUP_FAILURE_EXIT_CODE: i32 = 25;

fn is_speaker_load_stop(status: Option<i32>) -> bool {
    matches!(
        status,
        Some(SPEAKER_LOAD_STOP_EXIT_CODE)
            | Some(SPEAKER_LOAD_CLEANUP_TIMEOUT_EXIT_CODE)
            | Some(SPEAKER_LOAD_CLEANUP_FAILURE_EXIT_CODE)
    )
}

fn stop_exit_error(status: Option<i32>, closed_seen: bool) -> Option<PortError> {
    match status {
        Some(SPEAKER_CLEANUP_TIMEOUT_EXIT_CODE | SPEAKER_LOAD_CLEANUP_TIMEOUT_EXIT_CODE) => {
            Some(PortError::HearingStopTimeout)
        }
        Some(SPEAKER_CLEANUP_FAILURE_EXIT_CODE | SPEAKER_LOAD_CLEANUP_FAILURE_EXIT_CODE) => {
            Some(PortError::HearingStopCleanupFailed)
        }
        _ if !closed_seen => Some(PortError::HearingStopUnconfirmed),
        Some(0) => None,
        _ => Some(PortError::HearingStopCleanupFailed),
    }
}

pub(super) struct ActiveHelper {
    source: AudioObservationSource,
    control: InteractiveProcessControl,
}

#[derive(Default)]
pub(super) struct ActiveHelperRegistry {
    helpers: Mutex<HashMap<u32, Arc<ActiveHelper>>>,
    update_gate: tokio::sync::Mutex<()>,
}

pub(super) type ActiveHelpers = Arc<ActiveHelperRegistry>;

pub(super) async fn register_helper(
    source: AudioObservationSource,
    control: InteractiveProcessControl,
    active: &ActiveHelpers,
) {
    let Some(pid) = control.pid() else { return };
    let helper = Arc::new(ActiveHelper { source, control });
    active
        .helpers
        .lock()
        .expect("helper registry")
        .insert(pid, helper.clone());
    refresh_speaker_exclusions(active).await;
    let active = active.clone();
    tokio::spawn(async move {
        helper.control.wait_for_completion().await;
        {
            let mut registry = active.helpers.lock().expect("helper registry");
            if registry
                .get(&pid)
                .is_some_and(|current| Arc::ptr_eq(current, &helper))
            {
                registry.remove(&pid);
            }
        }
        if source == AudioObservationSource::Microphone {
            refresh_speaker_exclusions(&active).await;
        }
    });
}

async fn refresh_speaker_exclusions(active: &ActiveHelpers) {
    let _update = active.update_gate.lock().await;
    let (speaker, microphones) = {
        let registry = active.helpers.lock().expect("helper registry");
        let speaker = registry
            .values()
            .find(|helper| helper.source == AudioObservationSource::Speaker)
            .map(|helper| helper.control.clone());
        let microphones = registry
            .values()
            .filter(|helper| helper.source == AudioObservationSource::Microphone)
            .filter_map(|helper| helper.control.pid())
            .collect::<Vec<_>>();
        (speaker, microphones)
    };
    if let Some(speaker) = speaker {
        let command = serde_json::json!({"op": "update-exclude-pids", "pids": microphones});
        let _ = speaker.write_line(command.to_string().into_bytes()).await;
    }
}

pub(super) async fn wait_for_hearing_helpers(active: &ActiveHelpers) {
    let controls = active
        .helpers
        .lock()
        .expect("helper registry")
        .values()
        .map(|helper| helper.control.clone())
        .collect::<Vec<_>>();
    for control in controls {
        control.wait_for_completion().await;
    }
}

pub(super) async fn force_kill_hearing_helpers(active: &ActiveHelpers) {
    let controls = active
        .helpers
        .lock()
        .expect("helper registry")
        .values()
        .map(|helper| helper.control.clone())
        .collect::<Vec<_>>();
    for control in controls {
        let _ = control.terminate(true).await;
    }
}

struct BufferedDiagnosticLogger {
    sender: sync_mpsc::SyncSender<(String, String)>,
    dropped: Arc<AtomicU64>,
}

impl BufferedDiagnosticLogger {
    fn new(logger: Arc<dyn RuntimeLogger>) -> Self {
        let (sender, receiver) =
            sync_mpsc::sync_channel::<(String, String)>(DIAGNOSTIC_LOG_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let worker_dropped = dropped.clone();
        tokio::task::spawn_blocking(move || {
            while let Ok((level, message)) = receiver.recv() {
                let count = worker_dropped.swap(0, Ordering::Relaxed);
                if count > 0 {
                    let _ = logger.write(
                        "WARN",
                        &format!("聴覚観察 helper 診断ログを破棄しました dropped={count}"),
                    );
                }
                let _ = logger.write(&level, &message);
            }
            let count = worker_dropped.swap(0, Ordering::Relaxed);
            if count > 0 {
                let _ = logger.write(
                    "WARN",
                    &format!("聴覚観察 helper 診断ログを破棄しました dropped={count}"),
                );
            }
        });
        Self { sender, dropped }
    }
}

impl RuntimeLogger for BufferedDiagnosticLogger {
    fn write(&self, level: &str, message: &str) -> std::io::Result<()> {
        match self.sender.try_send((level.to_owned(), message.to_owned())) {
            Ok(()) => Ok(()),
            Err(sync_mpsc::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(sync_mpsc::TrySendError::Disconnected(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "聴覚観察 helper 診断ログの処理が終了しました",
            )),
        }
    }
}

pub(crate) struct SourceProcessSpec {
    pub(crate) source: AudioObservationSource,
    pub(crate) executable: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) initial_process: Option<InteractiveProcess>,
    pub(crate) initial_error: Option<PortError>,
    pub(crate) parent_cancellation: CancellationToken,
    pub(crate) logger: Arc<dyn RuntimeLogger>,
    pub(crate) active_helpers: ActiveHelpers,
}

pub(crate) struct SourceProcessEvent {
    pub(crate) source: AudioObservationSource,
    pub(crate) kind: SourceProcessEventKind,
}

pub(crate) enum SourceProcessEventKind {
    Restarting,
    Event {
        event: HearingEvent,
        source_retrying: bool,
    },
    Unavailable {
        error: Option<PortError>,
        retrying: bool,
    },
    Exhausted {
        error: Option<PortError>,
        stop_reason: String,
        capture_failure: bool,
    },
}

enum ProcessOutcome {
    Restart {
        error: Option<PortError>,
        reset_restart_budget: bool,
    },
    Stopped(Result<(), PortError>),
}

struct PersistentCaptureFailure {
    kind: String,
    message: String,
}

impl PersistentCaptureFailure {
    fn from_event(event: &HearingEvent) -> Option<Self> {
        let HearingEvent::Error { kind, message } = event else {
            return None;
        };
        if !(kind == "screen-capture"
            || kind == "system-audio"
            || kind.starts_with("system-audio-"))
            || matches!(
                kind.as_str(),
                "system-audio-overflow" | "system-audio-restored"
            )
        {
            return None;
        }
        Some(Self {
            kind: kind.clone(),
            message: message.clone(),
        })
    }

    fn error(&self, exhausted: bool) -> PortError {
        let reason = format!("source=speaker kind={}: {}", self.kind, self.message);
        if exhausted {
            PortError::SpeakerCaptureExhausted(format!(
                "取り込み口の失敗が{}回に達したため停止しました: {reason}",
                MAX_RESTART_ATTEMPTS + 1
            ))
        } else {
            PortError::Unavailable(reason)
        }
    }
}

struct ForwardSanitizedFinalContext<'a> {
    source: AudioObservationSource,
    events: &'a mpsc::Sender<SourceProcessEvent>,
    logger: &'a dyn RuntimeLogger,
}

pub(crate) async fn run_source_process(
    mut spec: SourceProcessSpec,
    events: mpsc::Sender<SourceProcessEvent>,
    cancellation: CancellationToken,
) -> Result<(), PortError> {
    let mut restart_attempts = 0;
    let mut capture_restart_attempts = 0;
    let diagnostic_logger = BufferedDiagnosticLogger::new(spec.logger.clone());
    let mut initial_process = spec.initial_process.take();
    let mut initial_error = spec.initial_error.take();

    loop {
        let mut load_stop_seen = false;
        let mut persistent_failure = None;
        if cancellation.is_cancelled() || spec.parent_cancellation.is_cancelled() {
            if let Some(mut process) = initial_process.take() {
                return match monitor_process(
                    spec.source,
                    &mut process,
                    &events,
                    &cancellation,
                    &spec.parent_cancellation,
                    &diagnostic_logger,
                    &mut load_stop_seen,
                    &mut persistent_failure,
                )
                .await
                {
                    ProcessOutcome::Stopped(result) => result,
                    ProcessOutcome::Restart { .. } => Ok(()),
                };
            }
            return Ok(());
        }
        if !send_source_event(&events, spec.source, SourceProcessEventKind::Restarting).await {
            if let Some(mut process) = initial_process.take() {
                let control = process.control();
                return cancel_and_reap(&mut process, &control, &diagnostic_logger).await;
            }
            return Ok(());
        }

        let process = match initial_process.take() {
            Some(process) => Ok(process),
            None => match initial_error.take() {
                Some(error) => Err(error),
                None => spawn_process(&spec).await,
            },
        };
        let mut process = match process {
            Ok(process) => process,
            Err(error) => {
                if !schedule_restart(
                    spec.source,
                    Some(error),
                    &mut restart_attempts,
                    false,
                    &events,
                    &cancellation,
                    &spec.parent_cancellation,
                    &diagnostic_logger,
                )
                .await
                {
                    return Ok(());
                }
                continue;
            }
        };

        let outcome = monitor_process(
            spec.source,
            &mut process,
            &events,
            &cancellation,
            &spec.parent_cancellation,
            &diagnostic_logger,
            &mut load_stop_seen,
            &mut persistent_failure,
        )
        .await;
        match outcome {
            ProcessOutcome::Stopped(result) => return result,
            ProcessOutcome::Restart {
                mut error,
                reset_restart_budget,
            } => {
                if let Some(failure) = persistent_failure.as_ref() {
                    error = Some(failure.error(capture_restart_attempts >= MAX_RESTART_ATTEMPTS));
                } else if load_stop_seen && error.is_none() {
                    error = Some(PortError::SpeakerOverflow(
                        "スピーカー音声の負荷による停止".to_owned(),
                    ));
                }
                if reset_restart_budget && !load_stop_seen {
                    restart_attempts = 0;
                }
                let capture_failure = persistent_failure.is_some() || load_stop_seen;
                let attempts = if capture_failure {
                    &mut capture_restart_attempts
                } else {
                    &mut restart_attempts
                };
                if !schedule_restart(
                    spec.source,
                    error,
                    attempts,
                    capture_failure,
                    &events,
                    &cancellation,
                    &spec.parent_cancellation,
                    &diagnostic_logger,
                )
                .await
                {
                    return Ok(());
                }
            }
        }
    }
}

async fn spawn_process(spec: &SourceProcessSpec) -> Result<InteractiveProcess, PortError> {
    let process = InteractiveProcess::spawn(
        InteractiveProcessRequest {
            executable: spec.executable.clone(),
            args: spec.args.clone(),
            env: Vec::new(),
            cwd: None,
        },
        CancellationToken::new(),
    )
    .await
    .map_err(process_error)?;
    register_helper(spec.source, process.control(), &spec.active_helpers).await;
    Ok(process)
}

#[allow(clippy::too_many_arguments)]
async fn schedule_restart(
    source: AudioObservationSource,
    error: Option<PortError>,
    restart_attempts: &mut u8,
    capture_failure: bool,
    events: &mpsc::Sender<SourceProcessEvent>,
    cancellation: &CancellationToken,
    parent_cancellation: &CancellationToken,
    logger: &dyn RuntimeLogger,
) -> bool {
    if let Some(error) = error.as_ref() {
        let _ = logger.write(
            "WARN",
            &format!(
                "聴覚観察 source-local process failure: source={} error={error}",
                source_name(source)
            ),
        );
    }
    let stop_reason = if *restart_attempts >= MAX_RESTART_ATTEMPTS {
        let detail = error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "helper が終了しました".to_owned());
        Some(if capture_failure {
            match error.as_ref() {
                Some(PortError::SpeakerCaptureExhausted(message)) => message.clone(),
                _ => format!(
                    "取り込み口の失敗が{}回に達したためスピーカー入力を停止しました: {detail}",
                    MAX_RESTART_ATTEMPTS + 1
                ),
            }
        } else {
            format!(
                "helper の失敗が{}回に達したため入力を停止しました: {detail}",
                MAX_RESTART_ATTEMPTS + 1
            )
        })
    } else {
        None
    };
    let retrying = *restart_attempts < MAX_RESTART_ATTEMPTS;
    if !send_source_event(
        events,
        source,
        SourceProcessEventKind::Unavailable { error, retrying },
    )
    .await
    {
        return false;
    }
    if !retrying {
        let _ = send_source_event(
            events,
            source,
            SourceProcessEventKind::Exhausted {
                error: None,
                stop_reason: stop_reason.expect("exhaustion has a stop reason"),
                capture_failure,
            },
        )
        .await;
        return false;
    }
    *restart_attempts += 1;
    let delay = restart_delay(*restart_attempts);
    let delay_sleep = tokio::time::sleep(delay);
    tokio::pin!(delay_sleep);
    tokio::select! {
        _ = &mut delay_sleep => true,
        _ = cancellation.cancelled() => false,
        _ = parent_cancellation.cancelled() => false,
    }
}

async fn forward_sanitized_final(
    event: HearingEvent,
    reason: &str,
    context: &ForwardSanitizedFinalContext<'_>,
) -> bool {
    if let HearingEvent::Final {
        generation,
        sequence,
        ..
    } = &event
    {
        let _ = context.logger.write(
            "WARN",
            &format!(
                "speaker-metadata-unavailable utterance-identification-unavailable transcription-continued source={} generation={generation} sequence={sequence} reason={}",
                source_name(context.source),
                speaker_metadata_reason_class(reason),
            ),
        );
    }
    send_source_event(
        context.events,
        context.source,
        SourceProcessEventKind::Event {
            event,
            source_retrying: false,
        },
    )
    .await
}

fn speaker_metadata_reason_class(reason: &str) -> &'static str {
    match reason {
        "metadata.decode" => "metadata.decode",
        "metadata.segment-id" => "metadata.segment-id",
        "metadata.period-bounds" => "metadata.period-bounds",
        "metadata.registry-id" => "metadata.registry-id",
        "metadata.correction-count" => "metadata.correction-count",
        "metadata.correction-consistency" => "metadata.correction-consistency",
        "metadata.correction-registry" => "metadata.correction-registry",
        "metadata.speaker-id" => "metadata.speaker-id",
        "metadata.identified-registry" => "metadata.identified-registry",
        "metadata.status-identifiers" => "metadata.status-identifiers",
        _ if reason.starts_with("metadata.speakerSegments[") => "metadata.speaker-segments",
        _ if reason.starts_with("metadata.speakerCorrections[") => "metadata.speaker-corrections",
        _ => "metadata.validation",
    }
}

#[allow(clippy::too_many_arguments)]
async fn monitor_process(
    source: AudioObservationSource,
    process: &mut InteractiveProcess,
    events: &mpsc::Sender<SourceProcessEvent>,
    cancellation: &CancellationToken,
    parent_cancellation: &CancellationToken,
    logger: &dyn RuntimeLogger,
    load_stop_seen: &mut bool,
    persistent_failure: &mut Option<PersistentCaptureFailure>,
) -> ProcessOutcome {
    let control = process.control();
    let mut valid_final_seen = false;
    let forward_context = ForwardSanitizedFinalContext {
        source,
        events,
        logger,
    };
    let device_started = std::time::Instant::now();
    let mut stopping = false;
    let mut closed_seen = false;
    let mut stop_limit_exceeded = false;
    let force_shutdown = tokio::time::sleep(SHUTDOWN_GRACE);
    tokio::pin!(force_shutdown);
    let mut force_shutdown_armed = false;
    let mut cancel_write_error = None;
    let _ = logger.write(
        "INFO",
        &format!("hearing-start: source={source:?} stage=device-ready phase=begin"),
    );
    loop {
        if !stopping && (cancellation.is_cancelled() || parent_cancellation.is_cancelled()) {
            stopping = true;
            if let Err(error) = control.write_line(br#"{"op":"cancel"}"#.to_vec()).await {
                cancel_write_error = Some(process_error(error));
            }
            force_shutdown
                .as_mut()
                .reset(tokio::time::Instant::now() + SHUTDOWN_GRACE);
            force_shutdown_armed = true;
        }
        let event = tokio::select! {
            biased;
            _ = &mut force_shutdown, if force_shutdown_armed => {
                stop_limit_exceeded = true;
                let _ = control.terminate(false).await;
                force_shutdown_armed = false;
                continue;
            }
            event = process.next_event() => event,
            _ = cancellation.cancelled(), if !stopping => continue,
            _ = parent_cancellation.cancelled(), if !stopping => continue,
        };
        match event {
            Some(Ok(InteractiveProcessEvent::StdoutLine(line))) => {
                if stopping {
                    if decode_hearing_event(&line).ok().is_some_and(|decoded| {
                        matches!(decoded.into_parts().0, HearingEvent::Closed)
                    }) {
                        closed_seen = true;
                    }
                    continue;
                }
                match decode_hearing_event(&line) {
                    Ok(decoded) => {
                        let (event, wire_metadata_reason) = decoded.into_parts();
                        if source == AudioObservationSource::Speaker
                            && *load_stop_seen
                            && matches!(
                                event,
                                HearingEvent::Recognizing { .. }
                                    | HearingEvent::NoSpeech { .. }
                                    | HearingEvent::Final { .. }
                            )
                        {
                            continue;
                        }
                        if let Some(reason) = wire_metadata_reason {
                            if event.is_valid_required_final_for_source(source)
                                && event.is_valid_for_source(source)
                            {
                                valid_final_seen = true;
                                if !forward_sanitized_final(event, reason, &forward_context).await {
                                    return ProcessOutcome::Stopped(
                                        cancel_and_reap(process, &control, logger).await,
                                    );
                                }
                                continue;
                            }
                        }
                        match event {
                            event if !event.is_valid_for_source(source) => {
                                if let Some((event, reason)) =
                                    event.detach_invalid_speaker_metadata(source)
                                {
                                    valid_final_seen = true;
                                    if !forward_sanitized_final(event, &reason, &forward_context)
                                        .await
                                    {
                                        return ProcessOutcome::Stopped(
                                            cancel_and_reap(process, &control, logger).await,
                                        );
                                    }
                                    continue;
                                }
                                let error = invalid_protocol_error(
                                    source,
                                    "聴覚観察 helper の話者情報が不正です",
                                    "聴覚観察 helper のイベント形式が不正です",
                                );
                                return terminate_before_restart(
                                    process,
                                    &control,
                                    Some(error),
                                    valid_final_seen,
                                    closed_seen,
                                    cancellation,
                                    parent_cancellation,
                                    logger,
                                )
                                .await;
                            }
                            event @ HearingEvent::Ready { .. } => {
                                let _ = logger.write("INFO", &format!("hearing-start: source={source:?} stage=device-ready phase=end elapsed-ms={}", device_started.elapsed().as_millis()));
                                if !send_source_event(
                                    events,
                                    source,
                                    SourceProcessEventKind::Event {
                                        event,
                                        source_retrying: false,
                                    },
                                )
                                .await
                                {
                                    return ProcessOutcome::Stopped(
                                        cancel_and_reap(process, &control, logger).await,
                                    );
                                }
                            }
                            HearingEvent::Error { ref kind, .. } if kind == "no-input-source" => {}
                            HearingEvent::Closed => {
                                closed_seen = true;
                                return terminate_before_restart(
                                    process,
                                    &control,
                                    None,
                                    valid_final_seen,
                                    closed_seen,
                                    cancellation,
                                    parent_cancellation,
                                    logger,
                                )
                                .await;
                            }
                            event => {
                                let source_retrying = source == AudioObservationSource::Speaker
                                    && matches!(&event, HearingEvent::Error { .. });
                                let terminal_error = source == AudioObservationSource::Microphone
                                    && matches!(&event, HearingEvent::Error { .. });
                                if source == AudioObservationSource::Speaker {
                                    if let Some(failure) =
                                        PersistentCaptureFailure::from_event(&event)
                                    {
                                        *persistent_failure = Some(failure);
                                    }
                                }
                                if source == AudioObservationSource::Speaker
                                    && matches!(&event, HearingEvent::Error { kind, .. } if kind == "system-audio-overflow")
                                {
                                    *load_stop_seen = true;
                                    *persistent_failure = None;
                                }
                                if matches!(event, HearingEvent::Final { .. }) {
                                    valid_final_seen = true;
                                }
                                if !send_source_event(
                                    events,
                                    source,
                                    SourceProcessEventKind::Event {
                                        event,
                                        source_retrying,
                                    },
                                )
                                .await
                                {
                                    return ProcessOutcome::Stopped(
                                        cancel_and_reap(process, &control, logger).await,
                                    );
                                }
                                if terminal_error {
                                    return ProcessOutcome::Stopped(
                                        cancel_and_reap(process, &control, logger).await,
                                    );
                                }
                            }
                        }
                    }
                    Err(_) => {
                        let error = invalid_protocol_error(
                            source,
                            "聴覚観察 helper の話者イベント JSON が不正です",
                            "聴覚観察 helper のイベント JSON が不正です",
                        );
                        return terminate_before_restart(
                            process,
                            &control,
                            Some(error),
                            valid_final_seen,
                            closed_seen,
                            cancellation,
                            parent_cancellation,
                            logger,
                        )
                        .await;
                    }
                }
            }
            Some(Ok(InteractiveProcessEvent::StderrLine(line))) => {
                log_helper_stderr(logger, &line);
            }
            Some(Ok(InteractiveProcessEvent::Exited { status, stderr })) => {
                process.wait_for_completion().await;
                if stopping || cancellation.is_cancelled() || parent_cancellation.is_cancelled() {
                    if !closed_seen {
                        let _ = logger
                            .write("ERROR", "聴覚観察 helper は closed を送らずに終了しました");
                    }
                    if stop_limit_exceeded {
                        return ProcessOutcome::Stopped(Err(PortError::HearingStopTimeout));
                    }
                    if let Some(error) = stop_exit_error(status, closed_seen) {
                        return ProcessOutcome::Stopped(Err(error));
                    }
                    return ProcessOutcome::Stopped(if closed_seen {
                        Ok(())
                    } else {
                        cancel_write_error.map_or(Ok(()), Err)
                    });
                }
                if source == AudioObservationSource::Speaker {
                    *load_stop_seen |= is_speaker_load_stop(status);
                    if is_speaker_load_stop(status) {
                        *persistent_failure = None;
                    }
                    if !*load_stop_seen && persistent_failure.is_none() {
                        *persistent_failure = Some(PersistentCaptureFailure {
                            kind: "system-audio-helper-exit".to_owned(),
                            message: "closed を受信できないまま helper が終了しました".to_owned(),
                        });
                    }
                }
                let error =
                    if is_speaker_load_stop(status) && source == AudioObservationSource::Speaker {
                        Some(PortError::SpeakerOverflow(
                            "スピーカー音声の負荷による停止中に helper が終了しました".to_owned(),
                        ))
                    } else if status != Some(0) {
                        Some(helper_exit_error(&stderr))
                    } else {
                        None
                    };
                return ProcessOutcome::Restart {
                    reset_restart_budget: valid_final_seen,
                    error,
                };
            }
            Some(Err(error)) => {
                if stop_limit_exceeded {
                    process.wait_for_completion().await;
                    return ProcessOutcome::Stopped(Err(PortError::HearingStopTimeout));
                }
                if stopping && closed_seen && matches!(error, InteractiveProcessError::Io(_)) {
                    continue;
                }
                return terminate_before_restart(
                    process,
                    &control,
                    Some(process_error(error)),
                    valid_final_seen,
                    closed_seen,
                    cancellation,
                    parent_cancellation,
                    logger,
                )
                .await;
            }
            None => {
                if stop_limit_exceeded {
                    process.wait_for_completion().await;
                    return ProcessOutcome::Stopped(Err(PortError::HearingStopTimeout));
                }
                return terminate_before_restart(
                    process,
                    &control,
                    Some(PortError::Unavailable(
                        "聴覚観察 helper が停止しました".to_owned(),
                    )),
                    valid_final_seen,
                    closed_seen,
                    cancellation,
                    parent_cancellation,
                    logger,
                )
                .await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn terminate_before_restart(
    process: &mut InteractiveProcess,
    control: &InteractiveProcessControl,
    restart_error: Option<PortError>,
    valid_final_seen: bool,
    mut closed_seen: bool,
    cancellation: &CancellationToken,
    parent_cancellation: &CancellationToken,
    logger: &dyn RuntimeLogger,
) -> ProcessOutcome {
    let termination_error = control.terminate(false).await.err().map(process_error);
    let mut wait_error = restart_error;
    let mut stopping = cancellation.is_cancelled() || parent_cancellation.is_cancelled();

    loop {
        let event = if stopping {
            process.next_event().await
        } else {
            tokio::select! {
                biased;
                event = process.next_event() => event,
                _ = cancellation.cancelled() => {
                    stopping = true;
                    continue;
                }
                _ = parent_cancellation.cancelled() => {
                    stopping = true;
                    continue;
                }
            }
        };
        if cancellation.is_cancelled() || parent_cancellation.is_cancelled() {
            stopping = true;
        }
        match event {
            Some(Ok(InteractiveProcessEvent::Exited { status, .. })) => {
                process.wait_for_completion().await;
                if stopping {
                    return ProcessOutcome::Stopped(
                        stop_exit_error(status, closed_seen)
                            .or(wait_error)
                            .map_or(Ok(()), Err),
                    );
                }
                return ProcessOutcome::Restart {
                    error: wait_error,
                    reset_restart_budget: valid_final_seen,
                };
            }
            Some(Ok(InteractiveProcessEvent::StdoutLine(line))) => {
                closed_seen |= decode_hearing_event(&line)
                    .ok()
                    .is_some_and(|decoded| matches!(decoded.into_parts().0, HearingEvent::Closed));
            }
            Some(Ok(InteractiveProcessEvent::StderrLine(line))) => {
                log_helper_stderr(logger, &line);
            }
            Some(Err(error)) => {
                if wait_error.is_none() {
                    wait_error = Some(process_error(error));
                }
            }
            None => {
                let error = wait_error.or(termination_error).unwrap_or_else(|| {
                    PortError::Unavailable(
                        "聴覚観察 helper の終了を確認できませんでした".to_owned(),
                    )
                });
                return ProcessOutcome::Stopped(Err(error));
            }
        }
    }
}

fn invalid_protocol_error(
    source: AudioObservationSource,
    speaker_message: &str,
    generic_message: &str,
) -> PortError {
    if source == AudioObservationSource::Speaker {
        PortError::SpeakerProtocol(speaker_message.to_owned())
    } else {
        PortError::Protocol(generic_message.to_owned())
    }
}

async fn send_source_event(
    events: &mpsc::Sender<SourceProcessEvent>,
    source: AudioObservationSource,
    kind: SourceProcessEventKind,
) -> bool {
    events
        .send(SourceProcessEvent { source, kind })
        .await
        .is_ok()
}

async fn cancel_and_reap(
    process: &mut InteractiveProcess,
    control: &InteractiveProcessControl,
    logger: &dyn RuntimeLogger,
) -> Result<(), PortError> {
    let mut wait_error = control
        .write_line(br#"{"op":"cancel"}"#.to_vec())
        .await
        .err()
        .map(process_error);
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
    let mut stop_limit_exceeded = false;
    let mut closed_seen = false;
    loop {
        let event = if stop_limit_exceeded {
            process.next_event().await
        } else {
            match tokio::time::timeout_at(deadline, process.next_event()).await {
                Ok(event) => event,
                Err(_) => {
                    stop_limit_exceeded = true;
                    if let Err(error) = control.terminate(false).await {
                        if wait_error.is_none() {
                            wait_error = Some(process_error(error));
                        }
                    }
                    continue;
                }
            }
        };
        match event {
            Some(Ok(InteractiveProcessEvent::Exited { status, .. })) => {
                process.wait_for_completion().await;
                if !closed_seen {
                    let _ =
                        logger.write("ERROR", "聴覚観察 helper は closed を送らずに終了しました");
                }
                if stop_limit_exceeded {
                    return Err(PortError::HearingStopTimeout);
                }
                if let Some(error) = stop_exit_error(status, closed_seen) {
                    return Err(error);
                }
                return wait_error.map_or(Ok(()), Err);
            }
            Some(Ok(InteractiveProcessEvent::StdoutLine(line))) => {
                if decode_hearing_event(&line)
                    .ok()
                    .is_some_and(|decoded| matches!(decoded.into_parts().0, HearingEvent::Closed))
                {
                    closed_seen = true;
                }
            }
            Some(Ok(InteractiveProcessEvent::StderrLine(line))) => {
                log_helper_stderr(logger, &line);
            }
            Some(Err(error)) => {
                if wait_error.is_none() {
                    wait_error = Some(process_error(error));
                }
            }
            None => {
                if stop_limit_exceeded {
                    process.wait_for_completion().await;
                    return Err(PortError::HearingStopTimeout);
                }
                return Err(wait_error.unwrap_or_else(|| {
                    PortError::Unavailable(
                        "聴覚観察 helper の終了を確認できませんでした".to_owned(),
                    )
                }));
            }
        }
    }
}

fn restart_delay(attempt: u8) -> std::time::Duration {
    std::time::Duration::from_millis(match attempt {
        1 => 250,
        2 => 1_000,
        _ => 4_000,
    })
}

pub(crate) fn process_error(error: impl std::fmt::Display) -> PortError {
    PortError::Unavailable(format!("聴覚観察 helper の実行に失敗しました: {error}"))
}

fn helper_exit_error(stderr: &[u8]) -> PortError {
    let detail = String::from_utf8_lossy(stderr)
        .trim()
        .chars()
        .take(300)
        .collect::<String>();
    let message = if detail.is_empty() {
        "聴覚観察 helper が異常終了しました".to_owned()
    } else {
        format!("聴覚観察 helper が異常終了しました: {detail}")
    };
    PortError::Unavailable(message)
}

fn log_helper_stderr(logger: &dyn RuntimeLogger, line: &[u8]) {
    let message = String::from_utf8_lossy(line).trim().to_owned();
    if message.is_empty() {
        return;
    }
    let _ = logger.write("INFO", &format!("聴覚観察 helper stderr: {message}"));
}
