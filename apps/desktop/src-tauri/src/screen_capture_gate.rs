use crate::state::DesktopState;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub(crate) struct ScreenCaptureGate {
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl Default for ScreenCaptureGate {
    fn default() -> Self {
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

impl ScreenCaptureGate {
    pub(crate) async fn acquire(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("画面撮影の直列化ゲートは閉じません")
    }
}

pub(crate) async fn acquire_screen_capture_gate(
    state: &DesktopState,
    cancellation: &CancellationToken,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    tokio::select! {
        () = cancellation.cancelled() => None,
        permit = state.screen_capture_gate.acquire() => Some(permit),
    }
}
