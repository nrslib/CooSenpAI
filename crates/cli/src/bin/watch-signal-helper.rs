#![cfg(unix)]

#[path = "../shutdown.rs"]
mod shutdown;

use coosenpai_core::process::{ProcessError, ProcessRequest, ProcessRunner, TokioProcessRunner};
use std::future::pending;
use std::path::PathBuf;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(mode) = arguments.next() else {
        std::process::exit(2);
    };
    let Some(path) = arguments.next().map(PathBuf::from) else {
        std::process::exit(2);
    };
    let result = match mode.as_str() {
        "--bootstrap" => run_bootstrap(path).await,
        "--graceful" => run_graceful(path).await,
        "--timeout" => run_timeout(path).await,
        "--second-signal" => run_second_signal(path).await,
        "--child" => run_child(path).await,
        _ => std::process::exit(2),
    };
    if result.is_err() {
        std::process::exit(1);
    }
}

async fn run_bootstrap(ready_path: PathBuf) -> Result<(), ProcessError> {
    let outcome = shutdown::bootstrap(|cancellation| run_provider(ready_path, cancellation))
        .await
        .map_err(ProcessError::Io)?;
    match outcome {
        shutdown::BootstrapOutcome::Cancelled => Ok(()),
        shutdown::BootstrapOutcome::Started { .. } => Err(ProcessError::Io(std::io::Error::other(
            "bootstrap provider が signal 前に終了しました",
        ))),
    }
}

async fn run_graceful(ready_path: PathBuf) -> Result<(), ProcessError> {
    let watch_ready_path = watch_ready_path(&ready_path);
    let outcome = shutdown::bootstrap(|cancellation| async move {
        start_provider(ready_path, cancellation).await
    })
    .await
    .map_err(ProcessError::Io)?;
    let shutdown::BootstrapOutcome::Started { value, signals } = outcome else {
        return Ok(());
    };
    std::fs::write(watch_ready_path, b"started").map_err(ProcessError::Io)?;
    let result = join_provider(value?).await;
    signals.stop().await;
    match result {
        Err(ProcessError::Cancelled) => Ok(()),
        Ok(_) => Err(ProcessError::Io(std::io::Error::other(
            "helper child が signal 前に終了しました",
        ))),
        Err(error) => Err(error),
    }
}

async fn run_timeout(ready_path: PathBuf) -> Result<(), ProcessError> {
    let cleanup_path = cleanup_started_path(&ready_path);
    let watch_ready_path = watch_ready_path(&ready_path);
    let outcome = shutdown::bootstrap(|_| async move {
        start_provider(ready_path, CancellationToken::new()).await
    })
    .await
    .map_err(ProcessError::Io)?;
    let shutdown::BootstrapOutcome::Started { value, signals } = outcome else {
        return Ok(());
    };
    let provider = value?;
    std::fs::write(watch_ready_path, b"started").map_err(ProcessError::Io)?;
    signals.cancellation().cancelled().await;
    std::fs::write(cleanup_path, b"started").map_err(ProcessError::Io)?;
    let timed_out =
        shutdown::wait_for_cleanup(pending::<()>(), shutdown::WATCH_SHUTDOWN_TIMEOUT).await;
    if timed_out.is_some() {
        return Err(ProcessError::Io(std::io::Error::other(
            "cleanup が timeout しませんでした",
        )));
    }
    let _ = join_provider(provider).await;
    signals.stop().await;
    Ok(())
}

async fn run_second_signal(ready_path: PathBuf) -> Result<(), ProcessError> {
    let cleanup_path = cleanup_started_path(&ready_path);
    let watch_ready_path = watch_ready_path(&ready_path);
    let outcome = shutdown::bootstrap(|cancellation| async move {
        start_provider(ready_path, cancellation).await
    })
    .await
    .map_err(ProcessError::Io)?;
    let shutdown::BootstrapOutcome::Started { value, signals } = outcome else {
        return Ok(());
    };
    let _provider = value?;
    std::fs::write(watch_ready_path, b"started").map_err(ProcessError::Io)?;
    signals.cancellation().cancelled().await;
    std::fs::write(cleanup_path, b"started").map_err(ProcessError::Io)?;
    pending::<()>().await;
    Ok(())
}

async fn start_provider(
    ready_path: PathBuf,
    cancellation: CancellationToken,
) -> Result<JoinHandle<Result<coosenpai_core::process::ProcessOutput, ProcessError>>, ProcessError>
{
    let provider_path = ready_path.clone();
    let provider = tokio::spawn(async move { run_provider(provider_path, cancellation).await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !ready_path.exists() {
        if tokio::time::Instant::now() >= deadline {
            provider.abort();
            return Err(ProcessError::Io(std::io::Error::other(
                "provider ready が timeout しました",
            )));
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    Ok(provider)
}

fn cleanup_started_path(ready_path: &std::path::Path) -> PathBuf {
    ready_path.with_file_name("cleanup-started")
}

fn watch_ready_path(ready_path: &std::path::Path) -> PathBuf {
    ready_path.with_file_name("watch-ready")
}

async fn run_provider(
    ready_path: PathBuf,
    cancellation: CancellationToken,
) -> Result<coosenpai_core::process::ProcessOutput, ProcessError> {
    TokioProcessRunner
        .run(
            ProcessRequest {
                executable: std::env::current_exe().map_err(ProcessError::Io)?,
                args: vec!["--child".to_owned(), ready_path.display().to_string()],
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout: Duration::from_secs(60),
            },
            cancellation,
        )
        .await
}

async fn join_provider(
    provider: JoinHandle<Result<coosenpai_core::process::ProcessOutput, ProcessError>>,
) -> Result<coosenpai_core::process::ProcessOutput, ProcessError> {
    provider
        .await
        .map_err(|error| ProcessError::Io(std::io::Error::other(error.to_string())))?
}

async fn run_child(ready_path: PathBuf) -> Result<(), ProcessError> {
    let mut terminate = signal(SignalKind::terminate()).map_err(ProcessError::Io)?;
    tokio::spawn(async move { while terminate.recv().await.is_some() {} });
    std::fs::write(ready_path, std::process::id().to_string()).map_err(ProcessError::Io)?;
    tokio::time::sleep(Duration::from_secs(60)).await;
    Ok(())
}
