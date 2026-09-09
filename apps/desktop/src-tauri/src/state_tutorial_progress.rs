use super::*;
use coosenpai_core::companion_storage::CompanionStorage;
use coosenpai_core::onboarding::TutorialStep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TutorialResponseStatus {
    None,
    Pending,
    SavedForPresentation,
}

fn response_status_from_entries(
    conversation: &[coosenpai_core::state::ConversationEntry],
    expected: Option<&str>,
    pending: bool,
) -> TutorialResponseStatus {
    if expected.is_some_and(|expected| {
        conversation.iter().any(|entry| {
            entry.role == coosenpai_core::state::ConversationRole::Companion
                && !entry.caused_by_ids.is_empty()
                && entry.message == expected
        })
    }) {
        return TutorialResponseStatus::SavedForPresentation;
    }
    if pending {
        TutorialResponseStatus::Pending
    } else {
        TutorialResponseStatus::None
    }
}

impl DesktopState {
    pub(crate) async fn tutorial_response_status(
        &self,
        step: TutorialStep,
    ) -> Result<TutorialResponseStatus, RuntimeError> {
        let Some(response_key) = step.response_key() else {
            return Ok(TutorialResponseStatus::None);
        };
        let expected = self.tutorial.lock().await.expected_response_message();
        if expected.is_some() {
            let storage = CompanionStorage::from_paths(
                &self.paths,
                self.runtime.config().retention.conversation_days,
            );
            let conversation = storage
                .load_conversation()
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            let status = response_status_from_entries(
                &conversation,
                expected.as_deref(),
                self.runtime.has_pending_tutorial_response(response_key)?,
            );
            if status != TutorialResponseStatus::None {
                return Ok(status);
            }
        }
        if self.runtime.has_pending_tutorial_response(response_key)? {
            return Ok(TutorialResponseStatus::Pending);
        }
        Ok(TutorialResponseStatus::None)
    }

    pub(super) async fn tutorial_response_presented(
        self: &Arc<Self>,
        entry_id: String,
        message: String,
    ) {
        let state = self.clone();
        let handler_state = state.clone();
        let _ = state
            .dispatch(
                crate::command_guard::CommandSource::RuntimeMonitor,
                crate::command_guard::DesktopCommand::PresentTutorialResponse,
                move |_context| async move {
                    handler_state
                        .advance_after_tutorial_response(&entry_id, &message)
                        .await;
                    Ok(())
                },
            )
            .await;
    }

    pub(super) async fn advance_after_tutorial_response(
        self: &Arc<Self>,
        entry_id: &str,
        message: &str,
    ) {
        let entry_id = entry_id.to_owned();
        let message = message.to_owned();
        let _ = self
            .ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::tutorial_progress_presenter::event(
                    crate::tutorial_progress_presenter::ProgressEvent::Response {
                        entry_id,
                        message,
                        reply,
                    },
                )
            })
            .await;
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn test_present_current_tutorial_response(
        self: &Arc<Self>,
        entry_id: &str,
    ) -> anyhow::Result<()> {
        let message = self
            .tutorial
            .lock()
            .await
            .expected_response_message()
            .ok_or_else(|| anyhow::anyhow!("current tutorial step has no response"))?;
        // 保存後の runtime 通知が既に表示を開始していても、ここでは renderer の完了を模擬する。
        self.advance_after_tutorial_response(entry_id, &message)
            .await;
        anyhow::ensure!(
            self.tutorial.lock().await.step_response_presented(),
            "tutorial response presentation was not accepted"
        );
        Ok(())
    }

    pub(super) async fn tutorial_settings_opened(self: &Arc<Self>) -> Result<bool, RuntimeError> {
        let highlight_requested = self.tutorial.lock().await.request_settings_highlight();
        if highlight_requested.is_some() {
            self.publish_tutorial_state().await;
        }
        Ok(true)
    }

    pub(super) async fn tutorial_settings_presented(self: &Arc<Self>) -> Result<(), RuntimeError> {
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::tutorial_progress_presenter::event(
                    crate::tutorial_progress_presenter::ProgressEvent::Settings(reply),
                )
            })
            .await
            .map_err(RuntimeError::Factory)?
    }

    pub(super) async fn tutorial_watch_started(self: &Arc<Self>) -> Result<(), ConfigCommitError> {
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::tutorial_progress_presenter::event(
                    crate::tutorial_progress_presenter::ProgressEvent::Watch(reply),
                )
            })
            .await
            .map_err(RuntimeError::Factory)?
            .map_err(ConfigCommitError::Runtime)
    }

    pub(super) async fn finish_tutorial_step(
        self: &Arc<Self>,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<(), RuntimeError> {
        self.request_finish_tutorial_step(step, skipped, true).await
    }

    pub(super) async fn finish_tutorial_step_without_guide_auto_advance(
        self: &Arc<Self>,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<(), RuntimeError> {
        self.request_finish_tutorial_step(step, skipped, false)
            .await
    }

    async fn request_finish_tutorial_step(
        &self,
        step: TutorialStep,
        skipped: bool,
        automatic: bool,
    ) -> Result<(), RuntimeError> {
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::tutorial_progress_presenter::event(
                    crate::tutorial_progress_presenter::ProgressEvent::Finish {
                        step,
                        skipped,
                        automatic,
                        reply,
                    },
                )
            })
            .await
            .map_err(RuntimeError::Factory)?
    }
}

