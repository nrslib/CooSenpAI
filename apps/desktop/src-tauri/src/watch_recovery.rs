use crate::state::DesktopState;
use crate::watch_presenter::WatchResult;
use anyhow::Error;
use coosenpai_core::ports::RuntimeLogger;
use coosenpai_core::runtime::RuntimeError;
use coosenpai_core::watch_coordinator::RetryBackoff;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_CONSECUTIVE_FAILURES: u32 = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum WatchSessionKind {
    #[default]
    Normal,
    Tutorial,
}

impl WatchSessionKind {
    pub(super) fn for_tutorial(tutorial_active: bool) -> Self {
        if tutorial_active {
            Self::Tutorial
        } else {
            Self::Normal
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait WatchErrorPublisher: Send + Sync {
    async fn observe_watch_failure(
        &self,
        generation: u64,
        attempt: u32,
        detail: String,
        tutorial: bool,
        cancellation: CancellationToken,
    );
    async fn observe_watch_exit(&self, generation: u64, detail: String, tutorial: bool) -> bool;
}

#[async_trait::async_trait]
impl WatchErrorPublisher for DesktopState {
    async fn observe_watch_failure(
        &self,
        generation: u64,
        attempt: u32,
        detail: String,
        tutorial: bool,
        cancellation: CancellationToken,
    ) {
        self.publish_watch_view(
            generation,
            WatchResult::FailureObserved {
                attempt,
                detail,
                tutorial,
                cancellation,
            },
        )
        .await;
    }

    async fn observe_watch_exit(&self, generation: u64, detail: String, tutorial: bool) -> bool {
        match self
            .ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::ui_events::UiEvent::SnapshotCompleted(Box::new(
                    crate::snapshot_presenter::SnapshotEvent::Watch {
                        generation,
                        event: WatchResult::ExitObserved {
                            detail,
                            tutorial,
                            reply,
                        },
                    },
                ))
            })
            .await
        {
            Ok(failed) => failed,
            Err(error) => {
                let _ = self.logger.write("WARN", &error);
                false
            }
        }
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
    session_kind: WatchSessionKind,
    consecutive_failures: u32,
    backoff: RetryBackoff,
}

impl WatchRecovery {
    pub(super) fn new(session_kind: WatchSessionKind) -> Self {
        Self {
            session_kind,
            ..Default::default()
        }
    }
    pub(super) fn reset(&mut self) {
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
}

pub(super) async fn record_watch_failure<H: super::WatchHost>(
    state: &Arc<H>,
    recovery: &mut WatchRecovery,
    error: &Error,
    generation: u64,
    cancellation: &CancellationToken,
) -> WatchRecoveryDecision {
    record_watch_failure_inner(
        recovery,
        error,
        generation,
        cancellation,
        state.logger(),
        state.as_ref(),
    )
    .await
}

async fn record_watch_failure_inner(
    recovery: &mut WatchRecovery,
    error: &Error,
    generation: u64,
    cancellation: &CancellationToken,
    logger: &dyn RuntimeLogger,
    publisher: &dyn WatchErrorPublisher,
) -> WatchRecoveryDecision {
    let detail = super::watch_error_detail(error);
    if is_config_update_cancellation(error) {
        let _ = logger.write("INFO", &format!("見守り: 設定反映に伴う provider キャンセルを処理しました: error-type=watch-config-cancellation error={detail}"));
        return WatchRecoveryDecision::ConfigUpdateCancelled;
    }
    let tutorial = recovery.session_kind == WatchSessionKind::Tutorial;
    if let Some(mailbox_error) = error
        .downcast_ref::<RuntimeError>()
        .and_then(RuntimeError::mailbox_error)
        .filter(|error| !error.is_retryable())
    {
        let _ = logger.write(
            "ERROR",
            &format!("見守りを停止しました: error-type=mailbox reason={} retryable=false action=stop error={detail}", mailbox_error.reason()),
        );
        publisher
            .observe_watch_exit(generation, detail, tutorial)
            .await;
        return WatchRecoveryDecision::Stop;
    }
    let Some(delay) = recovery.next_delay() else {
        let _ = logger.write(
            "ERROR",
            &format!("見守りを停止しました: error-type=watch recovery-exhausted error={detail}"),
        );
        publisher
            .observe_watch_exit(generation, detail, tutorial)
            .await;
        return WatchRecoveryDecision::Stop;
    };
    let attempt = recovery.consecutive_failures;
    let _ = logger.write("WARN", &format!("見守りで一時的なエラーが発生しました: error-type=watch attempt={attempt}/{MAX_CONSECUTIVE_FAILURES} retry-ms={} error={detail}", delay.as_millis()));
    publisher
        .observe_watch_failure(generation, attempt, detail, tutorial, cancellation.clone())
        .await;
    tokio::select! {
        _ = cancellation.cancelled() => WatchRecoveryDecision::Cancelled,
        _ = tokio::time::sleep(delay) => WatchRecoveryDecision::Retry,
    }
}

pub(super) async fn record_watch_exit<H: super::WatchHost>(
    state: &Arc<H>,
    generation: u64,
    result: Result<(), Error>,
    kind: WatchSessionKind,
) -> bool {
    let Err(error) = result else {
        return false;
    };
    let detail = super::watch_error_detail(&error);
    let _ = state.logger().write(
        "ERROR",
        &format!("見守りに失敗しました: error-type=watch error={detail}"),
    );
    state
        .observe_watch_exit(generation, detail, kind == WatchSessionKind::Tutorial)
        .await
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
