use async_trait::async_trait;
use coosenpai_core::interactive_process::{
    InteractiveProcess, InteractiveProcessEvent, InteractiveProcessRequest,
};
use coosenpai_core::ports::{PortError, SpeechCommand, SpeechEvent, SpeechPort, SpeechSession};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct MacSpeech {
    helper: PathBuf,
}

impl MacSpeech {
    pub fn new(helper: PathBuf) -> Self {
        Self { helper }
    }
}

#[async_trait]
impl SpeechPort for MacSpeech {
    async fn start(
        &self,
        locale: &str,
        input_device: &str,
        cancellation: CancellationToken,
    ) -> Result<SpeechSession, PortError> {
        let process = InteractiveProcess::spawn(
            InteractiveProcessRequest {
                executable: self.helper.clone(),
                args: vec![
                    "--locale".to_owned(),
                    locale.to_owned(),
                    "--input-device".to_owned(),
                    input_device.to_owned(),
                ],
                env: Vec::new(),
                cwd: None,
            },
            cancellation,
        )
        .await
        .map_err(process_error)?;
        let (command_tx, command_rx) = mpsc::channel(4);
        let (event_tx, event_rx) = mpsc::channel(32);
        tokio::spawn(run_session(process, command_rx, event_tx));
        Ok(SpeechSession::from_channels(command_tx, event_rx))
    }
}

async fn run_session(
    mut process: InteractiveProcess,
    mut commands: mpsc::Receiver<SpeechCommand>,
    events: mpsc::Sender<Result<SpeechEvent, PortError>>,
) {
    let control = process.control();
    let mut terminal_event_seen = false;
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(SpeechCommand::Finish) => {
                    if let Err(error) = control.write_line(br#"{"op":"finish"}"#.to_vec()).await {
                        let _ = events.send(Err(process_error(error))).await;
                        let _ = control.terminate(false).await;
                        return;
                    }
                }
                Some(SpeechCommand::Cancel { completed }) => {
                    let result = cancel_and_reap(&mut process, &control).await;
                    let _ = events.send(Ok(SpeechEvent::Closed)).await;
                    let _ = completed.send(result);
                    return;
                }
                None => {
                    let _ = cancel_and_reap(&mut process, &control).await;
                    return;
                }
            },
            event = process.next_event() => match event {
                Some(Ok(InteractiveProcessEvent::StdoutLine(line))) => {
                    match serde_json::from_slice::<SpeechEvent>(&line) {
                        Ok(event) => {
                            let closed = matches!(&event, SpeechEvent::Closed);
                            let terminal = matches!(
                                &event,
                                SpeechEvent::Final { .. }
                                    | SpeechEvent::Error { .. }
                                    | SpeechEvent::Closed
                            );
                            if terminal_event_seen && !closed {
                                continue;
                            }
                            if terminal && !closed {
                                terminal_event_seen = true;
                            }
                            if events.send(Ok(event)).await.is_err() || closed {
                                return;
                            }
                        }
                        Err(_) => {
                            let _ = events.send(Err(PortError::Unavailable(
                                "音声認識 helper が不正な応答を返しました".to_owned(),
                            ))).await;
                            let _ = control.terminate(false).await;
                            return;
                        }
                    }
                }
                Some(Ok(InteractiveProcessEvent::StderrLine(_))) => {}
                Some(Ok(InteractiveProcessEvent::Exited { status, stderr })) => {
                    if status == Some(0) {
                        let _ = events.send(Ok(SpeechEvent::Closed)).await;
                    } else {
                        let detail = String::from_utf8_lossy(&stderr);
                        let detail = detail.trim().chars().take(300).collect::<String>();
                        let message = if detail.is_empty() {
                            "音声認識 helper が異常終了しました".to_owned()
                        } else {
                            format!("音声認識 helper が異常終了しました: {detail}")
                        };
                        let _ = events.send(Err(PortError::Unavailable(message))).await;
                    }
                    return;
                }
                Some(Err(error)) => {
                    let _ = events.send(Err(process_error(error))).await;
                    return;
                }
                None => return,
            }
        }
    }
}

async fn cancel_and_reap(
    process: &mut InteractiveProcess,
    control: &coosenpai_core::interactive_process::InteractiveProcessControl,
) -> Result<(), PortError> {
    let _ = control.write_line(br#"{"op":"cancel"}"#.to_vec()).await;
    control.terminate(false).await.map_err(process_error)?;
    while let Some(event) = process.next_event().await {
        match event {
            Ok(InteractiveProcessEvent::Exited { .. }) => return Ok(()),
            Ok(InteractiveProcessEvent::StdoutLine(_)) => {}
            Ok(InteractiveProcessEvent::StderrLine(_)) => {}
            Err(error) => return Err(process_error(error)),
        }
    }
    Ok(())
}

fn process_error(error: impl std::fmt::Display) -> PortError {
    PortError::Unavailable(format!("音声認識 helper の実行に失敗しました: {error}"))
}

