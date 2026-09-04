use coosenpai_core::onboarding::{
    timestamp_now, OnboardingError, OnboardingState, OnboardingStore, TutorialProvider,
    TutorialStep,
};
use coosenpai_core::onboarding_notice::TutorialNoticePlan;
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;

pub const FINISH_PENDING_MESSAGE: &str = "終了処理をやり直してください";
pub(crate) const TUTORIAL_AUTO_ADVANCE_MESSAGE: &str = "この案内は自動で進みます";
pub(crate) const TUTORIAL_SKIP_ACTION: &str = "tutorial-skip";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupPhase {
    Inactive,
    Selecting {
        selected: String,
        detail: Option<String>,
    },
    Connecting {
        provider: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TutorialSettingsHighlight {
    Persona,
    Watch,
}

impl TutorialSettingsHighlight {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Persona => "persona",
            Self::Watch => "watch",
        }
    }
}

#[derive(Clone)]
pub struct SetupAttempt {
    generation: u64,
    cancellation: CancellationToken,
}

impl SetupAttempt {
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

fn setup_selecting(selected: &str, detail: Option<String>) -> SetupPhase {
    SetupPhase::Selecting {
        selected: selected.to_owned(),
        detail,
    }
}

pub(crate) fn tutorial_step_can_be_skipped(step: TutorialStep) -> bool {
    step.response_key().is_some() || step == TutorialStep::Watch
}

pub(crate) fn tutorial_step_for_guide_key(key: &str) -> Option<TutorialStep> {
    match key {
        "after-open" => Some(TutorialStep::Chat),
        "text-intro" => Some(TutorialStep::Text),
        "image-intro" => Some(TutorialStep::Image),
        "voice-intro" => Some(TutorialStep::Voice),
        "watch-intro" => Some(TutorialStep::Watch),
        _ => None,
    }
}

pub struct TutorialController {
    store: OnboardingStore,
    state: OnboardingState,
    provider: Option<TutorialProvider>,
    chat_opened: bool,
    resume_pending: bool,
    setup_phase: SetupPhase,
    setup_generation: u64,
    setup_cancellation: CancellationToken,
    settings_highlight_pending: Option<TutorialSettingsHighlight>,
    settings_presentation_in_progress: Option<TutorialSettingsHighlight>,
    presenting_responses: BTreeSet<String>,
    presented_responses: BTreeSet<String>,
    step_response_id: Option<String>,
    step_response_presented: bool,
    watch_capture_presenting: bool,
    production_restored: bool,
}

impl TutorialController {
    pub fn load(store: OnboardingStore) -> Result<Self, OnboardingError> {
        let state = store.load()?;
        Ok(Self::from_state(store, state))
    }

    pub fn from_state(store: OnboardingStore, state: OnboardingState) -> Self {
        let resume_pending = state.tutorial_active();
        let setup_phase = if state.needs_setup() {
            setup_selecting("codex", None)
        } else {
            SetupPhase::Inactive
        };
        Self {
            store,
            state,
            provider: None,
            chat_opened: false,
            resume_pending,
            setup_phase,
            setup_generation: 0,
            setup_cancellation: CancellationToken::new(),
            settings_highlight_pending: None,
            settings_presentation_in_progress: None,
            presenting_responses: BTreeSet::new(),
            presented_responses: BTreeSet::new(),
            step_response_id: None,
            step_response_presented: false,
            watch_capture_presenting: false,
            production_restored: false,
        }
    }

    pub fn state(&self) -> &OnboardingState {
        &self.state
    }

    pub fn provider(&self) -> Option<TutorialProvider> {
        self.provider.clone()
    }

    pub fn resume_pending(&self) -> bool {
        self.resume_pending && !self.finish_pending()
    }

    pub fn chat_input_enabled(&self) -> bool {
        !self.resume_pending()
            && self.state.tutorial_active()
            && self.state.current_step() == Some(TutorialStep::Chat)
            && self
                .state
                .tutorial
                .notices
                .get("after-open")
                .is_some_and(|notice| notice.bubble_accepted)
    }

