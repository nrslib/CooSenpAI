use super::tutorial_finish::{
    activate_production_then_complete, run_after_best_effort, run_tutorial_finish_once,
    TutorialCompletionFailure,
};
use super::tutorial_notice_effects::{
    append_tutorial_conversation, tracks_tutorial_notice_progress, DesktopTutorialNoticeEffects,
};
use super::*;
use crate::bubbles;
use crate::tutorial_notice;
use coosenpai_core::conversation_archive::{archive_conversation, reset_conversation};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::onboarding::{TutorialPlaceholders, TutorialProvider, TutorialStep};
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

impl DesktopState {
    pub(crate) async fn prepare_config_for_current_mode(
        &self,
        config: &Config,
        context_notice: Option<String>,
    ) -> Result<PreparedConfigRuntime, ConfigCommitError> {
        let current_config = self.runtime.config();
        let language_changed = current_config.ui.language != config.ui.language;
        let tutorial_provider = {
            let tutorial = self.tutorial.lock().await;
            tutorial
                .state()
                .tutorial_active()
                .then(|| {
                    if language_changed {
                        Some(self.factory.tutorial_provider_for_locale(
                            super::tutorial_state::tutorial_placeholders(config),
                            Locale::from_config(&config.ui.language),
                        ))
                    } else {
                        tutorial.provider().map(Ok)
                    }
                })
                .flatten()
                .transpose()?
        };
        let runtime = match tutorial_provider {
            Some(provider) => PreparedConfigRuntime::Tutorial {
                agents: self
                    .factory
                    .build_tutorial_agents(config, provider.clone())?,
                provider: language_changed.then_some(provider),
            },
            None => PreparedConfigRuntime::Production {
                agents: self
                    .factory
                    .build_candidate_with_notice(config, context_notice)
                    .await?,
            },
        };
        Ok(runtime)
    }

