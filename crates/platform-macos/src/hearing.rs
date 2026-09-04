use async_trait::async_trait;
use coosenpai_core::interactive_process::{
    InteractiveProcess, InteractiveProcessEvent, InteractiveProcessRequest,
};
use coosenpai_core::ports::{
    HearingCommand, HearingEvent, HearingPort, HearingSession, PortError, RuntimeLogger,
};
use coosenpai_core::state::AudioObservationSource;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct MacHearing {
    helper: PathBuf,
    logger: Arc<dyn RuntimeLogger>,
}

impl MacHearing {
    pub fn new(helper: PathBuf, logger: Arc<dyn RuntimeLogger>) -> Self {
        Self { helper, logger }
    }
}

#[async_trait]
impl HearingPort for MacHearing {
    async fn start(
        &self,
        locale: &str,
        input_device: &str,
        sources: Vec<AudioObservationSource>,
        debug_dump_dir: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<HearingSession, PortError> {
        if sources.is_empty() {
            return Err(PortError::Unavailable(
                "聴覚観察の入力源が選択されていません".to_owned(),
            ));
        }
        let process = InteractiveProcess::spawn(
            InteractiveProcessRequest {
                executable: self.helper.clone(),
                args: helper_arguments(locale, input_device, &sources, debug_dump_dir),
                env: Vec::new(),
                cwd: None,
            },
            cancellation,
        )
        .await
        .map_err(process_error)?;
        let (command_tx, command_rx) = mpsc::channel(4);
        let (event_tx, event_rx) = mpsc::channel(64);
        let cancel_requested = CancellationToken::new();
        tokio::spawn(run_session(
            process,
            command_rx,
            event_tx,
            cancel_requested.clone(),
            self.logger.clone(),
        ));
        Ok(HearingSession::from_channels_with_cancellation(
            command_tx,
            event_rx,
            cancel_requested,
        ))
    }
}

async fn run_session(
    mut process: InteractiveProcess,
    mut commands: mpsc::Receiver<HearingCommand>,
    events: mpsc::Sender<Result<HearingEvent, PortError>>,
    cancel_requested: CancellationToken,
    logger: Arc<dyn RuntimeLogger>,
) {
    let control = process.control();
    let mut closed_sent = false;
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(HearingCommand::Cancel { completed }) => {
                    let result = cancel_and_reap(&mut process, &control, logger.as_ref()).await;
                    if !closed_sent {
                        let _ = send_hearing_event(
                            &events,
                            Ok(HearingEvent::Closed),
                            &cancel_requested,
                        )
                        .await;
                    }
                    let _ = completed.send(result);
                    return;
                }
                None => {
                    let _ = cancel_and_reap(&mut process, &control, logger.as_ref()).await;
                    return;
                }
            },
            event = process.next_event() => match event {
                Some(Ok(InteractiveProcessEvent::StdoutLine(line))) => {
                    match serde_json::from_slice::<HearingEvent>(&line) {
                        Ok(event) => {
                            let closed = matches!(&event, HearingEvent::Closed);
                            if closed {
                                closed_sent = true;
                            }
                            if !send_hearing_event(&events, Ok(event), &cancel_requested).await {
                                if cancel_requested.is_cancelled() {
                                    continue;
                                }
                                return;
                            }
                            if closed {
                                return;
                            }
                        }
                        Err(_) => {
                            let _ = send_hearing_event(
                                &events,
                                Err(PortError::Unavailable(
                                    "聴覚観察 helper が不正な応答を返しました".to_owned(),
                                )),
                                &cancel_requested,
                            )
                            .await;
                            if cancel_requested.is_cancelled() {
                                continue;
                            }
                            let _ = control.terminate(false).await;
                            return;
                        }
                    }
                }
                Some(Ok(InteractiveProcessEvent::StderrLine(line))) => {
                    log_helper_stderr(logger.as_ref(), &line);
                }
                Some(Ok(InteractiveProcessEvent::Exited { status, stderr })) => {
                    if !closed_sent {
                        if status == Some(0) {
                            let _ = send_hearing_event(
                                &events,
                                Ok(HearingEvent::Closed),
                                &cancel_requested,
                            )
                            .await;
                        } else {
                            let detail = String::from_utf8_lossy(&stderr);
                            let detail = detail.trim().chars().take(300).collect::<String>();
                            let message = if detail.is_empty() {
                                "聴覚観察 helper が異常終了しました".to_owned()
                            } else {
                                format!("聴覚観察 helper が異常終了しました: {detail}")
                            };
                            let _ = send_hearing_event(
                                &events,
                                Err(PortError::Unavailable(message)),
                                &cancel_requested,
                            )
                            .await;
                        }
                    }
                    return;
                }
                Some(Err(error)) => {
                    let _ = send_hearing_event(
                        &events,
                        Err(process_error(error)),
                        &cancel_requested,
                    )
                    .await;
                    if cancel_requested.is_cancelled() {
                        continue;
                    }
                    return;
                }
                None if cancel_requested.is_cancelled() => {
                    if let Some(HearingCommand::Cancel { completed }) = commands.recv().await {
                        let result = cancel_and_reap(&mut process, &control, logger.as_ref()).await;
                        let _ = completed.send(result);
                    }
                    return;
                }
                None => return,
            }
        }
    }
}

async fn send_hearing_event(
    events: &mpsc::Sender<Result<HearingEvent, PortError>>,
    event: Result<HearingEvent, PortError>,
    cancel_requested: &CancellationToken,
) -> bool {
    tokio::select! {
        result = events.send(event) => result.is_ok(),
        _ = cancel_requested.cancelled() => false,
    }
}

async fn cancel_and_reap(
    process: &mut InteractiveProcess,
    control: &coosenpai_core::interactive_process::InteractiveProcessControl,
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

fn helper_arguments(
    locale: &str,
    input_device: &str,
    _sources: &[AudioObservationSource],
    debug_dump_dir: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--locale".to_owned(),
        locale.to_owned(),
        "--input-device".to_owned(),
        input_device.to_owned(),
        "--sources".to_owned(),
        "speaker".to_owned(),
    ];
    if let Some(directory) = debug_dump_dir {
        args.push("--debug-dump-appended".to_owned());
        args.push(directory.to_owned());
    }
    args
}

fn process_error(error: impl std::fmt::Display) -> PortError {
    PortError::Unavailable(format!("聴覚観察 helper の実行に失敗しました: {error}"))
}

fn log_helper_stderr(logger: &dyn RuntimeLogger, line: &[u8]) {
    let message = String::from_utf8_lossy(line).trim().to_owned();
    if message.is_empty() {
        return;
    }
    let _ = logger.write("INFO", &format!("聴覚観察 helper stderr: {message}"));
}

