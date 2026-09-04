use super::*;
use crate::command_guard::CommandContext;
use crate::config_update::ConfigUpdateOutcome;
use coosenpai_core::onboarding::TutorialStep;
use coosenpai_core::state::ObservationRecord;

impl DesktopState {
    pub(crate) async fn cancel_setup_attempt_before_restart(&self) {
        self.tutorial.lock().await.invalidate_setup_attempt();
        let _ = self.logger.write(
            "INFO",
            "初回セットアップの進行中 attempt を permit 待ちの前に取り消しました",
        );
    }

    pub(crate) async fn command_speech_begin(
        self: &Arc<Self>,
        permit: &CommandContext,
        source: crate::speech::SpeechSource,
    ) -> Result<(), String> {
        let action = crate::input_popup::start_action(
            self.input_popup_kind().await,
            crate::input_popup::InputPopupKind::Speech,
            match source {
                crate::speech::SpeechSource::Shortcut => {
                    crate::command_guard::CommandSource::GlobalShortcut
                }
                crate::speech::SpeechSource::Composer => {
                    crate::command_guard::CommandSource::IpcMain
                }
            },
        );
        if action == crate::input_popup::InputPopupStartAction::CancelThenStart {
            self.cancel_input_popup_for_switch(permit).await?;
        }
        self.speech
            .clone()
            .begin(self.clone(), permit, source)
            .await
    }

    pub(crate) fn command_speech_finish(self: &Arc<Self>, permit: &CommandContext) {
        self.speech.clone().finish(self.clone(), permit);
    }

    pub(crate) fn command_speech_cancel(
        self: &Arc<Self>,
        permit: &CommandContext,
    ) -> Result<(), String> {
        self.speech.clone().cancel(self.clone(), permit)
    }

    pub(crate) async fn cancel_input_popup_for_switch(
        &self,
        permit: &CommandContext,
    ) -> Result<(), String> {
        match self.input_popup_kind().await {
            Some(crate::input_popup::InputPopupKind::CaptureImage)
            | Some(crate::input_popup::InputPopupKind::CaptureText) => {
                crate::capture::cancel_for_switch(self, permit).await
            }
            Some(crate::input_popup::InputPopupKind::Speech) => {
                self.speech.cancel_and_wait_for_switch(self).await
            }
            None => Ok(()),
        }
    }

    pub(crate) async fn command_speech_confirm(
        self: &Arc<Self>,
        permit: &CommandContext,
        text: String,
    ) -> Result<String, String> {
        self.speech.confirm(self, permit, text).await
    }

    pub(crate) async fn command_enqueue_user_message(
        &self,
        permit: &CommandContext,
        message: String,
        caused_by: Vec<ObservationRecord>,
        attachment: user_input::UserMessageAttachment,
    ) -> Result<String, String> {
        self.enqueue_user_message_raw(message, caused_by, attachment, permit.tutorial_response())
            .await
    }

    pub(crate) async fn command_update_config_with<F>(
        self: &Arc<Self>,
        permit: &CommandContext,
        update: F,
    ) -> Result<ConfigUpdateOutcome, ConfigCommitError>
    where
        F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
    {
        self.update_config_with_raw(permit, update, None, None)
            .await
    }

    pub(crate) async fn command_update_config_with_expected_revision<F>(
        self: &Arc<Self>,
        permit: &CommandContext,
        expected_revision: u64,
        update: F,
    ) -> Result<ConfigUpdateOutcome, ConfigCommitError>
    where
        F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
    {
        self.update_config_with_raw(permit, update, None, Some(expected_revision))
            .await
    }

    pub(crate) async fn command_update_config_with_staged_avatar<F>(
        self: &Arc<Self>,
        permit: &CommandContext,
        staged_avatar: crate::avatar::StagedAvatar,
        update: F,
    ) -> Result<ConfigUpdateOutcome, ConfigCommitError>
    where
        F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
    {
        self.update_config_with_raw(permit, update, Some(staged_avatar), None)
            .await
    }

    pub(crate) async fn command_update_config_with_staged_avatar_expected_revision<F>(
        self: &Arc<Self>,
        permit: &CommandContext,
        staged_avatar: crate::avatar::StagedAvatar,
        expected_revision: u64,
        update: F,
    ) -> Result<ConfigUpdateOutcome, ConfigCommitError>
    where
        F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
    {
        self.update_config_with_raw(permit, update, Some(staged_avatar), Some(expected_revision))
            .await
    }

