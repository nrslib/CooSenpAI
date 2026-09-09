use crate::bubbles::{BubbleAction, BubbleInteraction, BubbleRecord};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use std::sync::Arc;

#[cfg(test)]
fn reset_prompt_record(
    config: &coosenpai_core::config::Config,
    conversation_generation: u64,
) -> BubbleRecord {
    reset_prompt_record_for_locale(config, conversation_generation, Locale::Ja)
}

pub(crate) fn reset_prompt_record_for_locale(
    config: &coosenpai_core::config::Config,
    conversation_generation: u64,
    locale: Locale,
) -> BubbleRecord {
    BubbleRecord {
        id: "conversation-reset-confirmation".to_owned(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: text(TextKey::ConversationResetPrompt, locale).to_owned(),
        message_kind: "notice".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name.clone(),
        persona: config.companion.persona.clone(),
        avatar_color: config.ui.avatar_color.clone(),
        conversation_generation,
        persistent: true,
        interaction: Some(reset_interaction_for_locale(locale)),
    }
}

pub(crate) async fn show_reset_complete(state: Arc<DesktopState>) {
    let config = state.runtime_config();
    let conversation_generation = state.bubbles.lock().await.conversation_generation();
    let record = BubbleRecord {
        id: format!("conversation-reset-complete-{conversation_generation}"),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: text(
            TextKey::ConversationResetComplete,
            Locale::from_config(&config.ui.language),
        )
        .to_owned(),
        message_kind: "notice".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name,
        persona: config.companion.persona,
        avatar_color: config.ui.avatar_color,
        conversation_generation,
        persistent: false,
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
        message: text(
            TextKey::TutorialComplete,
            Locale::from_config(&config.ui.language),
        )
        .to_owned(),
        message_kind: "notice".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name,
        persona: config.companion.persona,
        avatar_color: config.ui.avatar_color,
        conversation_generation,
        persistent: false,
        interaction: None,
    };
    crate::bubbles::show_best_effort(state, record, config.notification.bubble_duration_ms).await;
}

fn reset_interaction_for_locale(locale: Locale) -> BubbleInteraction {
    BubbleInteraction {
        select: None,
        secret_input: None,
        actions: vec![
            BubbleAction {
                id: "conversation-reset-confirm".to_owned(),
                label: text(TextKey::CommonYes, locale).to_owned(),
            },
            BubbleAction {
                id: "conversation-reset-cancel".to_owned(),
                label: text(TextKey::CommonNo, locale).to_owned(),
            },
        ],
        detail: None,
        technical_detail: None,
    }
}

