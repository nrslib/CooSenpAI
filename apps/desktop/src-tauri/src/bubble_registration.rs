use super::{
    schedule_expiry, sync_window, BubblePresentation, BubblePresentationOutcome, BubbleRecord,
    BubbleState,
};
use crate::state::DesktopState;
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;

const ACK_ATTEMPTS: usize = 3;
const ACK_TIMEOUT: Duration = Duration::from_millis(750);

pub async fn show(
    state: Arc<DesktopState>,
    record: BubbleRecord,
    duration_ms: u64,
) -> Result<BubblePresentationOutcome> {
    let presentation = register(&state, record, duration_ms).await?;
    complete_presentation(state, presentation).await
}

pub(crate) async fn show_replacing(
    state: Arc<DesktopState>,
    record: BubbleRecord,
    duration_ms: u64,
    replaced_ids: &[String],
    cancellation: &CancellationToken,
) -> Result<BubblePresentationOutcome> {
    let bubble_surface_active =
        !(state.main_window_focused.load(Ordering::Acquire) && record.interaction.is_none());
    let removed = bubble_surface_active && super::complete_actions(&state, replaced_ids).await;
    if removed && !super::wait_for_tutorial_bubble_transition(cancellation).await {
        return Ok(BubblePresentationOutcome::Dismissed);
    }
    if !replaced_ids.is_empty() && cancellation.is_cancelled() {
        return Ok(BubblePresentationOutcome::Dismissed);
    }
    let presentation = register(&state, record, duration_ms).await?;
    complete_presentation(state, presentation).await
}

pub(crate) async fn register(
    state: &DesktopState,
    record: BubbleRecord,
    duration_ms: u64,
) -> Result<BubblePresentation> {
    register_replacing(state, record, duration_ms, &[]).await
}

async fn register_replacing(
    state: &DesktopState,
    record: BubbleRecord,
    duration_ms: u64,
    replaced_ids: &[String],
) -> Result<BubblePresentation> {
    register_replacing_for_surface(
        &state.bubbles,
        &state.main_window_focused,
        record,
        Duration::from_millis(duration_ms),
        state.runtime_config().bubble.max_stack,
        replaced_ids,
    )
    .await
}

pub(crate) async fn register_replacing_for_surface(
    bubbles: &Mutex<BubbleState>,
    main_window_focused: &AtomicBool,
    record: BubbleRecord,
    duration: Duration,
    max_stack: usize,
    replaced_ids: &[String],
) -> Result<BubblePresentation> {
    let id = record.id.clone();
    let mut bubbles = bubbles.lock().await;
    if main_window_focused.load(Ordering::Acquire) && record.interaction.is_none() {
        return Ok(acknowledged_in_main());
    }
    let shown = if replaced_ids.is_empty() {
        bubbles.show(record, Instant::now(), duration, max_stack)
    } else {
        bubbles.show_replacing(record, Instant::now(), duration, max_stack, replaced_ids)
    };
    if !shown {
        anyhow::bail!("会話リセット前の吹き出しです");
    }
    let dismissed = bubbles
        .presentation_cancellation(&id)
        .context("登録した吹き出しの表示状態がありません")?;
    Ok(BubblePresentation {
        generation: bubbles.generation,
        acknowledgements: bubbles.subscribe_acknowledgements(),
        dismissed,
        registered_on_bubble_surface: true,
    })
}

fn acknowledged_in_main() -> BubblePresentation {
    let (_, acknowledgements) = watch::channel(0);
    BubblePresentation {
        generation: 0,
        acknowledgements,
        dismissed: CancellationToken::new(),
        registered_on_bubble_surface: false,
    }
}

pub(crate) async fn complete_presentation(
    state: Arc<DesktopState>,
    mut presentation: BubblePresentation,
) -> Result<BubblePresentationOutcome> {
    if !presentation.registered_on_bubble_surface {
        return Ok(BubblePresentationOutcome::Acknowledged);
    }
    for _ in 0..ACK_ATTEMPTS {
        sync_window(&state).await?;
        if let Some(outcome) = wait_for_presentation_completion(
            &mut presentation.acknowledgements,
            presentation.generation,
            &presentation.dismissed,
        )
        .await
        {
            if outcome == BubblePresentationOutcome::Acknowledged {
                schedule_expiry(state).await;
            }
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
    let Ok(presentation) = register(&state, record, duration_ms).await else {
        return false;
    };
    if !presentation.registered_on_bubble_surface {
        return true;
    }
    let _ = sync_window(&state).await;
    schedule_expiry(state).await;
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
