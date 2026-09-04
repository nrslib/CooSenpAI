use crate::state::DesktopState;
use anyhow::Error;
use coosenpai_core::ports::RuntimeLogger;
use coosenpai_core::runtime::RuntimeError;
use coosenpai_core::watch_coordinator::RetryBackoff;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_CONSECUTIVE_FAILURES: u32 = 5;
const RECOVERABLE_ERROR_DISPLAY_ATTEMPT: u32 = 3;
const RECOVERABLE_ERROR_DISPLAY_GRACE: Duration = Duration::from_secs(10);

#[async_trait::async_trait]
trait WatchErrorPublisher: Send + Sync {
    async fn publish_recoverable_error(&self, detail: String, cancellation: CancellationToken);
    async fn publish_exhausted_error(&self, detail: String);
}

#[async_trait::async_trait]
impl WatchErrorPublisher for DesktopState {
    async fn publish_recoverable_error(&self, detail: String, cancellation: CancellationToken) {
        self.publish(|snapshot| {
            if !cancellation.is_cancelled() {
                snapshot.observer.record_recoverable_error(detail);
            }
        })
        .await;
    }

    async fn publish_exhausted_error(&self, detail: String) {
        self.publish(|snapshot| snapshot.observer.record_error(detail))
            .await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WatchRecoveryDecision {
    Retry,
    Stop,
    Cancelled,
    ConfigUpdateCancelled,
}

#[derive(Debug, Default)]
pub(super) struct WatchRecovery {
    consecutive_failures: u32,
    backoff: RetryBackoff,
    pending_error_presentation: Option<PendingErrorPresentation>,
}

#[derive(Debug)]
struct PendingErrorPresentation {
    cancellation: CancellationToken,
    detail: Arc<Mutex<String>>,
}

impl WatchRecovery {
    pub(super) fn reset(&mut self) {
        self.cancel_pending_error_presentation();
        self.consecutive_failures = 0;
        self.backoff.reset();
    }

    pub(super) fn next_delay(&mut self) -> Option<Duration> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures > MAX_CONSECUTIVE_FAILURES {
            return None;
        }
        let now = Instant::now();
        self.backoff.defer(now);
        self.backoff
            .next_attempt()
            .map(|deadline| deadline.saturating_duration_since(now))
    }

    fn should_present_immediately(&self) -> bool {
        self.consecutive_failures >= RECOVERABLE_ERROR_DISPLAY_ATTEMPT
    }

    fn schedule_error_presentation(
        &mut self,
        publisher: Arc<dyn WatchErrorPublisher>,
        detail: String,
    ) {
        if let Some(pending) = &self.pending_error_presentation {
            *pending
                .detail
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = detail;
            return;
        }
        let cancellation = CancellationToken::new();
        let latest_detail = Arc::new(Mutex::new(detail));
        self.pending_error_presentation = Some(PendingErrorPresentation {
            cancellation: cancellation.clone(),
            detail: latest_detail.clone(),
        });
        let task_cancellation = cancellation.clone();
        let _task = tokio::spawn(async move {
            tokio::select! {
                _ = task_cancellation.cancelled() => {}
                _ = tokio::time::sleep(RECOVERABLE_ERROR_DISPLAY_GRACE) => {
                    if task_cancellation.is_cancelled() {
                        return;
                    }
                    let detail = latest_detail
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    if !task_cancellation.is_cancelled() {
                        publisher
                            .publish_recoverable_error(detail, task_cancellation.clone())
                            .await;
                    }
                }
            }
        });
    }

    fn cancel_pending_error_presentation(&mut self) {
        if let Some(pending) = self.pending_error_presentation.take() {
            pending.cancellation.cancel();
        }
    }
}

impl Drop for WatchRecovery {
    fn drop(&mut self) {
        self.cancel_pending_error_presentation();
    }
}

pub(super) async fn record_watch_failure(
    state: &Arc<DesktopState>,
    recovery: &mut WatchRecovery,
    error: &Error,
    cancellation: &CancellationToken,
) -> WatchRecoveryDecision {
    let publisher: Arc<dyn WatchErrorPublisher> = state.clone();
    record_watch_failure_inner(
        recovery,
        error,
        cancellation,
        state.logger.as_ref(),
        publisher,
    )
    .await
}

async fn record_watch_failure_inner(
    recovery: &mut WatchRecovery,
    error: &Error,
    cancellation: &CancellationToken,
    logger: &dyn RuntimeLogger,
    publisher: Arc<dyn WatchErrorPublisher>,
) -> WatchRecoveryDecision {
    let detail = super::watch_error_detail(error);
    if is_config_update_cancellation(error) {
        let _ = logger.write(
            "INFO",
            &format!(
                "見守り: 設定反映に伴う provider キャンセルを処理しました: error-type=watch-config-cancellation error={detail}"
            ),
        );
        return WatchRecoveryDecision::ConfigUpdateCancelled;
    }

    let Some(delay) = recovery.next_delay() else {
        recovery.cancel_pending_error_presentation();
        let _ = logger.write(
            "ERROR",
            &format!("見守りを停止しました: error-type=watch recovery-exhausted error={detail}"),
        );
        publisher.publish_exhausted_error(detail).await;
        return WatchRecoveryDecision::Stop;
    };
    let attempt = recovery.consecutive_failures;
    let _ = logger.write(
        "WARN",
        &format!(
            "見守りで一時的なエラーが発生しました: error-type=watch attempt={attempt}/{MAX_CONSECUTIVE_FAILURES} retry-ms={} error={detail}",
            delay.as_millis()
        ),
    );
    if recovery.should_present_immediately() {
        recovery.cancel_pending_error_presentation();
        publisher
            .publish_recoverable_error(detail, CancellationToken::new())
            .await;
    } else {
        recovery.schedule_error_presentation(publisher, detail);
    }
    tokio::select! {
        _ = cancellation.cancelled() => WatchRecoveryDecision::Cancelled,
        _ = tokio::time::sleep(delay) => WatchRecoveryDecision::Retry,
    }
}

fn is_config_update_cancellation(error: &Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<RuntimeError>().is_some_and(|error| {
            matches!(
                error,
                RuntimeError::ConfigUpdateCancelled | RuntimeError::ProviderStartsBlocked
            )
        })
    })
}

