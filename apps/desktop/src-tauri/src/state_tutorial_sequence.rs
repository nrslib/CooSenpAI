use super::*;
use crate::tutorial_notice::TutorialBubbleOutcome;
use tokio_util::sync::CancellationToken;

impl DesktopState {
    pub(super) async fn emit_tutorial_intro_sequence(
        self: &Arc<Self>,
        follows_setup_ok: bool,
    ) -> Result<(), RuntimeError> {
        self.start_tutorial_sequence(crate::tutorial_sequence_presenter::SequenceKind::Intro {
            follows_setup_ok,
        })
        .await
        .map(|_| ())
    }

    pub(super) async fn emit_watch_intro_sequence(
        self: &Arc<Self>,
    ) -> Result<TutorialBubbleOutcome, RuntimeError> {
        self.start_tutorial_sequence(crate::tutorial_sequence_presenter::SequenceKind::Watch)
            .await
    }

    async fn start_tutorial_sequence(
        &self,
        kind: crate::tutorial_sequence_presenter::SequenceKind,
    ) -> Result<TutorialBubbleOutcome, RuntimeError> {
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::ui_events::UiEvent::Tutorial(Box::new(
                    crate::tutorial_events::TutorialEvent::Sequence(
                        crate::tutorial_sequence_presenter::SequenceEvent::Start { kind, reply },
                    ),
                ))
            })
            .await
            .map_err(RuntimeError::Factory)?
    }

    pub(super) async fn cancel_tutorial_sequence(&self) {
        let _ = self
            .ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::ui_events::UiEvent::Tutorial(Box::new(
                    crate::tutorial_events::TutorialEvent::Sequence(
                        crate::tutorial_sequence_presenter::SequenceEvent::Cancel(reply),
                    ),
                ))
            })
            .await;
    }

    pub(crate) async fn load_sequence_card(
        self: &Arc<Self>,
        key: &str,
        milestone: bubbles::BubbleMilestone,
        cancellation: CancellationToken,
    ) -> crate::tutorial_sequence_presenter::SequenceCard {
        let already_accepted = self
            .tutorial
            .lock()
            .await
            .state()
            .tutorial
            .notices
            .get(key)
            .is_some_and(|notice| notice.bubble_accepted);
        let outcome = self
            .emit_tutorial_message_replacing(key, Vec::new(), cancellation)
            .await;
        let details = {
            let tutorial = self.tutorial.lock().await;
            tutorial
                .state()
                .tutorial
                .notices
                .get(key)
                .ok_or_else(|| {
                    RuntimeError::Factory(format!("チュートリアル案内がありません: {key}"))
                })
                .and_then(|notice| {
                    tutorial
                        .state()
                        .tutorial_notice_id(key)
                        .map(|id| (id, notice.message.clone()))
                        .map_err(|error| RuntimeError::Factory(error.to_string()))
                })
        };
        let has_completion = if let Ok((id, _)) = &details {
            self.bubbles
                .lock()
                .await
                .card_completion(id, milestone)
                .is_some()
        } else {
            false
        };
        crate::tutorial_sequence_presenter::SequenceCard {
            outcome,
            details,
            already_accepted,
            has_completion,
        }
    }

    pub(super) async fn tutorial_notice_ids(
        &self,
        keys: &[&str],
    ) -> Result<Vec<String>, RuntimeError> {
        let tutorial = self.tutorial.lock().await;
        keys.iter()
            .map(|key| {
                tutorial
                    .state()
                    .tutorial_notice_id(key)
                    .map_err(|error| RuntimeError::Factory(error.to_string()))
            })
            .collect()
    }
}
