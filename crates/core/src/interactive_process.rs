use crate::process::{cleanup_process_group, terminate_process_group, ActiveProcessGroup};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

const LINE_LIMIT: usize = 256 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_secs(1);
const FORCE_TERMINATION_WAIT: Duration = Duration::from_secs(1);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

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
    force_termination_requested: CancellationToken,
    pid: Option<u32>,
}

#[derive(Clone)]
pub(crate) struct ProcessCompletion {
    completed: watch::Sender<bool>,
}

impl ProcessCompletion {
    fn new() -> Self {
        let (completed, _) = watch::channel(false);
        Self { completed }
    }

    fn complete(&self) {
        self.completed.send_replace(true);
    }

    pub(crate) async fn wait(&self) {
        let mut completed = self.completed.subscribe();
        while !*completed.borrow_and_update() {
            completed
                .changed()
                .await
                .expect("ProcessCompletion の送信元は待機中に破棄されません");
        }
    }
}

pub struct InteractiveProcess {
    control: InteractiveProcessControl,
    events: mpsc::Receiver<Result<InteractiveProcessEvent, InteractiveProcessError>>,
    task: tokio::task::JoinHandle<()>,
    completion: ProcessCompletion,
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
        let force_termination_requested = CancellationToken::new();
        let completion = ProcessCompletion::new();
        let task = tokio::spawn(run_process(
            child,
            stdin,
            stdout,
            stderr,
            pid,
            command_rx,
            event_tx,
            cancellation,
            termination_requested.clone(),
            force_termination_requested.clone(),
            temporary_cwd,
            completion.clone(),
        ));
        Ok(Self {
            control: InteractiveProcessControl {
                commands: command_tx,
                termination_requested,
                force_termination_requested,
                pid,
            },
            events: event_rx,
            task,
            completion,
        })
    }

    pub fn control(&self) -> InteractiveProcessControl {
        self.control.clone()
    }

    pub(crate) fn completion(&self) -> ProcessCompletion {
        self.completion.clone()
    }

    pub async fn next_event(
        &mut self,
    ) -> Option<Result<InteractiveProcessEvent, InteractiveProcessError>> {
        self.events.recv().await
    }

    /// 子プロセスの終了を要求する。通常終了と強制終了の待ち時間には上限を設ける。
    ///
    /// `ProcessCompletion` は子プロセスの回収とプロセスグループの後始末まで完了した
    /// 時点で通知されるため、呼び出し側は新しいプロセスを起動する前にそれを待つ。
    pub async fn shutdown(self) {
        let control = self.control.clone();
        let mut task = self.task;
        let _ = control.terminate(false).await;
        let graceful = tokio::time::timeout(SHUTDOWN_GRACE, &mut task).await;
        if graceful.is_ok() {
            return;
        }
        let _ = control.terminate(true).await;
        let _ = tokio::time::timeout(FORCE_TERMINATION_WAIT, &mut task).await;
        if !task.is_finished() {
            tokio::spawn(async move {
                let _ = task.await;
            });
        }
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
        if force {
            self.force_termination_requested.cancel();
            terminate_process_group(self.pid, true);
        }
        match self.commands.try_send(ProcessCommand::Terminate { force }) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(InteractiveProcessError::Closed),
        }
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
    force_termination_requested: CancellationToken,
    _temporary_cwd: Option<tempfile::TempDir>,
    completion: ProcessCompletion,
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
                    let write_result = tokio::select! {
                        _ = force_termination_requested.cancelled() => {
                            terminate_process_group(pid, true);
                            break;
                        }
                        result = stdin.write_all(&bytes) => result,
                    };
                    if let Err(error) = write_result {
                        let _ = send_process_event(
                            &events,
                            Err(InteractiveProcessError::Io(error)),
                            &cancellation,
                            &termination_requested,
                        ).await;
                        terminate_process_group(pid, false);
                        break;
                    }
                    let flush_result = tokio::select! {
                        _ = force_termination_requested.cancelled() => {
                            terminate_process_group(pid, true);
                            break;
                        }
                        result = stdin.flush() => result,
                    };
                    if let Err(error) = flush_result {
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
            _ = force_termination_requested.cancelled() => {
                terminate_process_group(pid, true);
                break;
            }
        }
    }
    if status.is_none() {
        let exited = tokio::select! {
            result = child.wait() => Some(result.ok().map(|value| value.code().unwrap_or(-1))),
            _ = force_termination_requested.cancelled() => None,
            _ = tokio::time::sleep(TERMINATION_GRACE) => None,
        };
        status = match exited {
            Some(status) => status,
            None => {
                terminate_process_group(pid, true);
                wait_after_force_kill(&mut child, pid).await
            }
        };
    }
    let _ = tokio::time::timeout(TERMINATION_GRACE, async {
        while stderr_open {
            tokio::select! {
                _ = force_termination_requested.cancelled() => break,
                result = read_stderr_chunk(
                    &mut stderr,
                    &mut stderr_buffer,
                    &mut stderr_output,
                    &mut stderr_pending,
                    &events,
                    &cancellation,
                    &termination_requested,
                ) => match result {
                    Ok(open) => stderr_open = open,
                    Err(_) => break,
                }
            }
        }
    })
    .await;
    cleanup_process_group(pid).await;
    process_group.disarm();
    let _ = events.try_send(Ok(InteractiveProcessEvent::Exited {
        status,
        stderr: stderr_output,
    }));
    completion.complete();
}

async fn wait_after_force_kill(child: &mut tokio::process::Child, pid: Option<u32>) -> Option<i32> {
    match tokio::time::timeout(FORCE_TERMINATION_WAIT, child.wait()).await {
        Ok(Ok(value)) => Some(value.code().unwrap_or(-1)),
        _ => {
            let _ = tokio::time::timeout(FORCE_TERMINATION_WAIT, child.kill()).await;
            match tokio::time::timeout(FORCE_TERMINATION_WAIT, child.wait()).await {
                Ok(Ok(value)) => Some(value.code().unwrap_or(-1)),
                _ => {
                    terminate_process_group(pid, true);
                    None
                }
            }
        }
    }
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

