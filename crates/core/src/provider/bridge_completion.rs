use super::{invalid_output, retryable, BridgeState, Pending, PendingKind};
use crate::provider::{ProviderCapabilities, ProviderError, ProviderResult};
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub(super) enum BridgeReply {
    Capabilities(ProviderCapabilities),
    Result(ProviderResult),
    Ack,
}

pub(super) fn complete_pending(pending: Pending, result: Result<BridgeReply, ProviderError>) {
    match pending.kind {
        PendingKind::Open(response) => {
            let result = result.and_then(capabilities_reply);
            let _ = response.send(result);
        }
        PendingKind::Resolve(response) => {
            let result = result.and_then(capabilities_reply);
            let _ = response.send(result);
        }
        PendingKind::Send { response, .. } => {
            let result = result.and_then(|reply| match reply {
                BridgeReply::Result(value) => Ok(value),
                BridgeReply::Capabilities(_) | BridgeReply::Ack => {
                    Err(invalid_output("provider 応答が不正です。"))
                }
            });
            let _ = response.send(result);
        }
        PendingKind::Ack(response) => {
            let result = result.and_then(|reply| match reply {
                BridgeReply::Ack => Ok(()),
                BridgeReply::Capabilities(_) | BridgeReply::Result(_) => {
                    Err(invalid_output("provider bridge の ack が不正です。"))
                }
            });
            let _ = response.send(result);
        }
    }
}

fn capabilities_reply(reply: BridgeReply) -> Result<ProviderCapabilities, ProviderError> {
    match reply {
        BridgeReply::Capabilities(value) => Ok(value),
        BridgeReply::Result(_) | BridgeReply::Ack => {
            Err(invalid_output("capabilities 応答が不正です。"))
        }
    }
}

pub(super) fn fail_generation_once(state: &mut BridgeState, generation: u64) -> bool {
    if state.failed_generation == Some(generation) {
        return false;
    }
    state.failed_generation = Some(generation);
    if let Some((wait_generation, wait)) = state.wait_completion.as_ref() {
        if *wait_generation == generation && !*wait.borrow() {
            state.reaping = Some((generation, wait.clone()));
        }
    }
    state.stdin = None;
    state.pid = None;
    for (_, request) in std::mem::take(&mut state.pending) {
        if request.generation == generation {
            complete_pending(
                request,
                Err(retryable("provider bridge が異常終了しました。")),
            );
        }
    }
    state.restart_failures = state.restart_failures.saturating_add(1).min(5);
    let seconds = 1u64 << state.restart_failures.saturating_sub(1);
    state.restart_not_before = Some(Instant::now() + Duration::from_secs(seconds.min(30)));
    true
}

pub(super) fn mark_reaping(state: &mut BridgeState) {
    if let Some((generation, wait)) = state.wait_completion.as_ref() {
        if !*wait.borrow() {
            state.reaping = Some((*generation, wait.clone()));
        }
    }
}

pub(super) fn fail_pending(state: &mut BridgeState, error: ProviderError) {
    for (_, pending) in std::mem::take(&mut state.pending) {
        complete_pending(
            pending,
            Err(ProviderError {
                kind: error.kind,
                message: error.message.clone(),
            }),
        );
    }
}

pub(super) async fn wait_for_reap(wait: &mut watch::Receiver<bool>, timeout: Duration) -> bool {
    if *wait.borrow() {
        return true;
    }
    tokio::time::timeout(timeout, wait.wait_for(|completed| *completed))
        .await
        .is_ok_and(|result| result.is_ok())
}
