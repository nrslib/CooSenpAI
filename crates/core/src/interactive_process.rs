use crate::process::{cleanup_process_group, terminate_process_group, ActiveProcessGroup};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const LINE_LIMIT: usize = 256 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct InteractiveProcessRequest {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractiveProcessEvent {
    StdoutLine(Vec<u8>),
    StderrLine(Vec<u8>),
    Exited {
        status: Option<i32>,
        stderr: Vec<u8>,
    },
}

#[derive(Debug, Error)]
pub enum InteractiveProcessError {
    #[error("interactive process を起動できません")]
    Spawn(#[source] std::io::Error),
    #[error("interactive process の I/O に失敗しました")]
    Io(#[source] std::io::Error),
    #[error("interactive process の出力が上限を超えました")]
    OutputLimit,
    #[error("interactive process が停止しています")]
    Closed,
    #[error("interactive process がキャンセルされました")]
    Cancelled,
}

enum ProcessCommand {
    Write(Vec<u8>),
    Terminate { force: bool },
}

#[derive(Clone)]
pub struct InteractiveProcessControl {
    commands: mpsc::Sender<ProcessCommand>,
    termination_requested: CancellationToken,
}

pub struct InteractiveProcess {
    control: InteractiveProcessControl,
    events: mpsc::Receiver<Result<InteractiveProcessEvent, InteractiveProcessError>>,
}

impl InteractiveProcess {
    pub async fn spawn(
        request: InteractiveProcessRequest,
        cancellation: CancellationToken,
    ) -> Result<Self, InteractiveProcessError> {
        if cancellation.is_cancelled() {
            return Err(InteractiveProcessError::Cancelled);
        }
        let mut command = Command::new(&request.executable);
        command
            .args(&request.args)
            .envs(request.env.iter().map(|(key, value)| (key, value)))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let temporary_cwd = if request.cwd.is_none() {
            Some(
                tempfile::Builder::new()
                    .prefix("coosenpai-process-")
                    .tempdir()
                    .map_err(InteractiveProcessError::Spawn)?,
            )
        } else {
            None
        };
        if let Some(cwd) = request.cwd.as_deref() {
            command.current_dir(cwd);
        } else if let Some(cwd) = temporary_cwd.as_ref() {
            command.current_dir(cwd.path());
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(InteractiveProcessError::Spawn)?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or_else(|| {
            InteractiveProcessError::Io(std::io::Error::other("stdin pipe がありません"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            InteractiveProcessError::Io(std::io::Error::other("stdout pipe がありません"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            InteractiveProcessError::Io(std::io::Error::other("stderr pipe がありません"))
        })?;
        let (command_tx, command_rx) = mpsc::channel(16);
        let (event_tx, event_rx) = mpsc::channel(32);
        let termination_requested = CancellationToken::new();
        tokio::spawn(run_process(
            child,
            stdin,
            stdout,
            stderr,
            pid,
            command_rx,
            event_tx,
            cancellation,
            termination_requested.clone(),
            temporary_cwd,
        ));
        Ok(Self {
            control: InteractiveProcessControl {
                commands: command_tx,
                termination_requested,
            },
            events: event_rx,
        })
    }

    pub fn control(&self) -> InteractiveProcessControl {
        self.control.clone()
    }

    pub async fn next_event(
        &mut self,
    ) -> Option<Result<InteractiveProcessEvent, InteractiveProcessError>> {
        self.events.recv().await
    }
}

impl InteractiveProcessControl {
    pub async fn write_line(&self, mut line: Vec<u8>) -> Result<(), InteractiveProcessError> {
        if line.len() > LINE_LIMIT {
            return Err(InteractiveProcessError::OutputLimit);
        }
        if !line.ends_with(b"\n") {
            line.push(b'\n');
        }
        self.commands
            .send(ProcessCommand::Write(line))
            .await
            .map_err(|_| InteractiveProcessError::Closed)
    }

    pub async fn terminate(&self, force: bool) -> Result<(), InteractiveProcessError> {
        self.termination_requested.cancel();
        self.commands
            .send(ProcessCommand::Terminate { force })
            .await
            .map_err(|_| InteractiveProcessError::Closed)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_process(
    mut child: tokio::process::Child,
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    pid: Option<u32>,
    mut commands: mpsc::Receiver<ProcessCommand>,
    events: mpsc::Sender<Result<InteractiveProcessEvent, InteractiveProcessError>>,
    cancellation: CancellationToken,
    termination_requested: CancellationToken,
    _temporary_cwd: Option<tempfile::TempDir>,
) {
    let mut process_group = ActiveProcessGroup::register(pid);
    let mut lines = BufReader::new(stdout).lines();
    let mut stderr = stderr;
    let mut stderr_buffer = [0_u8; 4096];
    let mut stderr_pending = Vec::new();
    let mut stderr_output = Vec::new();
    let mut status = None;
    let mut stdout_open = true;
    let mut stderr_open = true;
    loop {
        tokio::select! {
            biased;
            line = lines.next_line(), if stdout_open => match line {
                Ok(Some(line)) if line.len() <= LINE_LIMIT => {
                    if !send_process_event(
                        &events,
                        Ok(InteractiveProcessEvent::StdoutLine(line.into_bytes())),
                        &cancellation,
                        &termination_requested,
                    ).await {
                        terminate_process_group(pid, false);
                        break;
                    }
                }
                Ok(Some(_)) => {
                    let _ = send_process_event(
                        &events,
                        Err(InteractiveProcessError::OutputLimit),
                        &cancellation,
                        &termination_requested,
                    ).await;
                    terminate_process_group(pid, false);
                    break;
                }
                Ok(None) => stdout_open = false,
                Err(error) => {
                    let _ = send_process_event(
                        &events,
                        Err(InteractiveProcessError::Io(error)),
                        &cancellation,
                        &termination_requested,
                    ).await;
                    terminate_process_group(pid, false);
                    break;
                }
            },
            result = read_stderr_chunk(
                &mut stderr,
                &mut stderr_buffer,
                &mut stderr_output,
                &mut stderr_pending,
                &events,
                &cancellation,
                &termination_requested,
            ), if stderr_open => match result {
                Ok(true) => {}
                Ok(false) => stderr_open = false,
                Err(error) => {
                    let _ = send_process_event(
                        &events,
                        Err(error),
                        &cancellation,
                        &termination_requested,
                    ).await;
                    terminate_process_group(pid, false);
                    break;
                }
            },
            instruction = commands.recv() => match instruction {
                Some(ProcessCommand::Write(bytes)) => {
                    if let Err(error) = stdin.write_all(&bytes).await {
                        let _ = send_process_event(
                            &events,
                            Err(InteractiveProcessError::Io(error)),
                            &cancellation,
                            &termination_requested,
                        ).await;
                        terminate_process_group(pid, false);
                        break;
                    }
                    if let Err(error) = stdin.flush().await {
                        let _ = send_process_event(
                            &events,
                            Err(InteractiveProcessError::Io(error)),
                            &cancellation,
                            &termination_requested,
                        ).await;
                        terminate_process_group(pid, false);
                        break;
                    }
                }
                Some(ProcessCommand::Terminate { force }) => {
                    terminate_process_group(pid, force);
                    break;
                }
                None => {
                    terminate_process_group(pid, false);
                    break;
                }
            },
            result = child.wait() => {
                status = result.ok().map(|value| value.code().unwrap_or(-1));
                break;
            }
            _ = cancellation.cancelled() => {
                terminate_process_group(pid, false);
                break;
            }
            _ = termination_requested.cancelled() => {
                terminate_process_group(pid, false);
                break;
            }
        }
    }
    if status.is_none() {
        status = match tokio::time::timeout(TERMINATION_GRACE, child.wait()).await {
            Ok(Ok(value)) => Some(value.code().unwrap_or(-1)),
            _ => {
                terminate_process_group(pid, true);
                child
                    .wait()
                    .await
                    .ok()
                    .map(|value| value.code().unwrap_or(-1))
            }
        };
    }
    while stderr_open {
        match read_stderr_chunk(
            &mut stderr,
            &mut stderr_buffer,
            &mut stderr_output,
            &mut stderr_pending,
            &events,
            &cancellation,
            &termination_requested,
        )
        .await
        {
            Ok(open) => stderr_open = open,
            Err(_) => break,
        }
    }
    cleanup_process_group(pid).await;
    process_group.disarm();
    let _ = events
        .send(Ok(InteractiveProcessEvent::Exited {
            status,
            stderr: stderr_output,
        }))
        .await;
}

async fn read_stderr_chunk(
    reader: &mut tokio::process::ChildStderr,
    buffer: &mut [u8],
    output: &mut Vec<u8>,
    pending: &mut Vec<u8>,
    events: &mpsc::Sender<Result<InteractiveProcessEvent, InteractiveProcessError>>,
    cancellation: &CancellationToken,
    termination_requested: &CancellationToken,
) -> Result<bool, InteractiveProcessError> {
    let count = reader
        .read(buffer)
        .await
        .map_err(InteractiveProcessError::Io)?;
    if count == 0 {
        if !pending.is_empty() {
            let line = std::mem::take(pending);
            if !send_process_event(
                events,
                Ok(InteractiveProcessEvent::StderrLine(line)),
                cancellation,
                termination_requested,
            )
            .await
            {
                return Err(InteractiveProcessError::Closed);
            }
        }
        return Ok(false);
    }
    output.extend_from_slice(&buffer[..count]);
    let excess = output.len().saturating_sub(STDERR_LIMIT);
    if excess > 0 {
        output.drain(..excess);
    }
    pending.extend_from_slice(&buffer[..count]);
    if pending.len() > STDERR_LIMIT {
        return Err(InteractiveProcessError::OutputLimit);
    }
    while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
        let line = pending.drain(..=index).collect::<Vec<_>>();
        if !send_process_event(
            events,
            Ok(InteractiveProcessEvent::StderrLine(line)),
            cancellation,
            termination_requested,
        )
        .await
        {
            return Err(InteractiveProcessError::Closed);
        }
    }
    Ok(true)
}

async fn send_process_event(
    events: &mpsc::Sender<Result<InteractiveProcessEvent, InteractiveProcessError>>,
    event: Result<InteractiveProcessEvent, InteractiveProcessError>,
    cancellation: &CancellationToken,
    termination_requested: &CancellationToken,
) -> bool {
    tokio::select! {
        result = events.send(event) => result.is_ok(),
        _ = cancellation.cancelled() => false,
        _ = termination_requested.cancelled() => false,
    }
}

