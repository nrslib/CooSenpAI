use super::tutorial_finish::{
    activate_production_then_complete, run_after_best_effort, run_tutorial_finish_once,
    TutorialCompletionFailure,
};
use super::tutorial_notice_effects::{
    append_tutorial_conversation, tracks_tutorial_notice_progress, tutorial_bubble_record,
    DesktopTutorialNoticeEffects,
};
use super::*;
use crate::bubbles::{self, BubbleRecord};
use crate::tutorial_notice;
use coosenpai_core::conversation_archive::{archive_conversation, reset_conversation};
use coosenpai_core::onboarding::{TutorialPlaceholders, TutorialStep};
use coosenpai_core::state::ConversationRole;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TutorialFinishEntry {
    Automatic,
    Main,
}

impl TutorialFinishEntry {
    fn message_key(self) -> &'static str {
        match self {
            Self::Automatic => "finish",
            Self::Main => "forced-finish",
        }
    }
}

fn finish_display_is_accepted(
    outcome: &Result<tutorial_notice::TutorialBubbleOutcome, RuntimeError>,
) -> bool {
    matches!(
        outcome,
        Ok(tutorial_notice::TutorialBubbleOutcome::Acknowledged)
    )
}

impl DesktopState {
    pub(crate) async fn replace_config_for_current_mode(
        &self,
        config: Config,
        invalidates_operations: bool,
    ) -> Result<(), ConfigCommitError> {
        self.replace_config_for_current_mode_with_notice(config, invalidates_operations, None)
            .await
    }