    pub(crate) async fn command_switch_persona(
        self: &Arc<Self>,
        _permit: &CommandContext,
        persona: String,
    ) -> Result<Config, ConfigCommitError> {
        self.switch_persona_raw(persona).await
    }

    pub(crate) async fn command_reload_persona(
        &self,
        _permit: &CommandContext,
    ) -> Result<(), ConfigCommitError> {
        self.reload_persona_raw().await
    }

    pub(crate) async fn command_handle_bubble_interaction(
        self: &Arc<Self>,
        permit: &CommandContext,
        id: &str,
        action: &str,
        value: Option<&str>,
    ) -> Result<(), ConfigCommitError> {
        self.handle_bubble_interaction(permit, id, action, value)
            .await
    }

    pub(crate) async fn command_sync_resolved_fact_prompt(
        &self,
        _permit: &CommandContext,
        candidate_id: &str,
    ) -> Result<(), ConfigCommitError> {
        self.sync_resolved_fact_prompt(candidate_id).await
    }

    pub(crate) async fn command_announce_initial_onboarding(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> Result<(), RuntimeError> {
        self.announce_initial_onboarding().await
    }

    pub(crate) async fn command_finish_tutorial_step(
        self: &Arc<Self>,
        _permit: &CommandContext,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<(), RuntimeError> {
        self.finish_tutorial_step(step, skipped).await
    }

    pub(crate) async fn command_finish_tutorial_step_without_guide_auto_advance(
        self: &Arc<Self>,
        _permit: &CommandContext,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<(), RuntimeError> {
        self.finish_tutorial_step_without_guide_auto_advance(step, skipped)
            .await
    }

    pub(crate) async fn command_finish_tutorial(
        self: &Arc<Self>,
        _permit: &CommandContext,
        entry: tutorial_state::TutorialFinishEntry,
    ) -> Result<(), ConfigCommitError> {
        self.finish_tutorial_from(entry).await
    }

    pub(crate) async fn command_tutorial_settings_presented(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> Result<(), RuntimeError> {
        self.tutorial_settings_presented().await
    }

    pub(crate) async fn command_restart_tutorial(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> Result<(), ConfigCommitError> {
        self.start_tutorial().await
    }

    pub(crate) async fn command_reset_setup(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> Result<(), RuntimeError> {
        self.reset_setup().await
    }

    pub(crate) async fn command_dismiss_setup_bubble(
        self: &Arc<Self>,
        _permit: &CommandContext,
        id: &str,
    ) -> Result<(), RuntimeError> {
        self.dismiss_setup_and_restart(id).await
    }

    pub(crate) async fn command_reset_conversation(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> Result<(), RuntimeError> {
        self.reset_conversation().await
    }

    pub(crate) async fn command_stop_watch(
        self: &Arc<Self>,
        permit: &CommandContext,
    ) -> Result<AppSnapshot, ConfigCommitError> {
        let _watch_intent = self.watch_intent_lock.lock().await;
        let (snapshot, applied) = self.stop_watch(permit).await;
        if !applied {
            return Ok(snapshot);
        }
        self.command_update_config_with(permit, |mut config| {
            config.watch.enabled = false;
            Ok(config)
        })
        .await?;
        Ok(snapshot)
    }

    pub(crate) async fn command_suspend_for_power(&self, _permit: &CommandContext) {
        self.suspend_for_power().await;
    }

    pub(crate) async fn command_resume_after_power(self: &Arc<Self>, permit: &CommandContext) {
        self.resume_after_power(permit).await;
    }

    pub(crate) async fn command_tutorial_watch_started(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> Result<(), ConfigCommitError> {
        self.tutorial_watch_started().await
    }

    pub(crate) async fn command_tutorial_main_opened(self: &Arc<Self>, _permit: &CommandContext) {
        self.tutorial_main_opened().await;
    }

    pub(crate) async fn command_accept_saved_tutorial_response(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> bool {
        self.accept_saved_tutorial_response().await
    }

    pub(crate) async fn command_tutorial_settings_opened(
        self: &Arc<Self>,
        _permit: &CommandContext,
    ) -> Result<bool, RuntimeError> {
        self.tutorial_settings_opened().await
    }
}
