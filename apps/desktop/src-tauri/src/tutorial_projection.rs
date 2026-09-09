use crate::snapshot::OnboardingView;
use crate::tutorial::{TutorialController, TutorialSettingsHighlight};
use coosenpai_core::onboarding::{OnboardingState, TutorialStep};

#[derive(Debug)]
pub(crate) struct TutorialSnapshotData {
    state: OnboardingState,
    resume_pending: bool,
    skip_hint: Option<String>,
    settings_highlight: Option<TutorialSettingsHighlight>,
}

impl TutorialSnapshotData {
    pub(crate) fn read(controller: &TutorialController) -> Self {
        Self {
            state: controller.state().clone(),
            resume_pending: controller.resume_pending(),
            skip_hint: controller
                .provider()
                .and_then(|provider| provider.render("skip-hint").ok()),
            settings_highlight: controller.settings_highlight_pending(),
        }
    }

    pub(crate) fn view(self) -> OnboardingView {
        let mut view = OnboardingView::from_state_and_resume(&self.state, self.resume_pending);
        let current_step = self.state.current_step();
        if self.state.tutorial_active()
            && matches!(
                current_step,
                Some(
                    TutorialStep::Chat
                        | TutorialStep::Text
                        | TutorialStep::Image
                        | TutorialStep::Voice
                        | TutorialStep::Watch
                )
            )
        {
            view.skip_hint = self.skip_hint;
        }
        view.chat_input_enabled = !self.resume_pending
            && self.state.tutorial_active()
            && current_step == Some(TutorialStep::Chat)
            && self
                .state
                .tutorial
                .notices
                .get("after-open")
                .is_some_and(|notice| notice.bubble_accepted);
        view.settings_highlight = self
            .settings_highlight
            .map(|highlight| highlight.as_str().to_owned());
        view.highlighted_settings = crate::tutorial_ui::highlighted_sections(&view);
        view
    }
}
