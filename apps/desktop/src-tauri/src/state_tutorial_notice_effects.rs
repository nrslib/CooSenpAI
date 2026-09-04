use super::DesktopState;
use crate::bubbles::{self, BubbleAction, BubbleInteraction, BubbleRecord};
use crate::tutorial::{tutorial_step_for_guide_key, TUTORIAL_SKIP_ACTION};
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
        if crate::windows::main_is_focused(&self.state.app) {
            return Ok(TutorialBubbleOutcome::Acknowledged);
        }
        let config = self.state.runtime.config();
        let record = tutorial_bubble_record(
            &self.state,
            notice.id.clone(),
            &notice.key,
            notice.created_at.clone(),
            notice.message.clone(),
        )
        .await;
        if self.require_ack {
            bubbles::show_replacing(
                self.state.clone(),
                record,
                config.notification.bubble_duration_ms,
                &self.replaced_bubble_ids,
                &self.transition_cancellation,
            )
            .await
            .map(|outcome| match outcome {
                bubbles::BubblePresentationOutcome::Acknowledged => {
                    TutorialBubbleOutcome::Acknowledged
                }
                bubbles::BubblePresentationOutcome::Dismissed => TutorialBubbleOutcome::Dismissed,
            })
            .map_err(|error| RuntimeError::Factory(error.to_string()))
        } else {
            bubbles::show_best_effort(
                self.state.clone(),
                record,
                config.notification.bubble_duration_ms,
            )
            .await;
            Ok(TutorialBubbleOutcome::Acknowledged)
        }
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
    state.refresh_conversation().await;
    Ok(())
}

pub(super) async fn tutorial_bubble_record(
    state: &DesktopState,
    id: String,
    key: &str,
    created_at: String,
    message: String,
) -> BubbleRecord {
    let config = state.runtime.config();
    let conversation_generation = state.bubbles.lock().await.conversation_generation();
    BubbleRecord {
        id,
        created_at,
        message,
        message_kind: "tutorial".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name,
        persona: config.companion.persona,
        avatar_color: config.ui.avatar_color,
        conversation_generation,
        persistent: true,
        open_url: None,
        interaction: tutorial_skip_interaction(key),
    }
}

fn tutorial_skip_interaction(key: &str) -> Option<BubbleInteraction> {
    tutorial_step_for_guide_key(key).map(|_| BubbleInteraction {
        select: None,
        actions: vec![BubbleAction {
            id: TUTORIAL_SKIP_ACTION.to_owned(),
            label: "この項目をスキップ".to_owned(),
        }],
        detail: None,
        technical_detail: None,
    })
}

pub(super) fn tracks_tutorial_notice_progress(key: &str) -> bool {
    !matches!(key, "later" | "finish" | "forced-finish")
}

