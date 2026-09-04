use async_trait::async_trait;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use rustix::process::{kill_process_group, test_kill_process_group, Pid, Signal};

const STDOUT_LIMIT: usize = 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_secs(1);
const PROCESS_GROUP_POLL: Duration = Duration::from_millis(20);

static ACTIVE_PROCESS_GROUPS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct ProcessRequest {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// `None` は supervisor が空の一時 cwd を割り当てる。
    pub cwd: Option<PathBuf>,
    pub stdin: Vec<u8>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("provider process を起動できません")]
    Spawn(#[source] std::io::Error),
    #[error("provider process の I/O に失敗しました")]
    Io(#[source] std::io::Error),
    #[error("provider process の {stream} 出力が上限を超えました")]
    OutputLimit { stream: &'static str },
    #[error("provider process が timeout しました")]
    Timeout,
    #[error("provider process がキャンセルされました")]
    Cancelled,
}

#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutput, ProcessError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TokioProcessRunner;

#[async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        let mut command = Command::new(&request.executable);
        command
            .args(&request.args)
            .envs(request.env.iter().map(|(key, value)| (key, value)));
        let temp_cwd = if request.cwd.is_none() {
            Some(
                tempfile::Builder::new()
                    .prefix("coosenpai-provider-")
                    .tempdir()
                    .map_err(ProcessError::Spawn)?,
            )
        } else {
            None
        };
        match (request.cwd.as_ref(), temp_cwd.as_ref()) {
            (Some(cwd), _) => {
                command.current_dir(cwd);
            }
            (None, Some(cwd)) => {
                command.current_dir(cwd.path());
            }
            (None, None) => {
                return Err(ProcessError::Spawn(std::io::Error::other(
                    "provider process の cwd を準備できません",
                )));
            }
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        if cancellation.is_cancelled() {
            return Err(ProcessError::Cancelled);
        }
        let mut child = command.spawn().map_err(ProcessError::Spawn)?;
        let pid = child.id();
        let mut active_process_group = ActiveProcessGroup::register(pid);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io(std::io::Error::other("stdout pipe がありません")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProcessError::Io(std::io::Error::other("stderr pipe がありません")))?;
        let stdin = child.stdin.take();
        let stdout_task = tokio::spawn(read_limited(stdout, STDOUT_LIMIT, "stdout"));
        let stderr_task = tokio::spawn(read_limited(stderr, STDERR_LIMIT, "stderr"));
        let stdin_task = tokio::spawn(async move {
            if let Some(mut stdin) = stdin {
                stdin
                    .write_all(&request.stdin)
                    .await
                    .map_err(ProcessError::Io)?;
                stdin.shutdown().await.map_err(ProcessError::Io)?;
            }
            Ok::<(), ProcessError>(())
        });
        let mut stdin_task = Box::pin(stdin_task);
        tokio::pin!(stdout_task);
        tokio::pin!(stderr_task);
        let mut wait = Box::pin(child.wait());
        let timeout = tokio::time::sleep(request.timeout);
        tokio::pin!(timeout);
        let mut stdin_result = None;
        let mut stdout_result = None;
        let mut stderr_result = None;
        let mut status: Option<i32> = None;
        let mut wait_result: Option<Result<std::process::ExitStatus, std::io::Error>> = None;
        let mut failure = None;

        while failure.is_none() {
            tokio::select! {
                biased;
                result = &mut stdin_task, if stdin_result.is_none() => {
                    let result = match result {
                        Ok(result) => result,
                        Err(_) => Err(ProcessError::Io(std::io::Error::other("stdin writer が停止しました"))),
                    };
                    if let Err(error) = &result {
                        failure = Some(match error {
                            ProcessError::Io(error) => ProcessError::Io(std::io::Error::other(error.to_string())),
                            other => ProcessError::Io(std::io::Error::other(other.to_string())),
                        });
                        terminate_process_group(pid, false);
                    }
                    stdin_result = Some(result);
                }
                result = &mut stdout_task, if stdout_result.is_none() => {
                    let result = match result {
                        Ok(result) => result,
                        Err(_) => Err(ProcessError::Io(std::io::Error::other("stdout reader が停止しました"))),
                    };
                    if let Err(error) = &result {
                        failure = Some(match error {
                            ProcessError::OutputLimit { stream } => ProcessError::OutputLimit { stream },
                            ProcessError::Io(error) => ProcessError::Io(std::io::Error::other(error.to_string())),
                            other => ProcessError::Io(std::io::Error::other(other.to_string())),
                        });
                        terminate_process_group(pid, false);
                    }
                    stdout_result = Some(result);
                }
                result = &mut stderr_task, if stderr_result.is_none() => {
                    let result = match result {
                        Ok(result) => result,
                        Err(_) => Err(ProcessError::Io(std::io::Error::other("stderr reader が停止しました"))),
                    };
                    if let Err(error) = &result {
                        failure = Some(match error {
                            ProcessError::OutputLimit { stream } => ProcessError::OutputLimit { stream },
                            ProcessError::Io(error) => ProcessError::Io(std::io::Error::other(error.to_string())),
                            other => ProcessError::Io(std::io::Error::other(other.to_string())),
                        });
                        terminate_process_group(pid, false);
                    }
                    stderr_result = Some(result);
                }
                result = &mut wait, if status.is_none() => {
                    wait_result = Some(result);
                    break;
                }
                _ = &mut timeout => {
                    failure = Some(ProcessError::Timeout);
                    terminate_process_group(pid, false);
                    break;
                }
                _ = cancellation.cancelled() => {
                    failure = Some(ProcessError::Cancelled);
                    terminate_process_group(pid, false);
                }
            }
        }

        if status.is_none() && wait_result.is_none() {
            match tokio::time::timeout(TERMINATION_GRACE, &mut wait).await {
                Ok(result) => wait_result = Some(result),
                Err(_) => {
                    terminate_process_group(pid, true);
                    wait_result = Some(wait.await);
                }
            }
        }
        if status.is_none() {
            if let Some(result) = wait_result {
                match result {
                    Ok(result) => {
                        let code = result.code().unwrap_or(-1);
                        status = Some(code);
                    }
                    Err(error) => {
                        if failure.is_none() {
                            failure = Some(ProcessError::Io(error));
                        }
                    }
                }
            }
        }

        // 親が正常終了しても、同じ process group に残った server/worker を回収する。
        cleanup_process_group(pid).await;
        active_process_group.disarm();
        if stdin_result.is_none() {
            stdin_task.await.map_err(|_| {
                ProcessError::Io(std::io::Error::other("stdin writer が停止しました"))
            })??;
        }
        if stdout_result.is_none() {
            stdout_result = Some(stdout_task.await.map_err(|_| {
                ProcessError::Io(std::io::Error::other("stdout reader が停止しました"))
            })?);
        }
        if stderr_result.is_none() {
            stderr_result = Some(stderr_task.await.map_err(|_| {
                ProcessError::Io(std::io::Error::other("stderr reader が停止しました"))
            })?);
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let stdout = stdout_result.ok_or_else(|| {
            ProcessError::Io(std::io::Error::other("stdout reader の結果がありません"))
        })??;
        let stderr = stderr_result.ok_or_else(|| {
            ProcessError::Io(std::io::Error::other("stderr reader の結果がありません"))
        })??;
        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
        })
    }
}