    pub(crate) async fn replace_config_for_current_mode_with_notice(
        &self,
        config: Config,
        invalidates_operations: bool,
        context_notice: Option<String>,
    ) -> Result<(), ConfigCommitError> {
        if invalidates_operations {
            self.runtime.quiesce_for_config_update().await?;
        }
        let tutorial_provider = {
            let tutorial = self.tutorial.lock().await;
            tutorial
                .state()
                .tutorial_active()
                .then(|| tutorial.provider())
                .flatten()
        };
        let agents = match tutorial_provider {
            Some(provider) => self.factory.build_tutorial_agents(&config, provider)?,
            None => {
                self.factory
                    .build_candidate_with_notice(&config, context_notice)
                    .await?
            }
        };
        if invalidates_operations {
            self.runtime.replace_config(config, agents).await?;
        } else {
            self.runtime
                .replace_config_when_idle(config, agents)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn start_tutorial(self: &Arc<Self>) -> Result<(), ConfigCommitError> {
        let transaction = self.config_update.begin().await;
        self.stop_watch_internal(true).await;
        self.archive_conversation_for_tutorial().await?;
        let config = self.runtime.config();
        let placeholders = tutorial_placeholders(&config);
        let (agents, provider) = self
            .factory
            .build_tutorial_candidate(&config, placeholders)
            .await?;
        {
            let mut tutorial = self.tutorial.lock().await;
            tutorial
                .restart(provider)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        }
        self.runtime.replace_config(config, agents).await?;
        self.activate_runtime();
        self.publish_tutorial_state().await;
        crate::windows::hide_main(&self.app);
        transaction.commit()?;
        self.emit_tutorial_intro_sequence(false).await?;
        Ok(())
    }

    pub(super) async fn resume_tutorial(self: &Arc<Self>) -> Result<(), RuntimeError> {
        self.reconcile_tutorial_notices().await?;
        let current = {
            let mut tutorial = self.tutorial.lock().await;
            tutorial.resume();
            tutorial.state().current_step()
        };
        self.publish_tutorial_state().await;
        if let Some(key) = current.and_then(step_intro_key) {
            let mut tutorial = self.tutorial.lock().await;
            if tutorial.state().tutorial.notices.contains_key(key) {
                tutorial
                    .reopen_notice_bubble(key)
                    .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            }
            drop(tutorial);
            let outcome = self.emit_tutorial_message(key).await?;
            if outcome == tutorial_notice::TutorialBubbleOutcome::Acknowledged {
                if let Some(step) = current {
                    self.schedule_tutorial_guide_auto_advance(step);
                }
            }
        } else if current == Some(TutorialStep::Chat) {
            let follows_setup_ok = {
                let mut tutorial = self.tutorial.lock().await;
                let follows_setup_ok = tutorial.state().tutorial.notices.contains_key("setup-ok");
                if tutorial
                    .state()
                    .tutorial
                    .notices
                    .contains_key("intro-click")
                {
                    tutorial
                        .reopen_notice_bubble("intro-click")
                        .map_err(|error| RuntimeError::Factory(error.to_string()))?;
                }
                follows_setup_ok
            };
            self.emit_tutorial_intro_sequence(follows_setup_ok).await?;
        }
        Ok(())
    }

    pub(super) async fn tutorial_main_opened(self: &Arc<Self>) {
        let should_emit = {
            let mut tutorial = self.tutorial.lock().await;
            tutorial.take_chat_opened()
        };
        if should_emit {
            let accepted = matches!(
                self.emit_tutorial_message("after-open").await,
                Ok(tutorial_notice::TutorialBubbleOutcome::Acknowledged)
            );
            if !accepted {
                self.tutorial.lock().await.chat_open_presentation_failed();
            }
            self.publish_tutorial_state().await;
        }
    }

    async fn reconcile_tutorial_notices(self: &Arc<Self>) -> Result<(), RuntimeError> {
        match tutorial_notice::reconcile_except(
            &self.tutorial,
            &DesktopTutorialNoticeEffects::new(self.clone(), true),
            &["setup-ok", "intro", "intro-click"],
        )
        .await?
        {
            tutorial_notice::TutorialBubbleOutcome::Acknowledged => {
                self.publish_tutorial_state().await;
                Ok(())
            }
            tutorial_notice::TutorialBubbleOutcome::Dismissed => Err(RuntimeError::Factory(
                "チュートリアル案内の表示確認が完了していません".to_owned(),
            )),
        }
    }

    pub(super) async fn finish_tutorial_from(
        self: &Arc<Self>,
        entry: TutorialFinishEntry,
    ) -> Result<(), ConfigCommitError> {
        let transaction = self.config_update.begin().await;
        self.cancel_tutorial_sequence().await;
        let active_state = self.clone();
        let finish_state = self.clone();
        run_tutorial_finish_once(
            transaction,
            move || async move { active_state.tutorial.lock().await.state().tutorial_active() },
            move || async move {
                let (display, cleanup) = run_after_best_effort(
                    async {
                        finish_state
                            .emit_tutorial_message_with_ack(entry.message_key(), false)
                            .await
                    },
                    finish_state.finish_tutorial_cleanup(),
                )
                .await;
                if finish_display_is_accepted(&display) && cleanup.is_ok() {
                    crate::windows::show_main(&finish_state.app);
                }
                cleanup
            },
        )
        .await
    }

    async fn finish_tutorial_cleanup(self: &Arc<Self>) -> Result<(), ConfigCommitError> {
        let production_restored = self.tutorial.lock().await.production_restored();
        if !production_restored {
            self.stop_watch_internal(true).await;
            self.runtime.quiesce_for_conversation_reset().await?;
            self.deactivate_runtime().await;
            if let Err(error) = self.tutorial.lock().await.prepare_finish() {
                let error = ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string()));
                self.enter_tutorial_finish_degraded(&error).await?;
                return Err(error);
            }
        }
        let config = self.runtime.config();
        let replacement = config.clone();
        if !production_restored {
            if let Err(error) = self
                .archive_conversation_storage(config.retention.conversation_days)
                .await
            {
                let error = ConfigCommitError::Runtime(error);
                self.enter_tutorial_finish_degraded(&error).await?;
                return Err(error);
            }
        }
        let activation = activate_production_then_complete(
            production_restored,
            || async {
                self.factory
                    .build_candidate(&config)
                    .await
                    .map_err(ConfigCommitError::Factory)
            },
            |agents| async {
                self.runtime
                    .replace_config(replacement, agents)
                    .await
                    .map_err(ConfigCommitError::Runtime)?;
                self.tutorial.lock().await.mark_production_restored();
                self.activate_runtime();
                Ok(())
            },
            |error| async move {
                self.enter_tutorial_finish_degraded(&error)
                    .await
                    .map_err(ConfigCommitError::Runtime)
            },
            || async {
                self.tutorial.lock().await.finish().map_err(|error| {
                    ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string()))
                })
            },
        )
        .await;
        if let Err(error) = activation {
            match error {
                TutorialCompletionFailure::Activation(error) => return Err(error),
                TutorialCompletionFailure::Persistence(error) => {
                    let message = error.format_for_user();
                    self.publish_tutorial_state().await;
                    self.publish(|snapshot| {
                        snapshot.last_error = Some(
                            crate::state::startup::persistence_runtime_error(message.clone()),
                        );
                    })
                    .await;
                    return Err(error);
                }
            }
        }
        let onboarding = {
            let tutorial = self.tutorial.lock().await;
            tutorial_onboarding_view(&tutorial)
        };
        let runtime_error = self.runtime.snapshot().last_error;
        self.publish(|snapshot| {
            snapshot.conversation.clear();
            snapshot.onboarding = onboarding;
            snapshot.last_error = runtime_error;
        })
        .await;
        crate::windows::sync_tutorial(&self.app, false);
        crate::bubble_conversation::show_tutorial_complete(self.clone()).await;
        Ok(())
    }

    async fn enter_tutorial_finish_degraded(
        &self,
        error: &ConfigCommitError,
    ) -> Result<(), RuntimeError> {
        self.deactivate_runtime().await;
        self.runtime
            .enter_degraded(super::persona::config_commit_last_error(error))
            .await?;
        self.publish_tutorial_state().await;
        Ok(())
    }

    pub(super) async fn reset_conversation(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let transaction = self.config_update.begin().await;
        self.runtime.quiesce_for_conversation_reset().await?;
        self.deactivate_runtime().await;
        let config = self.runtime.config();
        self.reset_conversation_storage().await?;
        self.rebuild_runtime_after_conversation_reset(&config)
            .await?;
        self.activate_runtime();
        self.publish(|snapshot| snapshot.conversation.clear()).await;
        transaction
            .commit()
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        Ok(())
    }

    async fn rebuild_runtime_after_conversation_reset(
        &self,
        config: &Config,
    ) -> Result<(), RuntimeError> {
        let agents = {
            let provider = self.tutorial.lock().await.provider();
            match provider {
                Some(provider) => self
                    .factory
                    .build_tutorial_agents(config, provider)
                    .map_err(|error| RuntimeError::Factory(error.to_string()))?,
                None => self
                    .factory
                    .build_candidate(config)
                    .await
                    .map_err(|error| RuntimeError::Factory(error.to_string()))?,
            }
        };
        self.runtime.replace_config(config.clone(), agents).await?;
        Ok(())
    }

    pub(super) async fn archive_conversation_for_tutorial(&self) -> Result<(), RuntimeError> {
        self.runtime.quiesce_for_conversation_reset().await?;
        self.deactivate_runtime().await;
        let retention = self.runtime.config().retention.conversation_days;
        self.archive_conversation_storage(retention).await?;
        self.publish(|snapshot| snapshot.conversation.clear()).await;
        Ok(())
    }

    async fn reset_conversation_storage(&self) -> Result<(), RuntimeError> {
        let _conversation_sync = self.conversation_sync.lock().await;
        let paths = self.paths.clone();
        let generation =
            tokio::task::spawn_blocking(move || reset_conversation(&paths, chrono::Utc::now()))
                .await
                .map_err(|error| RuntimeError::Factory(error.to_string()))?
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        let changed = self
            .bubbles
            .lock()
            .await
            .advance_conversation_generation(generation);
        if changed {
            let _ = bubbles::sync_window(self).await;
        }
        Ok(())
    }

    async fn archive_conversation_storage(&self, retention: u64) -> Result<(), RuntimeError> {
        let _conversation_sync = self.conversation_sync.lock().await;
        let paths = self.paths.clone();
        let generation = tokio::task::spawn_blocking(move || {
            archive_conversation(&paths, retention, chrono::Utc::now())?;
            coosenpai_core::conversation_archive::current_conversation_generation(&paths)
        })
        .await
        .map_err(|error| RuntimeError::Factory(error.to_string()))?
        .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        let changed = self
            .bubbles
            .lock()
            .await
            .advance_conversation_generation(generation);
        if changed {
            let _ = bubbles::sync_window(self).await;
        }
        Ok(())
    }

    pub(super) async fn publish_tutorial_state(&self) {
        let (view, active) = {
            let tutorial = self.tutorial.lock().await;
            (
                tutorial_onboarding_view(&tutorial),
                tutorial.state().tutorial_active(),
            )
        };
        let setup_required = view.setup_required;
        self.publish(|snapshot| snapshot.onboarding = view).await;
        crate::windows::sync_onboarding(&self.app, setup_required, active);
    }

    pub(super) async fn emit_tutorial_message(
        self: &Arc<Self>,
        key: &str,
    ) -> Result<tutorial_notice::TutorialBubbleOutcome, RuntimeError> {
        self.emit_tutorial_message_with_ack(key, true).await
    }

    pub(super) async fn emit_tutorial_message_with_ack(
        self: &Arc<Self>,
        key: &str,
        require_ack: bool,
    ) -> Result<tutorial_notice::TutorialBubbleOutcome, RuntimeError> {
        self.emit_tutorial_message_configured(
            key,
            require_ack,
            Vec::new(),
            self.cancellation.clone(),
        )
        .await
    }

    pub(super) async fn emit_tutorial_message_replacing(
        self: &Arc<Self>,
        key: &str,
        replaced_bubble_ids: Vec<String>,
        transition_cancellation: CancellationToken,
    ) -> Result<tutorial_notice::TutorialBubbleOutcome, RuntimeError> {
        self.emit_tutorial_message_configured(
            key,
            true,
            replaced_bubble_ids,
            transition_cancellation,
        )
        .await
    }

    async fn emit_tutorial_message_configured(
        self: &Arc<Self>,
        key: &str,
        require_ack: bool,
        replaced_bubble_ids: Vec<String>,
        transition_cancellation: CancellationToken,
    ) -> Result<tutorial_notice::TutorialBubbleOutcome, RuntimeError> {
        let provider = self.tutorial.lock().await.provider().ok_or_else(|| {
            RuntimeError::Factory("チュートリアルが開始されていません".to_owned())
        })?;
        let message = provider
            .render(key)
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        if tracks_tutorial_notice_progress(key) {
            return tutorial_notice::deliver(
                &self.tutorial,
                key,
                &message,
                &DesktopTutorialNoticeEffects::replacing(
                    self.clone(),
                    require_ack,
                    replaced_bubble_ids,
                    transition_cancellation,
                ),
            )
            .await;
        }
        let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let id = format!("tutorial-{}", Uuid::new_v4());
        append_tutorial_conversation(self, &id, key, &created_at, &message).await?;
        let config = self.runtime.config();
        let record = tutorial_bubble_record(self, id, key, created_at, message).await;
        if require_ack {
            return bubbles::show(self.clone(), record, config.notification.bubble_duration_ms)
                .await
                .map(|outcome| match outcome {
                    bubbles::BubblePresentationOutcome::Acknowledged => {
                        tutorial_notice::TutorialBubbleOutcome::Acknowledged
                    }
                    bubbles::BubblePresentationOutcome::Dismissed => {
                        tutorial_notice::TutorialBubbleOutcome::Dismissed
                    }
                })
                .map_err(|error| RuntimeError::Factory(error.to_string()));
        }
        bubbles::show_best_effort(self.clone(), record, config.notification.bubble_duration_ms)
            .await;
        Ok(tutorial_notice::TutorialBubbleOutcome::Acknowledged)
    }

    pub(super) async fn prepare_pending_tutorial_response(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
    ) -> Option<TutorialPresentation> {
        let entry = self.pending_tutorial_response_entry().await?;
        let should_present = self
            .tutorial
            .lock()
            .await
            .begin_response_presentation(&entry.id, &entry.message);
        if !should_present {
            return None;
        }
        let config = self.runtime.config();
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        let entry_id = entry.id.clone();
        let message = entry.message.clone();
        let presentation = bubbles::register(
            self,
            BubbleRecord {
                id: entry.id.clone(),
                created_at: entry.created_at,
                message: entry.message,
                message_kind: "tutorial".to_owned(),
                notification_priority: "none".to_owned(),
                caused_by: entry.caused_by_ids.last().cloned(),
                display_name: config.companion.display_name,
                persona: config.companion.persona,
                avatar_color: config.ui.avatar_color,
                conversation_generation,
                persistent: true,
                open_url: None,
                interaction: None,
            },
            config.notification.bubble_duration_ms,
        )
        .await;
        let Ok(presentation) = presentation else {
            self.tutorial
                .lock()
                .await
                .response_presentation_failed(&entry_id);
            return None;
        };
        Some(TutorialPresentation {
            entry_id,
            message,
            presentation,
            conversation: permit.fence(crate::command_guard::GenerationResource::Conversation)?,
            bubble: permit.fence(crate::command_guard::GenerationResource::Bubble)?,
        })
    }

    pub(crate) async fn present_pending_tutorial_response(self: &Arc<Self>) {
        if !self.tutorial_is_active().await
            || self
                .tutorial
                .lock()
                .await
                .expected_response_message()
                .is_none()
        {
            return;
        }
        if crate::windows::main_is_focused(&self.app) {
            let handler_state = self.clone();
            let _ = self
                .dispatch(
                    crate::command_guard::CommandSource::RuntimeMonitor,
                    crate::command_guard::DesktopCommand::PresentTutorialResponse,
                    move |_context| async move {
                        handler_state
                            .accept_pending_tutorial_response_in_main()
                            .await;
                        Ok(())
                    },
                )
                .await;
            return;
        }
        let handler_state = self.clone();
        let prepared = self
            .dispatch(
                crate::command_guard::CommandSource::RuntimeMonitor,
                crate::command_guard::DesktopCommand::PresentTutorialResponse,
                move |context| async move {
                    Ok(handler_state
                        .prepare_pending_tutorial_response(&context)
                        .await)
                },
            )
            .await;
        let Ok(Some(prepared)) = prepared else { return };
        let accepted = matches!(
            bubbles::complete_presentation(self.clone(), prepared.presentation).await,
            Ok(bubbles::BubblePresentationOutcome::Acknowledged)
        );
        let entry_id = prepared.entry_id;
        let message = prepared.message;
        let handler_state = self.clone();
        let _ = self
            .dispatch_with_fences(
                crate::command_guard::CommandSource::RuntimeMonitor,
                crate::command_guard::DesktopCommand::PresentTutorialResponse,
                [prepared.conversation, prepared.bubble],
                move |_context| async move {
                    if accepted {
                        handler_state
                            .advance_after_tutorial_response(&entry_id, &message)
                            .await;
                    } else {
                        handler_state
                            .tutorial
                            .lock()
                            .await
                            .response_presentation_failed(&entry_id);
                    }
                    Ok(())
                },
            )
            .await;
    }

    async fn pending_tutorial_response_entry(
        &self,
    ) -> Option<coosenpai_core::state::ConversationEntry> {
        let expected = self.tutorial.lock().await.expected_response_message()?;
        self.snapshot()
            .await
            .conversation
            .into_iter()
            .rev()
            .find(|entry| {
                entry.role == ConversationRole::Companion
                    && !entry.caused_by_ids.is_empty()
                    && entry.message == expected
            })
    }

    async fn accept_pending_tutorial_response_in_main(self: &Arc<Self>) {
        let Some(entry) = self.pending_tutorial_response_entry().await else {
            return;
        };
        let should_present = self
            .tutorial
            .lock()
            .await
            .begin_response_presentation(&entry.id, &entry.message);
        if should_present {
            self.advance_after_tutorial_response(&entry.id, &entry.message)
                .await;
        }
    }

    pub(super) async fn accept_saved_tutorial_response(self: &Arc<Self>) -> bool {
        let Some(entry) = self.pending_tutorial_response_entry().await else {
            return false;
        };
        let accepted = self
            .tutorial
            .lock()
            .await
            .response_presentation_accepted(&entry.id, &entry.message)
            .is_some();
        if accepted {
            self.publish_tutorial_state().await;
        }
        accepted || self.tutorial_step_response_presented().await
    }
}

