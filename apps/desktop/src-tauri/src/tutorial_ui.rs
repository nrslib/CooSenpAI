use crate::snapshot::AppSnapshot;
use crate::status_presenter::UiText;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TutorialUi {
    pub message: UiText,
    pub next: Option<&'static str>,
    pub finish_label: UiText,
}
pub(crate) fn response_failed(s: &AppSnapshot) -> bool {
    s.onboarding.tutorial_active
        && ["chat", "text", "image", "voice"]
            .contains(&s.onboarding.current_step.as_deref().unwrap_or(""))
        && s.last_error.is_some()
        && s.active_user_message_id.is_none()
        && s.conversation.iter().any(|entry| {
            entry.role == coosenpai_core::state::ConversationRole::User
                && !s
                    .conversation
                    .iter()
                    .any(|answer| answer.caused_by_ids.contains(&entry.id))
        })
}
pub(crate) fn settings_available(s: &AppSnapshot) -> bool {
    !s.onboarding.tutorial_active
        || ["persona", "watch"].contains(&s.onboarding.current_step.as_deref().unwrap_or(""))
}
pub(crate) fn settings_ack(s: &AppSnapshot) -> bool {
    s.onboarding.tutorial_active
        && matches!(
            s.onboarding.current_step.as_deref(),
            Some("persona" | "watch")
        )
        && s.onboarding.current_step == s.onboarding.settings_highlight
}
pub(crate) fn view(s: &AppSnapshot) -> Option<TutorialUi> {
    if !s.onboarding.tutorial_active {
        return None;
    }
    let failed = response_failed(s);
    let can_skip = ["chat", "text", "image", "voice", "watch"]
        .contains(&s.onboarding.current_step.as_deref().unwrap_or(""));
    Some(TutorialUi {
        message: if s.onboarding.finish_pending {
            UiText::message("app.tutorialFinishPending")
        } else if failed {
            UiText::message("app.tutorialResponseFailed")
        } else {
            s.onboarding
                .skip_hint
                .as_ref()
                .map(UiText::literal)
                .unwrap_or_else(|| UiText::message("app.tutorialGuide"))
        },
        next: (!s.onboarding.finish_pending && !s.onboarding.resume_pending && can_skip)
            .then_some(if failed { "retry" } else { "next" }),
        finish_label: UiText::message(if s.onboarding.finish_pending {
            "app.tutorialFinishRetry"
        } else {
            "app.tutorialFinish"
        }),
    })
}
pub(crate) fn highlighted_sections(onboarding: &crate::snapshot::OnboardingView) -> Vec<String> {
    if !onboarding.tutorial_active || onboarding.current_step != onboarding.settings_highlight {
        return vec![];
    }
    match onboarding.current_step.as_deref() {
        Some("persona") => vec!["persona".into(), "provider".into()],
        Some("watch") => vec!["watch".into()],
        _ => vec![],
    }
}

