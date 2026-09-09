use super::{sync_window, BubblePresentation, BubblePresentationOutcome, BubbleRecord};
use crate::state::DesktopState;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const ACK_ATTEMPTS: usize = 3;
const ACK_TIMEOUT: Duration = Duration::from_millis(750);

pub async fn show(
    state: Arc<DesktopState>,
    record: BubbleRecord,
    duration_ms: u64,
) -> Result<BubblePresentationOutcome> {
    let presentation = register(&state.ui, record, duration_ms).await?;
    complete_presentation(state, presentation).await
}

pub(crate) async fn show_replacing(
    state: Arc<DesktopState>,
    record: BubbleRecord,
    duration_ms: u64,
    replaced_ids: &[String],
    cancellation: &CancellationToken,
) -> Result<BubblePresentationOutcome> {
    if cancellation.is_cancelled() {
        return Ok(BubblePresentationOutcome::Dismissed);
    }
    let presentation = register_replacing(&state.ui, record, duration_ms, replaced_ids).await?;
    complete_presentation(state, presentation).await
}

pub(crate) async fn register(
    ui: &crate::ui_root::UiHandle,
    record: BubbleRecord,
    duration_ms: u64,
) -> Result<BubblePresentation> {
    register_replacing(ui, record, duration_ms, &[]).await
}

async fn register_replacing(
    ui: &crate::ui_root::UiHandle,
    record: BubbleRecord,
    duration_ms: u64,
    replaced_ids: &[String],
) -> Result<BubblePresentation> {
    let (reply, response) = tokio::sync::oneshot::channel();
    ui.request(
        crate::ui_events::UiView::Application,
        crate::ui_events::UiEvent::BubbleRequested {
            record: Box::new(record),
            duration_ms,
            replaced_ids: replaced_ids.to_vec(),
            reply,
        },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    response.await.context("吹き出しの受付が終了しました")?
}

pub(crate) async fn complete_presentation(
    state: Arc<DesktopState>,
    presentation: BubblePresentation,
) -> Result<BubblePresentationOutcome> {
    await_presentation(&state.ui, presentation).await
}

pub(crate) async fn await_presentation(
    ui: &crate::ui_root::UiHandle,
    mut presentation: BubblePresentation,
) -> Result<BubblePresentationOutcome> {
    if !presentation.registered_on_bubble_surface {
        return Ok(BubblePresentationOutcome::Acknowledged);
    }
    for _ in 0..ACK_ATTEMPTS {
        ui.request(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::BubbleRefresh,
        )
        .await
        .map_err(anyhow::Error::msg)?;
        if let Some(outcome) = wait_for_presentation_completion(
            &mut presentation.acknowledgements,
            presentation.generation,
            &presentation.dismissed,
        )
        .await
        {
            return Ok(outcome);
        }
    }
    anyhow::bail!("吹き出しrendererから表示確認がありません")
}

pub async fn show_best_effort(
    state: Arc<DesktopState>,
    record: BubbleRecord,
    duration_ms: u64,
) -> bool {
    let Ok(presentation) = register(&state.ui, record, duration_ms).await else {
        return false;
    };
    if !presentation.registered_on_bubble_surface {
        return true;
    }
    let _ = sync_window(&state).await;
    true
}

pub(crate) async fn wait_for_presentation_completion(
    acknowledgements: &mut watch::Receiver<u64>,
    generation: u64,
    dismissed: &CancellationToken,
) -> Option<BubblePresentationOutcome> {
    tokio::select! {
        biased;
        () = dismissed.cancelled() => Some(BubblePresentationOutcome::Dismissed),
        acknowledged = wait_for_acknowledgement(acknowledgements, generation) => {
            acknowledged.then_some(BubblePresentationOutcome::Acknowledged)
        }
    }
}

pub(crate) async fn wait_for_acknowledgement(
    acknowledgements: &mut watch::Receiver<u64>,
    generation: u64,
) -> bool {
    if *acknowledgements.borrow() >= generation {
        return true;
    }
    matches!(
        tokio::time::timeout(
            ACK_TIMEOUT,
            acknowledgements.wait_for(|value| *value >= generation)
        )
        .await,
        Ok(Ok(_))
    )
}