pub(crate) struct TutorialPresentation {
    entry_id: String,
    message: String,
    presentation: crate::bubbles::BubblePresentation,
    conversation: crate::command_guard::GenerationStamp,
    bubble: crate::command_guard::GenerationStamp,
}

fn tutorial_onboarding_view(
    tutorial: &crate::tutorial::TutorialController,
) -> crate::snapshot::OnboardingView {
    let mut view = crate::snapshot::OnboardingView::from_state_and_resume(
        tutorial.state(),
        tutorial.resume_pending(),
    );
    view.skip_hint = tutorial.skip_hint();
    view.chat_input_enabled = tutorial.chat_input_enabled();
    view.settings_highlight = tutorial
        .settings_highlight_pending()
        .map(|highlight| highlight.as_str().to_owned());
    view
}

pub(super) fn step_intro_key(step: TutorialStep) -> Option<&'static str> {
    match step {
        TutorialStep::Text => Some("text-intro"),
        TutorialStep::Image => Some("image-intro"),
        TutorialStep::Voice => Some("voice-intro"),
        TutorialStep::Persona => Some("persona-intro"),
        TutorialStep::Watch => Some("watch-intro"),
        TutorialStep::Chat => None,
    }
}

pub(crate) fn tutorial_placeholders(config: &Config) -> TutorialPlaceholders {
    TutorialPlaceholders {
        display_name: config.companion.display_name.clone(),
        send_text: shortcut_label(config.keymap.send_text.as_deref()),
        capture_region: shortcut_label(config.keymap.capture_region.as_deref()),
        microphone: shortcut_label(config.keymap.microphone.as_deref()),
        toggle_watch: shortcut_label(config.keymap.toggle_watch.as_deref()),
    }
}

pub(crate) fn shortcut_label(value: Option<&str>) -> String {
    value.map_or_else(
        || "未設定".to_owned(),
        |value| {
            value
                .replace("CommandOrControl+", "⌘")
                .replace("Control+", "⌃")
                .replace("Alt+", "⌥")
                .replace("Shift+", "⇧")
                .replace("Space", "空白")
        },
    )
}