    pub fn finish_pending(&self) -> bool {
        self.state.tutorial_finish_pending()
    }

    pub fn production_restored(&self) -> bool {
        self.production_restored
    }

    pub fn attach_provider(&mut self, provider: TutorialProvider) {
        self.provider = Some(provider);
        self.chat_opened = false;
        self.resume_pending = true;
        self.reset_response_tracking();
        self.production_restored = false;
        self.settings_highlight_pending = None;
        self.settings_presentation_in_progress = None;
    }

    pub fn attach_setup_provider(&mut self, provider: TutorialProvider) {
        self.provider = Some(provider);
        self.chat_opened = false;
        self.resume_pending = false;
        if !matches!(self.setup_phase, SetupPhase::Selecting { .. }) {
            self.setup_phase = setup_selecting("codex", None);
        }
        self.reset_response_tracking();
        self.production_restored = false;
        self.settings_highlight_pending = None;
        self.settings_presentation_in_progress = None;
    }

    pub fn resume(&mut self) {
        self.resume_pending = false;
    }

    pub fn setup_phase(&self) -> &SetupPhase {
        &self.setup_phase
    }

    pub fn begin_setup_connection(
        &mut self,
        provider: &str,
        parent_cancellation: &CancellationToken,
    ) -> Result<SetupAttempt, OnboardingError> {
        if !matches!(provider, "codex" | "claude" | "opencode") {
            return Err(OnboardingError::Invalid("provider が不正です".to_owned()));
        }
        if !matches!(self.setup_phase, SetupPhase::Selecting { .. }) {
            return Err(OnboardingError::Invalid(
                "初回セットアップの応答を受け付けられません".to_owned(),
            ));
        }
        self.setup_cancellation.cancel();
        self.setup_generation = self.setup_generation.saturating_add(1);
        self.setup_cancellation = parent_cancellation.child_token();
        self.setup_phase = SetupPhase::Connecting {
            provider: provider.to_owned(),
        };
        Ok(SetupAttempt {
            generation: self.setup_generation,
            cancellation: self.setup_cancellation.clone(),
        })
    }

    pub fn setup_attempt_is_current(&self, attempt: &SetupAttempt) -> bool {
        self.setup_generation == attempt.generation && !attempt.cancellation.is_cancelled()
    }

    pub fn setup_connection_failed(
        &mut self,
        attempt: &SetupAttempt,
        provider: &str,
        detail: String,
    ) -> bool {
        if self.setup_attempt_is_current(attempt)
            && matches!(
                &self.setup_phase,
                SetupPhase::Connecting { provider: current } if current == provider
            )
        {
            self.setup_phase = setup_selecting(provider, Some(detail));
            return true;
        }
        false
    }

    pub fn invalidate_setup_attempt(&mut self) {
        self.setup_cancellation.cancel();
        self.setup_generation = self.setup_generation.saturating_add(1);
    }

    pub fn setup_detail(&self) -> Option<&str> {
        match &self.setup_phase {
            SetupPhase::Selecting { detail, .. } => detail.as_deref(),
            SetupPhase::Inactive | SetupPhase::Connecting { .. } => None,
        }
    }

    pub fn setup_selected(&self) -> &str {
        match &self.setup_phase {
            SetupPhase::Selecting { selected, .. } => selected,
            SetupPhase::Connecting { provider } => provider,
            SetupPhase::Inactive => "codex",
        }
    }

    pub fn skip_hint(&self) -> Option<String> {
        if !self.state.tutorial_active()
            || !matches!(
                self.state.current_step(),
                Some(
                    TutorialStep::Chat
                        | TutorialStep::Text
                        | TutorialStep::Image
                        | TutorialStep::Voice
                        | TutorialStep::Watch
                )
            )
        {
            return None;
        }
        self.provider.as_ref()?.render("skip-hint").ok()
    }

