use super::source_name;
use coosenpai_core::interactive_process::{
    InteractiveProcess, InteractiveProcessControl, InteractiveProcessEvent,
    InteractiveProcessRequest,
};
use coosenpai_core::ports::{HearingEvent, PortError, RuntimeLogger};
use coosenpai_core::state::AudioObservationSource;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MAX_RESTART_ATTEMPTS: u8 = 3;

pub(crate) struct SourceProcessSpec {
    pub(crate) source: AudioObservationSource,
    pub(crate) executable: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) initial_process: Option<InteractiveProcess>,
    pub(crate) initial_error: Option<PortError>,
    pub(crate) parent_cancellation: CancellationToken,
    pub(crate) logger: Arc<dyn RuntimeLogger>,
}

pub(crate) struct SourceProcessEvent {
    pub(crate) source: AudioObservationSource,
    pub(crate) kind: SourceProcessEventKind,
}

pub(crate) enum SourceProcessEventKind {
    Restarting,
    Event(HearingEvent),
    Unavailable,
    Exhausted { error: Option<PortError> },
}

enum ProcessOutcome {
    Restart {
        error: Option<PortError>,
        ready_seen: bool,
    },
    Stopped(Result<(), PortError>),
}

pub(crate) async fn run_source_process(
    mut spec: SourceProcessSpec,
    events: mpsc::Sender<SourceProcessEvent>,
    cancellation: CancellationToken,
) -> Result<(), PortError> {
    let mut restart_attempts = 0;
    let mut initial_process = spec.initial_process.take();
    let mut initial_error = spec.initial_error.take();

    loop {
        if cancellation.is_cancelled() || spec.parent_cancellation.is_cancelled() {
            if let Some(mut process) = initial_process.take() {
                let control = process.control();
                return cancel_and_reap(&mut process, &control, spec.logger.as_ref()).await;
            }
            return Ok(());
        }
        if !send_source_event(
            &events,
            spec.source,
            SourceProcessEventKind::Restarting,
            &cancellation,
            &spec.parent_cancellation,
        )
        .await
        {
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
                    &events,
                    &cancellation,
                    &spec.parent_cancellation,
                    spec.logger.as_ref(),
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
            spec.logger.as_ref(),
        )
        .await;
        match outcome {
            ProcessOutcome::Stopped(result) => return result,
            ProcessOutcome::Restart { error, ready_seen } => {
                if ready_seen {
                    restart_attempts = 0;
                }
                if !schedule_restart(
                    spec.source,
                    error,
                    &mut restart_attempts,
                    &events,
                    &cancellation,
                    &spec.parent_cancellation,
                    spec.logger.as_ref(),
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
    InteractiveProcess::spawn(
        InteractiveProcessRequest {
            executable: spec.executable.clone(),
            args: spec.args.clone(),
            env: Vec::new(),
            cwd: None,
        },
        spec.parent_cancellation.clone(),
    )
    .await
    .map_err(process_error)
}

async fn schedule_restart(
    source: AudioObservationSource,
    error: Option<PortError>,
    restart_attempts: &mut u8,
    events: &mpsc::Sender<SourceProcessEvent>,
    cancellation: &CancellationToken,
    parent_cancellation: &CancellationToken,
    logger: &dyn RuntimeLogger,
) -> bool {
    if !send_source_event(
        events,
        source,
        SourceProcessEventKind::Unavailable,
        cancellation,
        parent_cancellation,
    )
    .await
    {
        return false;
    }
    if let Some(error) = error.as_ref() {
        let _ = logger.write(
            "WARN",
            &format!(
                "聴覚観察 source-local process failure: source={} error={error}",
                source_name(source)
            ),
        );
    }
    if *restart_attempts >= MAX_RESTART_ATTEMPTS {
        let _ = send_source_event(
            events,
            source,
            SourceProcessEventKind::Exhausted { error },
            cancellation,
            parent_cancellation,
        )
        .await;
        return false;
    }
    *restart_attempts += 1;
    let delay = restart_delay(*restart_attempts);
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        _ = cancellation.cancelled() => false,
        _ = parent_cancellation.cancelled() => false,
    }
}

async fn monitor_process(
    source: AudioObservationSource,
    process: &mut InteractiveProcess,
    events: &mpsc::Sender<SourceProcessEvent>,
    cancellation: &CancellationToken,
    parent_cancellation: &CancellationToken,
    logger: &dyn RuntimeLogger,
) -> ProcessOutcome {
    let control = process.control();
    let mut ready_seen = false;
    loop {
        let event = tokio::select! {
            biased;
            event = process.next_event() => event,
            _ = cancellation.cancelled() => {
                return ProcessOutcome::Stopped(
                    cancel_and_reap(process, &control, logger).await,
                );
            }
            _ = parent_cancellation.cancelled() => {
                return ProcessOutcome::Stopped(
                    cancel_and_reap(process, &control, logger).await,
                );
            }
        };
        match event {
            Some(Ok(InteractiveProcessEvent::StdoutLine(line))) => {
                match serde_json::from_slice::<HearingEvent>(&line) {
                    Ok(event @ HearingEvent::Ready { .. }) => {
                        ready_seen = true;
                        if !send_source_event(
                            events,
                            source,
                            SourceProcessEventKind::Event(event),
                            cancellation,
                            parent_cancellation,
                        )
                        .await
                        {
                            return ProcessOutcome::Stopped(
                                cancel_and_reap(process, &control, logger).await,
                            );
                        }
                    }
                    Ok(HearingEvent::Error { ref kind, .. }) if kind == "no-input-source" => {}
                    Ok(HearingEvent::Closed) => {
                        return terminate_before_restart(
                            process,
                            &control,
                            None,
                            ready_seen,
                            cancellation,
                            parent_cancellation,
                            logger,
                        )
                        .await;
                    }
                    Ok(event) => {
                        if !send_source_event(
                            events,
                            source,
                            SourceProcessEventKind::Event(event),
                            cancellation,
                            parent_cancellation,
                        )
                        .await
                        {
                            return ProcessOutcome::Stopped(
                                cancel_and_reap(process, &control, logger).await,
                            );
                        }
                    }
                    Err(_) => {
                        let error = PortError::Unavailable(
                            "聴覚観察 helper が不正な応答を返しました".to_owned(),
                        );
                        return terminate_before_restart(
                            process,
                            &control,
                            Some(error),
                            ready_seen,
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
                if cancellation.is_cancelled() || parent_cancellation.is_cancelled() {
                    return ProcessOutcome::Stopped(Ok(()));
                }
                let error = (status != Some(0)).then(|| helper_exit_error(&stderr));
                return ProcessOutcome::Restart { error, ready_seen };
            }
            Some(Err(error)) => {
                return terminate_before_restart(
                    process,
                    &control,
                    Some(process_error(error)),
                    ready_seen,
                    cancellation,
                    parent_cancellation,
                    logger,
                )
                .await;
            }
            None => {
                return terminate_before_restart(
                    process,
                    &control,
                    Some(PortError::Unavailable(
                        "聴覚観察 helper が停止しました".to_owned(),
                    )),
                    ready_seen,
                    cancellation,
                    parent_cancellation,
                    logger,
                )
                .await;
            }
        }
    }
}

async fn terminate_before_restart(
    process: &mut InteractiveProcess,
    control: &InteractiveProcessControl,
    restart_error: Option<PortError>,
    ready_seen: bool,
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
            Some(Ok(InteractiveProcessEvent::Exited { .. })) => {
                if stopping {
                    return ProcessOutcome::Stopped(Ok(()));
                }
                return ProcessOutcome::Restart {
                    error: wait_error,
                    ready_seen,
                };
            }
            Some(Ok(InteractiveProcessEvent::StdoutLine(_))) => {}
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

async fn send_source_event(
    events: &mpsc::Sender<SourceProcessEvent>,
    source: AudioObservationSource,
    kind: SourceProcessEventKind,
    cancellation: &CancellationToken,
    parent_cancellation: &CancellationToken,
) -> bool {
    tokio::select! {
        result = events.send(SourceProcessEvent { source, kind }) => result.is_ok(),
        _ = cancellation.cancelled() => false,
        _ = parent_cancellation.cancelled() => false,
    }
}

async fn cancel_and_reap(
    process: &mut InteractiveProcess,
    control: &InteractiveProcessControl,
    logger: &dyn RuntimeLogger,
) -> Result<(), PortError> {
    let _ = control.write_line(br#"{"op":"cancel"}"#.to_vec()).await;
    control.terminate(false).await.map_err(process_error)?;
    while let Some(event) = process.next_event().await {
        match event {
            Ok(InteractiveProcessEvent::Exited { .. }) => return Ok(()),
            Ok(InteractiveProcessEvent::StdoutLine(_)) => {}
            Ok(InteractiveProcessEvent::StderrLine(line)) => log_helper_stderr(logger, &line),
            Err(error) => return Err(process_error(error)),
        }
    }
    Ok(())
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