    pub(crate) async fn apply_prepared_config(
        &self,
        config: Config,
        invalidates_operations: bool,
        prepared: PreparedConfigRuntime,
    ) -> Result<(), ConfigCommitError> {
        let provider = match &prepared {
            PreparedConfigRuntime::Tutorial { provider, .. } => provider.clone(),
            PreparedConfigRuntime::Production { .. } => None,
        };
        let agents = match prepared {
            PreparedConfigRuntime::Tutorial { agents, .. }
            | PreparedConfigRuntime::Production { agents } => agents,
        };
        if invalidates_operations {
            self.runtime.replace_config(config, agents).await?;
        } else {
            self.runtime
                .replace_config_when_idle(config, agents)
                .await?;
        }
        if let Some(provider) = provider {
            self.tutorial.lock().await.replace_provider(provider);
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
        transaction.commit()?;
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::tutorial_lifecycle_presenter::event(
                    crate::tutorial_lifecycle_presenter::LifecycleEvent::Started(reply),
                )
            })
            .await
            .map_err(RuntimeError::Factory)??;
        Ok(())
    }

    pub(super) async fn resume_tutorial(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let resume_watch_sequence = {
            let mut tutorial = self.tutorial.lock().await;
            let pending = tutorial.state().current_step() == Some(TutorialStep::Persona)
                && tutorial
                    .state()
                    .tutorial
                    .notices
                    .contains_key("after-persona");
            if pending {
                for key in ["after-persona", "watch-intro"] {
                    if tutorial.state().tutorial.notices.contains_key(key) {
                        tutorial
                            .reopen_notice_bubble(key)
                            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
                    }
                }
            }
            pending
        };
        self.reconcile_tutorial_notices(resume_watch_sequence)
            .await?;
        let current = {
            let mut tutorial = self.tutorial.lock().await;
            tutorial.resume();
            tutorial.state().current_step()
        };
        self.publish_tutorial_state().await;
        let follows_setup_ok = self
            .tutorial
            .lock()
            .await
            .state()
            .tutorial
            .notices
            .contains_key("setup-ok");
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::tutorial_lifecycle_presenter::event(
                    crate::tutorial_lifecycle_presenter::LifecycleEvent::Resumed {
                        current,
                        watch_sequence: resume_watch_sequence,
                        follows_setup_ok,
                        reply,
                    },
                )
            })
            .await
            .map_err(RuntimeError::Factory)?
    }

    pub(super) async fn tutorial_main_opened(self: &Arc<Self>) {
        crate::tutorial_lifecycle_presenter::main_opened(&self.tutorial, &self.ui).await;
    }

    async fn reconcile_tutorial_notices(
        self: &Arc<Self>,
        resume_watch_sequence: bool,
    ) -> Result<(), RuntimeError> {
        let excluded: &[&str] = if resume_watch_sequence {
            &[
                "setup-ok",
                "intro",
                "intro-click",
                "after-persona",
                "watch-intro",
            ]
        } else {
            &["setup-ok", "intro", "intro-click"]
        };
        match tutorial_notice::reconcile_except(
            &self.tutorial,
            &DesktopTutorialNoticeEffects::new(self.clone(), true),
            excluded,
        )
        .await?
        {
            tutorial_notice::TutorialBubbleOutcome::Acknowledged => {
                self.publish_tutorial_state().await;
                Ok(())
            }
            tutorial_notice::TutorialBubbleOutcome::Dismissed => Err(RuntimeError::Factory(
                text(
                    TextKey::TutorialGuideNotReady,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
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
                finish_state.ui.input(
                    crate::ui_events::UiView::Application,
                    crate::tutorial_lifecycle_presenter::event(
                        crate::tutorial_lifecycle_presenter::LifecycleEvent::Finished {
                            display,
                            cleanup_ok: cleanup.is_ok(),
                        },
                    ),
                );
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
                    let message = error
                        .format_for_locale(Locale::from_config(&self.runtime_config().ui.language));
                    self.publish_tutorial_state().await;
                    self.publish_event(
                        crate::snapshot_presenter::SnapshotEvent::TutorialPersistenceFailed(
                            crate::state::startup::persistence_runtime_error(message.clone()),
                        ),
                    )
                    .await;
                    return Err(error);
                }
            }
        }
        let onboarding = {
            let tutorial = self.tutorial.lock().await;
            crate::tutorial_projection::TutorialSnapshotData::read(&tutorial)
        };
        let runtime_error = self.runtime.snapshot().last_error;
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::TutorialEnded {
            tutorial: onboarding,
            runtime_error,
        })
        .await;
        let _ = self
            .ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::tutorial_lifecycle_presenter::event(
                    crate::tutorial_lifecycle_presenter::LifecycleEvent::CleanupCompleted(reply),
                )
            })
            .await;
        Ok(())
    }

    async fn enter_tutorial_finish_degraded(
        &self,
        error: &ConfigCommitError,
    ) -> Result<(), RuntimeError> {
        self.deactivate_runtime().await;
        self.runtime
            .enter_degraded(super::persona::config_commit_last_error_for_locale(
                error,
                coosenpai_core::locale::Locale::from_config(&self.runtime_config().ui.language),
            ))
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
        self.refresh_conversation().await;
        transaction
            .commit()
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        Ok(())
    }

    pub(super) async fn rebuild_runtime_after_conversation_reset(
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
        self.refresh_conversation().await;
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
        bubbles::mutate(
            self,
            bubbles::BubbleMutation::ConversationGeneration(generation),
        )
        .await;
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
        bubbles::mutate(
            self,
            bubbles::BubbleMutation::ConversationGeneration(generation),
        )
        .await;
        Ok(())
    }

    pub(super) async fn publish_tutorial_state(&self) {
        let data = {
            let tutorial = self.tutorial.lock().await;
            crate::tutorial_projection::TutorialSnapshotData::read(&tutorial)
        };
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::TutorialLoaded(
            data,
        ))
        .await;
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
            RuntimeError::Factory(
                text(
                    TextKey::TutorialNotStarted,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
            )
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
        self.present_tutorial_notice(
            id,
            key.to_owned(),
            created_at,
            message,
            require_ack,
            replaced_bubble_ids,
            transition_cancellation,
        )
        .await
    }

    pub(crate) async fn present_pending_tutorial_response(self: &Arc<Self>) {
        let entry = self.pending_tutorial_response_entry().await;
        let config = Box::new(self.runtime.config());
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        let _ = self
            .ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::ui_events::UiEvent::Tutorial(Box::new(
                    crate::tutorial_events::TutorialEvent::Response(
                        crate::tutorial_response_presenter::ResponseEvent::PendingLoaded {
                            entry,
                            config,
                            conversation_generation,
                            reply,
                        },
                    ),
                ))
            })
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

pub(crate) enum PreparedConfigRuntime {
    Tutorial {
        agents: coosenpai_core::runtime::RuntimeAgents,
        provider: Option<TutorialProvider>,
    },
    Production {
        agents: coosenpai_core::runtime::RuntimeAgents,
    },
}

#[derive(Debug)]
pub(crate) struct TutorialPresentation {
    pub(crate) completion: crate::tutorial_response_presenter::ResponseCompletion,
    pub(crate) presentation: crate::bubbles::BubblePresentation,
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
    shortcut_label_for_locale(value, Locale::Ja)
}

pub(crate) fn shortcut_label_for_locale(value: Option<&str>, locale: Locale) -> String {
    value.map_or_else(
        || text(TextKey::ShortcutUnset, locale).to_owned(),
        |value| {
            value
                .replace("CommandOrControl+", "⌘")
                .replace("Control+", "⌃")
                .replace("Alt+", "⌥")
                .replace("Shift+", "⇧")
                .replace("Space", text(TextKey::ShortcutSpace, locale))
        },
    )
}