    pub fn start(&mut self, provider: TutorialProvider) -> Result<(), OnboardingError> {
        let now = timestamp_now();
        let setup_ok = provider
            .render("setup-ok")
            .map_err(|error| OnboardingError::Invalid(error.to_string()))?;
        let intro = provider
            .render("intro")
            .map_err(|error| OnboardingError::Invalid(error.to_string()))?;
        let intro_click = provider
            .render("intro-click")
            .map_err(|error| OnboardingError::Invalid(error.to_string()))?;
        self.state = self.store.update(|state| {
            state.complete_setup(&now);
            state.start_tutorial(&now);
            state.prepare_tutorial_notice_sequence(
                &[
                    ("setup-ok", &setup_ok),
                    ("intro", &intro),
                    ("intro-click", &intro_click),
                ],
                &now,
            )?;
            Ok::<_, OnboardingError>(state.clone())
        })??;
        self.provider = Some(provider);
        self.chat_opened = false;
        self.resume_pending = false;
        self.setup_phase = SetupPhase::Inactive;
        self.reset_response_tracking();
        self.production_restored = false;
        self.settings_highlight_pending = None;
        self.settings_presentation_in_progress = None;
        Ok(())
    }

    pub fn restart(&mut self, provider: TutorialProvider) -> Result<(), OnboardingError> {
        let now = timestamp_now();
        let intro = provider
            .render("intro")
            .map_err(|error| OnboardingError::Invalid(error.to_string()))?;
        let intro_click = provider
            .render("intro-click")
            .map_err(|error| OnboardingError::Invalid(error.to_string()))?;
        self.state = self.store.update(|state| {
            state.tutorial = Default::default();
            state.start_tutorial(&now);
            state.prepare_tutorial_notice_sequence(
                &[("intro", &intro), ("intro-click", &intro_click)],
                &now,
            )?;
            Ok::<_, OnboardingError>(state.clone())
        })??;
        self.provider = Some(provider);
        self.chat_opened = false;
        self.resume_pending = false;
        self.setup_phase = SetupPhase::Inactive;
        self.reset_response_tracking();
        self.production_restored = false;
        self.settings_highlight_pending = None;
        self.settings_presentation_in_progress = None;
        Ok(())
    }

    pub fn prepare_notice(
        &mut self,
        key: &str,
        message: &str,
        created_at: &str,
    ) -> Result<TutorialNoticePlan, OnboardingError> {
        let (state, plan) = self.store.update(|state| {
            let plan = state.prepare_tutorial_notice(key, message, created_at)?;
            Ok::<_, OnboardingError>((state.clone(), plan))
        })??;
        self.state = state;
        Ok(plan)
    }

    pub fn mark_notice_conversation_stored(&mut self, key: &str) -> Result<(), OnboardingError> {
        self.state = self.store.update(|state| {
            state.mark_tutorial_notice_conversation_stored(key)?;
            Ok::<_, OnboardingError>(state.clone())
        })??;
        Ok(())
    }

    pub fn mark_notice_bubble_accepted(&mut self, key: &str) -> Result<(), OnboardingError> {
        self.state = self.store.update(|state| {
            state.mark_tutorial_notice_bubble_accepted(key)?;
            Ok::<_, OnboardingError>(state.clone())
        })??;
        Ok(())
    }

    pub fn reopen_notice_bubble(&mut self, key: &str) -> Result<(), OnboardingError> {
        self.state = self.store.update(|state| {
            state.reopen_tutorial_notice_bubble(key)?;
            Ok::<_, OnboardingError>(state.clone())
        })??;
        Ok(())
    }

    pub fn take_chat_opened(&mut self) -> bool {
        if self.chat_opened
            || !self.state.tutorial_active()
            || self.finish_pending()
            || self.state.current_step() != Some(TutorialStep::Chat)
        {
            return false;
        }
        self.chat_opened = true;
        true
    }

    pub fn chat_open_presentation_failed(&mut self) {
        let accepted = self
            .state
            .tutorial
            .notices
            .get("after-open")
            .is_some_and(|notice| notice.bubble_accepted);
        if self.state.current_step() == Some(TutorialStep::Chat) && !accepted {
            self.chat_opened = false;
        }
    }