pub fn force_kill_provider_processes() {
    let process_groups = {
        let registry = ACTIVE_PROCESS_GROUPS.get_or_init(|| Mutex::new(HashSet::new()));
        let guard = match registry.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.iter().copied().collect::<Vec<_>>()
    };
    for pid in process_groups {
        terminate_process_group(Some(pid), true);
    }
}

pub(crate) struct ActiveProcessGroup {
    pid: Option<u32>,
}

impl ActiveProcessGroup {
    pub(crate) fn register(pid: Option<u32>) -> Self {
        if let Some(pid) = pid {
            let registry = ACTIVE_PROCESS_GROUPS.get_or_init(|| Mutex::new(HashSet::new()));
            let mut guard = match registry.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.insert(pid);
        }
        Self { pid }
    }

    pub(crate) fn disarm(&mut self) {
        let Some(pid) = self.pid.take() else { return };
        unregister_process_group(pid);
    }
}

impl Drop for ActiveProcessGroup {
    fn drop(&mut self) {
        let Some(pid) = self.pid.take() else { return };
        terminate_process_group(Some(pid), true);
        unregister_process_group(pid);
    }
}

fn unregister_process_group(pid: u32) {
    let registry = ACTIVE_PROCESS_GROUPS.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = match registry.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.remove(&pid);
}

async fn read_limited<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    stream: &'static str,
) -> Result<Vec<u8>, ProcessError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await.map_err(ProcessError::Io)?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > limit {
            return Err(ProcessError::OutputLimit { stream });
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

pub(crate) fn terminate_process_group(pid: Option<u32>, force: bool) {
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    {
        let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
            return;
        };
        let signal = if force { Signal::KILL } else { Signal::TERM };
        // process_group(0) で子プロセス自身を group leader にするため、group 宛てに終了要求を送る。
        let _ = kill_process_group(pid, signal);
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, force);
    }
}

pub(crate) async fn cleanup_process_group(pid: Option<u32>) {
    #[cfg(unix)]
    {
        let Some(pid) = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(Pid::from_raw)
        else {
            return;
        };
        let _ = kill_process_group(pid, Signal::TERM);
        if wait_for_process_group_exit(pid, TERMINATION_GRACE).await {
            return;
        }
        let _ = kill_process_group(pid, Signal::KILL);
        let _ = wait_for_process_group_exit(pid, TERMINATION_GRACE).await;
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

#[cfg(unix)]
async fn wait_for_process_group_exit(pid: Pid, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match test_kill_process_group(pid) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::SRCH => return true,
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(PROCESS_GROUP_POLL).await;
    }
}

