use std::future::Future;
use std::io;
use std::time::Duration;
use tokio::signal::unix::{signal, Signal, SignalKind};
use tokio_util::sync::CancellationToken;

pub(crate) const WATCH_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) enum BootstrapOutcome<T> {
    Started { value: T, signals: ShutdownSignals },
    Cancelled,
}

pub(crate) struct ShutdownSignals {
    cancellation: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ShutdownSignals {
    fn install() -> io::Result<Self> {
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        let cancellation = CancellationToken::new();
        let first_signal = cancellation.clone();
        let task = tokio::spawn(async move {
            if receive_signal(&mut interrupt, &mut terminate)
                .await
                .is_none()
            {
                return;
            }
            first_signal.cancel();
            if receive_signal(&mut interrupt, &mut terminate)
                .await
                .is_some()
            {
                coosenpai_core::process::force_kill_provider_processes();
                std::process::exit(0);
            }
        });
        Ok(Self {
            cancellation,
            task: Some(task),
        })
    }

    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub(crate) async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(crate) async fn bootstrap<T, F, Fut>(start: F) -> io::Result<BootstrapOutcome<T>>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = T>,
{
    let signals = ShutdownSignals::install()?;
    let cancellation = signals.cancellation();
    let bootstrap = start(cancellation.clone());
    tokio::pin!(bootstrap);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Ok(BootstrapOutcome::Cancelled),
        value = &mut bootstrap => Ok(BootstrapOutcome::Started { value, signals }),
    }
}

pub(crate) async fn wait_for_cleanup<F>(cleanup: F, timeout: Duration) -> Option<F::Output>
where
    F: Future,
{
    match tokio::time::timeout(timeout, cleanup).await {
        Ok(result) => Some(result),
        Err(_) => {
            coosenpai_core::process::force_kill_provider_processes();
            None
        }
    }
}

async fn receive_signal(interrupt: &mut Signal, terminate: &mut Signal) -> Option<()> {
    tokio::select! {
        signal = interrupt.recv() => signal,
        signal = terminate.recv() => signal,
    }
}