    pub fn finish_step(
        &mut self,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<Option<TutorialStep>, OnboardingError> {
        if self.finish_pending() {
            return Err(OnboardingError::Invalid(FINISH_PENDING_MESSAGE.to_owned()));
        }
        let now = timestamp_now();
        self.state = self.store.update(|state| {
            state.finish_step(step, skipped, &now);
            state.clone()
        })?;
        if step == TutorialStep::Persona {
            self.settings_highlight_pending = None;
            self.settings_presentation_in_progress = None;
        }
        if matches!(
            step,
            TutorialStep::Chat | TutorialStep::Text | TutorialStep::Image | TutorialStep::Voice
        ) {
            self.step_response_id = None;
            self.step_response_presented = false;
        }
        Ok(self.state.current_step())
    }

    pub fn request_settings_highlight(&mut self) -> Option<TutorialSettingsHighlight> {
        if !self.state.tutorial_active() || self.finish_pending() {
            return None;
        }
        let highlight = match self.state.current_step()? {
            TutorialStep::Persona => TutorialSettingsHighlight::Persona,
            TutorialStep::Watch => TutorialSettingsHighlight::Watch,
            TutorialStep::Chat | TutorialStep::Text | TutorialStep::Image | TutorialStep::Voice => {
                return None
            }
        };
        self.settings_highlight_pending = Some(highlight);
        Some(highlight)
    }

    pub fn settings_highlight_pending(&self) -> Option<TutorialSettingsHighlight> {
        self.settings_highlight_pending
    }

    pub fn begin_settings_presentation(&mut self) -> Option<TutorialSettingsHighlight> {
        if self.settings_presentation_in_progress.is_some()
            || !self.state.tutorial_active()
            || self.finish_pending()
        {
            return None;
        }
        let highlight = self.settings_highlight_pending?;
        let expected_step = match highlight {
            TutorialSettingsHighlight::Persona => TutorialStep::Persona,
            TutorialSettingsHighlight::Watch => TutorialStep::Watch,
        };
        if self.state.current_step() != Some(expected_step) {
            return None;
        }
        self.settings_presentation_in_progress = Some(highlight);
        Some(highlight)
    }

    pub fn complete_settings_presentation(
        &mut self,
    ) -> Result<Option<TutorialStep>, OnboardingError> {
        match self.settings_presentation_in_progress {
            Some(TutorialSettingsHighlight::Persona) => {
                let result = self.finish_step(TutorialStep::Persona, false);
                if result.is_err() {
                    self.settings_presentation_in_progress = None;
                }
                result
            }
            Some(TutorialSettingsHighlight::Watch) => {
                self.settings_highlight_pending = None;
                self.settings_presentation_in_progress = None;
                Ok(self.state.current_step())
            }
            None => Err(OnboardingError::Invalid(
                "設定画面の表示確認を受け付けられません".to_owned(),
            )),
        }
    }

    pub fn settings_presentation_failed(&mut self) {
        if let Some(highlight) = self.settings_presentation_in_progress.take() {
            self.settings_highlight_pending = Some(highlight);
        }
    }

    pub fn finish(&mut self) -> Result<(), OnboardingError> {
        let now = timestamp_now();
        self.state = self.store.update(|state| {
            state.finish_tutorial(&now);
            state.clone()
        })?;
        self.provider = None;
        self.resume_pending = false;
        self.setup_phase = SetupPhase::Inactive;
        self.reset_response_tracking();
        self.production_restored = false;
        self.settings_highlight_pending = None;
        self.settings_presentation_in_progress = None;
        Ok(())
    }

    pub fn prepare_finish(&mut self) -> Result<(), OnboardingError> {
        if self.state.tutorial_finish_pending() {
            return Ok(());
        }
        let now = timestamp_now();
        self.state = self.store.update(|state| {
            state.request_tutorial_finish(&now);
            state.clone()
        })?;
        Ok(())
    }

    pub fn mark_production_restored(&mut self) {
        self.provider = None;
        self.resume_pending = false;
        self.reset_response_tracking();
        self.production_restored = true;
    }

    pub fn reset_setup(&mut self) -> Result<(), OnboardingError> {
        self.state = self.store.update(|state| {
            state.setup.completed_at = None;
            state.tutorial = Default::default();
            state.clone()
        })?;
        self.provider = None;
        self.resume_pending = false;
        self.setup_phase = setup_selecting("codex", None);
        self.reset_response_tracking();
        self.production_restored = false;
        self.settings_highlight_pending = None;
        self.settings_presentation_in_progress = None;
        Ok(())
    }

    pub fn expected_response_message(&self) -> Option<String> {
        let key = match self.state.current_step()? {
            TutorialStep::Chat => "after-chat",
            TutorialStep::Text => "after-text",
            TutorialStep::Image => "after-image",
            TutorialStep::Voice => "after-voice",
            TutorialStep::Persona | TutorialStep::Watch => return None,
        };
        self.provider.as_ref()?.render(key).ok()
    }

    pub fn begin_response_presentation(&mut self, id: &str, message: &str) -> bool {
        self.state.tutorial_active()
            && !self.finish_pending()
            && self
                .expected_response_message()
                .is_some_and(|expected| expected == message)
            && !self.presented_responses.contains(id)
            && self.presenting_responses.insert(id.to_owned())
    }

    pub fn response_presentation_accepted(
        &mut self,
        id: &str,
        message: &str,
    ) -> Option<TutorialStep> {
        if !self.state.tutorial_active()
            || self.finish_pending()
            || !self
                .expected_response_message()
                .is_some_and(|expected| expected == message)
            || !self.presented_responses.insert(id.to_owned())
        {
            return None;
        }
        self.presenting_responses.remove(id);
        let step = self.state.current_step()?;
        if matches!(
            step,
            TutorialStep::Chat | TutorialStep::Text | TutorialStep::Image | TutorialStep::Voice
        ) {
            self.step_response_id = Some(id.to_owned());
            self.step_response_presented = true;
        }
        Some(step)
    }

    pub fn step_response_presented(&self) -> bool {
        matches!(
            self.state.current_step(),
            Some(
                TutorialStep::Chat | TutorialStep::Text | TutorialStep::Image | TutorialStep::Voice
            )
        ) && self.step_response_presented
    }

    pub fn response_presentation_is_current(&self, step: TutorialStep, id: &str) -> bool {
        self.state.tutorial_active()
            && !self.finish_pending()
            && self.state.current_step() == Some(step)
            && self.step_response_presented
            && self.step_response_id.as_deref() == Some(id)
            && self.presented_responses.contains(id)
    }

    pub fn guide_presentation_is_current(&self, step: TutorialStep, id: &str) -> bool {
        let key = match step {
            TutorialStep::Persona => "persona-intro",
            TutorialStep::Watch => "watch-intro",
            TutorialStep::Chat | TutorialStep::Text | TutorialStep::Image | TutorialStep::Voice => {
                return false
            }
        };
        self.state.tutorial_active()
            && !self.finish_pending()
            && self.state.current_step() == Some(step)
            && self.settings_highlight_pending.is_none()
            && self.settings_presentation_in_progress.is_none()
            && (step != TutorialStep::Watch || !self.watch_capture_presenting)
            && matches!(
                self.state.tutorial.notices.get(key),
                Some(notice) if notice.bubble_accepted
            )
            && matches!(self.state.tutorial_notice_id(key), Ok(current) if current == id)
    }

    pub fn begin_watch_capture_presentation(&mut self) -> bool {
        if !self.state.tutorial_active()
            || self.finish_pending()
            || self.state.current_step() != Some(TutorialStep::Watch)
            || self.watch_capture_presenting
        {
            return false;
        }
        self.watch_capture_presenting = true;
        true
    }

    pub fn watch_capture_presentation_failed(&mut self) {
        self.watch_capture_presenting = false;
    }

    pub fn response_presentation_failed(&mut self, id: &str) {
        self.presenting_responses.remove(id);
    }

    fn reset_response_tracking(&mut self) {
        self.presenting_responses.clear();
        self.presented_responses.clear();
        self.step_response_id = None;
        self.step_response_presented = false;
        self.watch_capture_presenting = false;
    }
}

