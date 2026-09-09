use crate::bubbles::{BubbleAction, BubbleInteraction, BubbleRecord};
use crate::ui_events::{UiEffect, UiTask};
use coosenpai_core::locale::{text, Locale, TextKey};

#[derive(Debug)]
pub(crate) struct FactCandidateLoaded {
    pub candidate: Option<coosenpai_core::memory::FactCandidate>,
    pub conversation_generation: u64,
}

pub(crate) fn tick_completed(
    date: chrono::NaiveDate,
    result: Result<bool, String>,
) -> Vec<UiEffect> {
    let Ok(event_completed) = result else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    if event_completed {
        effects.push(UiEffect::Run(UiTask::RefreshConversation));
    }
    effects.push(UiEffect::Spawn(UiTask::ReadFactCandidate(date)));
    effects
}

pub(crate) fn fact_prompt(
    candidate: coosenpai_core::memory::FactCandidate,
    config: &coosenpai_core::config::Config,
    conversation_generation: u64,
) -> BubbleRecord {
    let locale = Locale::from_config(&config.ui.language);
    BubbleRecord {
        id: format!("fact-prompt-{}", candidate.id),
        created_at: chrono::Utc::now().to_rfc3339(),
        message: text(TextKey::FactRememberPrompt, locale).replace("{text}", &candidate.text),
        message_kind: "fact-confirmation".to_owned(),
        notification_priority: "info".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name.clone(),
        persona: config.companion.persona.clone(),
        avatar_color: config.ui.avatar_color.clone(),
        conversation_generation,
        persistent: true,
        interaction: Some(BubbleInteraction {
            select: None,
            secret_input: None,
            actions: vec![
                BubbleAction {
                    id: "memory-confirm".to_owned(),
                    label: text(TextKey::CommonYes, locale).to_owned(),
                },
                BubbleAction {
                    id: "memory-reject".to_owned(),
                    label: text(TextKey::CommonNo, locale).to_owned(),
                },
            ],
            detail: Some(candidate.id),
            technical_detail: None,
        }),
    }
}
