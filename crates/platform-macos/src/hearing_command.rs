use std::ffi::OsString;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum HearingHelperCommandError {
    #[error("hearing helper の実行が時間内に終わりません")]
    Timeout,
    #[error("hearing helper を実行できません")]
    Io(#[source] std::io::Error),
}

pub async fn run_hearing_helper_command(
    executable: &Path,
    arguments: &[OsString],
    limit: Duration,
) -> Result<Output, HearingHelperCommandError> {
    let mut command = tokio::process::Command::new(executable);
    command
        .args(arguments)
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .kill_on_drop(true);
    tokio::time::timeout(limit, command.output())
        .await
        .map_err(|_| HearingHelperCommandError::Timeout)?
        .map_err(HearingHelperCommandError::Io)
}
