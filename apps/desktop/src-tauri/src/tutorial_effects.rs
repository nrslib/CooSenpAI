use crate::state::DesktopState;
use crate::tutorial_events::{CardEvent, CardSignal, CardTask, TutorialEvent, TutorialTask};
use std::sync::Arc;

async fn run_card(state: Arc<DesktopState>, task: CardTask) -> CardEvent {
    match task {
        CardTask::ReadCard {
            id,
            notice_id,
            milestone,
        } => {
            let signals = state
                .bubbles
                .lock()
                .await
                .card_completion(&notice_id, milestone);
            CardEvent::CardLoaded { id, signals }
        }
        CardTask::ObserveCard {
            id,
            reached,
            dismissed,
            cancellation,
        } => {
            let signal = tokio::select! {
                biased;
                () = reached.cancelled() => CardSignal::Reached,
                () = dismissed.cancelled() => CardSignal::Dismissed,
                () = cancellation.cancelled() => CardSignal::Cancelled,
                () = state.cancellation.cancelled() => CardSignal::Cancelled,
            };
            CardEvent::CardObserved { id, signal }
        }
        CardTask::ObserveCancellation { id, cancellation } => {
            tokio::select! { () = cancellation.cancelled() => {}, () = state.cancellation.cancelled() => {} }
            CardEvent::CardObserved {
                id,
                signal: CardSignal::Cancelled,
            }
        }
    }
}

pub(crate) async fn run(state: Arc<DesktopState>, task: TutorialTask) -> TutorialEvent {
    match task {
        TutorialTask::Lifecycle(task) => {
            TutorialEvent::Lifecycle(state.run_tutorial_lifecycle_task(task).await)
        }
        TutorialTask::Response(task) => {
            TutorialEvent::Response(state.run_tutorial_response_task(task).await)
        }
        TutorialTask::Progress(task) => {
            TutorialEvent::Progress(state.run_tutorial_progress_task(task).await)
        }
        TutorialTask::Notice(task) => {
            use crate::tutorial_notice_presenter::{NoticeEvent, NoticeTask};
            let (result, reply) = match task {
                NoticeTask::Acknowledge {
                    record,
                    duration_ms,
                    replaced_bubble_ids,
                    cancellation,
                    reply,
                } => {
                    let result = crate::bubbles::show_replacing(
                        state,
                        *record,
                        duration_ms,
                        &replaced_bubble_ids,
                        &cancellation,
                    )
                    .await
                    .map(|outcome| match outcome {
                        crate::bubbles::BubblePresentationOutcome::Acknowledged => {
                            crate::tutorial_notice::TutorialBubbleOutcome::Acknowledged
                        }
                        crate::bubbles::BubblePresentationOutcome::Dismissed => {
                            crate::tutorial_notice::TutorialBubbleOutcome::Dismissed
                        }
                    })
                    .map_err(|error| {
                        coosenpai_core::runtime::RuntimeError::Factory(error.to_string())
                    });
                    (result, reply)
                }
                NoticeTask::BestEffort {
                    record,
                    duration_ms,
                    reply,
                } => {
                    crate::bubbles::show_best_effort(state, *record, duration_ms).await;
                    (
                        Ok(crate::tutorial_notice::TutorialBubbleOutcome::Acknowledged),
                        reply,
                    )
                }
            };
            TutorialEvent::Notice(NoticeEvent::Presented { result, reply })
        }
        TutorialTask::Card(task) => TutorialEvent::Card(run_card(state, task).await),
        TutorialTask::Sequence(task) => {
            use crate::tutorial_sequence_presenter::{SequenceEvent, SequenceTask};
            TutorialEvent::Sequence(match task {
                SequenceTask::Present {
                    generation,
                    key,
                    milestone,
                    cancellation,
                } => SequenceEvent::Presented {
                    generation,
                    card: state.load_sequence_card(key, milestone, cancellation).await,
                },
                SequenceTask::Wait {
                    generation,
                    notice_id,
                    milestone,
                    delay,
                    cancellation,
                } => SequenceEvent::Reached {
                    generation,
                    reached: crate::bubbles::wait_for_card_milestone_with_cancellation(
                        &state,
                        &notice_id,
                        milestone,
                        delay,
                        cancellation,
                    )
                    .await,
                },
            })
        }
    }
}
