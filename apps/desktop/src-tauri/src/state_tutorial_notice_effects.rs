use super::DesktopState;
use crate::tutorial_notice::{TutorialBubbleOutcome, TutorialNoticeEffects};
use async_trait::async_trait;
use coosenpai_core::companion_storage::CompanionStorage;
use coosenpai_core::onboarding_notice::TutorialNoticePlan;
use coosenpai_core::runtime::RuntimeError;
use coosenpai_core::state::{ConversationEntry, ConversationRole};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) struct DesktopTutorialNoticeEffects {
    state: Arc<DesktopState>,
    require_ack: bool,
    replaced_bubble_ids: Vec<String>,
    transition_cancellation: CancellationToken,
}

impl DesktopTutorialNoticeEffects {
    pub(super) fn new(state: Arc<DesktopState>, require_ack: bool) -> Self {
        let transition_cancellation = state.cancellation.clone();
        Self {
            state,
            require_ack,
            replaced_bubble_ids: Vec::new(),
            transition_cancellation,
        }
    }

    pub(super) fn replacing(
        state: Arc<DesktopState>,
        require_ack: bool,
        replaced_bubble_ids: Vec<String>,
        transition_cancellation: CancellationToken,
    ) -> Self {
        Self {
            state,
            require_ack,
            replaced_bubble_ids,
            transition_cancellation,
        }
    }
}

#[async_trait]
impl TutorialNoticeEffects for DesktopTutorialNoticeEffects {
    async fn append_conversation(&self, notice: &TutorialNoticePlan) -> Result<(), RuntimeError> {
        append_tutorial_conversation(
            &self.state,
            &notice.id,
            &notice.key,
            &notice.created_at,
            &notice.message,
        )
        .await
    }

    async fn present_bubble(
        &self,
        notice: &TutorialNoticePlan,
    ) -> Result<TutorialBubbleOutcome, RuntimeError> {
        self.state
            .present_tutorial_notice(
                notice.id.clone(),
                notice.key.clone(),
                notice.created_at.clone(),
                notice.message.clone(),
                self.require_ack,
                self.replaced_bubble_ids.clone(),
                self.transition_cancellation.clone(),
            )
            .await
    }
}

pub(super) async fn append_tutorial_conversation(
    state: &DesktopState,
    id: &str,
    tutorial_response_key: &str,
    created_at: &str,
    message: &str,
) -> Result<(), RuntimeError> {
    let entry = ConversationEntry {
        schema_version: 1,
        id: id.to_owned(),
        created_at: created_at.to_owned(),
        role: ConversationRole::Companion,
        message: message.to_owned(),
        attachment_path: None,
        attachment_text: None,
        tutorial_response_key: Some(tutorial_response_key.to_owned()),
        screen_context: None,
        caused_by_ids: Vec::new(),
        notification_priority: "none".to_owned(),
    };
    let storage = CompanionStorage::from_paths(
        &state.paths,
        state.runtime.config().retention.conversation_days,
    );
    tokio::task::spawn_blocking(move || {
        storage.append_conversation_once_at(&entry, chrono::Utc::now())
    })
    .await
    .map_err(|error| RuntimeError::Factory(error.to_string()))?
    .map_err(|error| RuntimeError::Factory(error.to_string()))?;
    state.ui.input(
        crate::ui_events::UiView::Application,
        crate::ui_events::UiEvent::Tutorial(Box::new(
            crate::tutorial_events::TutorialEvent::Notice(
                crate::tutorial_notice_presenter::NoticeEvent::ConversationSaved,
            ),
        )),
    );
    Ok(())
}

pub(super) fn tracks_tutorial_notice_progress(key: &str) -> bool {
    !matches!(key, "later" | "finish" | "forced-finish")
}

impl DesktopState {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn present_tutorial_notice(
        &self,
        id: String,
        key: String,
        created_at: String,
        message: String,
        require_ack: bool,
        replaced_bubble_ids: Vec<String>,
        cancellation: CancellationToken,
    ) -> Result<TutorialBubbleOutcome, RuntimeError> {
        let config = self.runtime.config();
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::ui_events::UiEvent::Tutorial(Box::new(
                    crate::tutorial_events::TutorialEvent::Notice(
                        crate::tutorial_notice_presenter::NoticeEvent::Requested {
                            id,
                            key,
                            created_at,
                            message,
                            config: Box::new(config),
                            conversation_generation,
                            require_ack,
                            replaced_bubble_ids,
                            cancellation,
                            reply,
                        },
                    ),
                ))
            })
            .await
            .map_err(RuntimeError::Factory)?
    }
}
