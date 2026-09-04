use crate::bubbles::{BubbleAction, BubbleInteraction, BubbleRecord};
use crate::state::DesktopState;
use std::sync::Arc;

pub(crate) async fn show_reset_prompt(state: Arc<DesktopState>) {
    let config = state.runtime_config();
    let conversation_generation = state.bubbles.lock().await.conversation_generation();
    let record = reset_prompt_record(&config, conversation_generation);
    crate::bubbles::show_best_effort(state, record, config.notification.bubble_duration_ms).await;
}

fn reset_prompt_record(
    config: &coosenpai_core::config::Config,
    conversation_generation: u64,
) -> BubbleRecord {
    BubbleRecord {
        id: "conversation-reset-confirmation".to_owned(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: "今の会話を閉じて新しく始めますか？（履歴は残ります）".to_owned(),
        message_kind: "notice".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name.clone(),
        persona: config.companion.persona.clone(),
        avatar_color: config.ui.avatar_color.clone(),
        conversation_generation,
        persistent: true,
        open_url: None,
        interaction: Some(reset_interaction()),
    }
}

pub(crate) async fn show_reset_complete(state: Arc<DesktopState>) {
    let config = state.runtime_config();
    let conversation_generation = state.bubbles.lock().await.conversation_generation();
    let record = BubbleRecord {
        id: format!("conversation-reset-complete-{conversation_generation}"),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: "会話をリセットしました".to_owned(),
        message_kind: "notice".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name,
        persona: config.companion.persona,
        avatar_color: config.ui.avatar_color,
        conversation_generation,
        persistent: false,
        open_url: None,
        interaction: None,
    };
    crate::bubbles::show_best_effort(state, record, 2_000).await;
}

pub(crate) async fn show_tutorial_complete(state: Arc<DesktopState>) {
    let config = state.runtime_config();
    let conversation_generation = state.bubbles.lock().await.conversation_generation();
    let record = BubbleRecord {
        id: format!("tutorial-complete-{conversation_generation}"),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: "チュートリアルを終わりました。ここからは本番です".to_owned(),
        message_kind: "notice".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name,
        persona: config.companion.persona,
        avatar_color: config.ui.avatar_color,
        conversation_generation,
        persistent: false,
        open_url: None,
        interaction: None,
    };
    crate::bubbles::show_best_effort(state, record, config.notification.bubble_duration_ms).await;
}

fn reset_interaction() -> BubbleInteraction {
    BubbleInteraction {
        select: None,
        actions: vec![
            BubbleAction {
                id: "conversation-reset-confirm".to_owned(),
                label: "はい".to_owned(),
            },
            BubbleAction {
                id: "conversation-reset-cancel".to_owned(),
                label: "いいえ".to_owned(),
            },
        ],
        detail: None,
        technical_detail: None,
    }
}

