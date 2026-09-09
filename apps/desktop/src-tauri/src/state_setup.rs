use super::*;
use crate::bubbles::{
    self, BubbleInteraction, BubbleOption, BubbleRecord, BubbleSecretInput, BubbleSelect,
};
use crate::tutorial::{
    tutorial_step_can_be_skipped, SetupConnectionMethod, SetupPhase, TUTORIAL_SKIP_ACTION,
};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::onboarding::TutorialProvider;
use coosenpai_core::provider::ProviderName;
use coosenpai_core::runtime::RuntimeAgents;
use std::future::Future;
use std::time::Duration;

const SETUP_MESSAGE_KIND: &str = "setup";
const SETUP_CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
enum SetupConnectionWait<T, E> {
    Completed(Result<T, E>),
    Cancelled,
    TimedOut,
}

async fn wait_for_setup_connection<T, E, F>(
    attempt_cancellation: &tokio_util::sync::CancellationToken,
    operation_cancellation: &tokio_util::sync::CancellationToken,
    timeout: Duration,
    operation: F,
) -> SetupConnectionWait<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::pin!(operation);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let stopped = tokio::select! {
        biased;
        () = attempt_cancellation.cancelled() => {
            SetupConnectionWait::Cancelled
        }
        result = &mut operation => return SetupConnectionWait::Completed(result),
        () = &mut deadline => SetupConnectionWait::TimedOut,
    };
    operation_cancellation.cancel();
    let _ = operation.await;
    stopped
}

#[derive(Debug, PartialEq, Eq)]
enum SetupPromptEffect {
    RenderIntro,
    Redisplay(Box<BubbleRecord>),
    AutoResumeTutorial,
    None,
}

fn setup_prompt_effect(
    needs_setup: bool,
    tutorial_active: bool,
    existing: Option<BubbleRecord>,
) -> SetupPromptEffect {
    match (needs_setup, tutorial_active, existing) {
        (true, _, Some(record)) => SetupPromptEffect::Redisplay(Box::new(record)),
        (true, _, None) => SetupPromptEffect::RenderIntro,
        (false, true, _) => SetupPromptEffect::AutoResumeTutorial,
        (false, false, _) => SetupPromptEffect::None,
    }
}

impl DesktopState {
    pub(super) async fn announce_initial_onboarding(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let (needs_setup, needs_language_selection, tutorial_active) = {
            let tutorial = self.tutorial.lock().await;
            (
                tutorial.state().needs_setup(),
                tutorial.state().needs_language_selection(),
                tutorial.state().tutorial_active(),
            )
        };
        if needs_language_selection {
            return self.emit_setup_language_choice(true).await;
        }
        let existing = if needs_setup {
            self.bubbles
                .lock()
                .await
                .record_for_message_kind(SETUP_MESSAGE_KIND)
        } else {
            None
        };
        match setup_prompt_effect(needs_setup, tutorial_active, existing) {
            SetupPromptEffect::Redisplay(record) => {
                self.present_setup_record(
                    *record,
                    self.runtime_config().notification.bubble_duration_ms,
                    true,
                    true,
                )
                .await
            }
            SetupPromptEffect::RenderIntro => self.emit_setup_choice("setup-intro", true).await,
            SetupPromptEffect::AutoResumeTutorial => self.resume_tutorial().await,
            SetupPromptEffect::None => Ok(()),
        }
    }

    pub(super) async fn handle_bubble_interaction(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
        id: &str,
        action: &str,
        value: Option<&str>,
    ) -> Result<(), ConfigCommitError> {
        if !self
            .bubbles
            .lock()
            .await
            .accepts_interaction(id, action, value)
        {
            return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::SetupExpired,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
            )));
        }
        let locale = Locale::from_config(&self.runtime.config().ui.language);
        match action {
            TUTORIAL_SKIP_ACTION => {
                let step = self.tutorial_current_step().await.ok_or_else(|| {
                    ConfigCommitError::Runtime(RuntimeError::Factory(
                        text(TextKey::TutorialCurrentStepMissing, locale).to_owned(),
                    ))
                })?;
                if !tutorial_step_can_be_skipped(step) {
                    return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                        text(TextKey::TutorialAutoAdvance, locale).to_owned(),
                    )));
                }
                self.command_finish_tutorial_step(permit, step, true)
                    .await?;
                Ok(())
            }
            "setup-language-select" => self.select_setup_language(permit, value).await,
            "setup-provider-select" => {
                let provider = value.ok_or_else(|| {
                    ConfigCommitError::Runtime(RuntimeError::Factory(
                        text(TextKey::SetupProviderRequired, locale).to_owned(),
                    ))
                })?;
                self.select_setup_provider(provider).await
            }
            "setup-method-select" => {
                let method = value
                    .and_then(SetupConnectionMethod::parse)
                    .ok_or_else(|| {
                        ConfigCommitError::Runtime(RuntimeError::Factory(
                            text(TextKey::SetupConnectionMethodRequired, locale).to_owned(),
                        ))
                    })?;
                self.select_setup_connection_method(method).await
            }
            "setup-api-key-submit" => {
                let api_key = value.ok_or_else(|| {
                    ConfigCommitError::Runtime(RuntimeError::Factory(
                        text(TextKey::SetupApiKeyRequired, locale).to_owned(),
                    ))
                })?;
                if api_key.trim().is_empty() {
                    return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                        text(TextKey::SetupApiKeyRequired, locale).to_owned(),
                    )));
                }
                if api_key.contains('\0') {
                    return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                        text(TextKey::SetupApiKeyInvalid, locale).to_owned(),
                    )));
                }
                let provider = {
                    let tutorial = self.tutorial.lock().await;
                    if tutorial.setup_connection_method() != Some(SetupConnectionMethod::ApiKey) {
                        return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                            text(TextKey::SetupApiKeyNotAccepted, locale).to_owned(),
                        )));
                    }
                    tutorial.setup_selected().to_owned()
                };
                self.connect_setup_provider(
                    &provider,
                    SetupConnectionMethod::ApiKey,
                    Some(api_key.to_owned()),
                )
                .await
            }
            "setup-settings" => {
                self.ui.input(
                    crate::ui_events::UiView::Bubble,
                    crate::ui_events::UiEvent::OpenSettings,
                );
                Ok(())
            }
            "setup-retry" => {
                self.emit_setup_choice("setup-intro", false).await?;
                Ok(())
            }
            "memory-confirm" => self.resolve_fact_prompt(id, true).await,
            "memory-reject" => self.resolve_fact_prompt(id, false).await,
            "conversation-reset-confirm" => {
                self.command_reset_conversation(permit).await?;
                crate::bubble_conversation::show_reset_complete(self.clone()).await;
                Ok(())
            }
            "conversation-reset-cancel" => {
                bubbles::dismiss(self.as_ref(), id).await;
                Ok(())
            }
            "watch-fullscreen-settings" => {
                bubbles::complete_action(self.as_ref(), id).await;
                self.ui.input(
                    crate::ui_events::UiView::Bubble,
                    crate::ui_events::UiEvent::OpenSettingsAt("watch"),
                );
                Ok(())
            }
            _ => Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(TextKey::InvalidBubbleAction, locale).to_owned(),
            ))),
        }
    }

    async fn select_setup_provider(
        self: &Arc<Self>,
        provider: &str,
    ) -> Result<(), ConfigCommitError> {
        self.tutorial
            .lock()
            .await
            .select_setup_provider(provider)
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        self.emit_setup_choice("setup-intro", false)
            .await
            .map_err(ConfigCommitError::from)
    }

    async fn select_setup_language(
        self: &Arc<Self>,
        _permit: &crate::command_guard::CommandContext,
        value: Option<&str>,
    ) -> Result<(), ConfigCommitError> {
        let language = value.ok_or_else(|| {
            ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::SetupLanguageRequired,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
            ))
        })?;
        let locale = match language {
            "ja" => Locale::Ja,
            "en" => Locale::En,
            _ => {
                return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                    text(
                        TextKey::SetupLanguageRequired,
                        Locale::from_config(&self.runtime.config().ui.language),
                    )
                    .to_owned(),
                )))
            }
        };
        let previous = self.runtime.config();
        let provider = self.factory.tutorial_provider_for_locale(
            super::tutorial_state::tutorial_placeholders(&previous),
            locale,
        )?;
        let transaction = self.config_update.begin().await;
        let config =
            coosenpai_core::config::patch_config(&self.paths, Some(&previous), |mut current| {
                current.ui.language = language.to_owned();
                Ok(current)
            })?;
        self.runtime
            .replace_config(
                config.clone(),
                RuntimeAgents {
                    observation_delivery: coosenpai_core::runtime::ObservationDelivery::Companion,
                    observer: None,
                    companion: None,
                    memory: None,
                },
            )
            .await?;
        transaction.commit_config(config.revision)?;
        {
            let mut tutorial = self.tutorial.lock().await;
            tutorial
                .select_setup_language(language)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            tutorial.attach_setup_provider(provider);
        }
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigLoaded(
            config,
        ))
        .await;
        self.emit_setup_choice("setup-intro", false)
            .await
            .map_err(ConfigCommitError::from)
    }

    async fn select_setup_connection_method(
        self: &Arc<Self>,
        method: SetupConnectionMethod,
    ) -> Result<(), ConfigCommitError> {
        let provider = {
            let mut tutorial = self.tutorial.lock().await;
            tutorial
                .select_setup_connection_method(method)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            tutorial.setup_selected().to_owned()
        };
        match method {
            SetupConnectionMethod::Login => {
                self.connect_setup_provider(&provider, method, None).await
            }
            SetupConnectionMethod::ApiKey => self
                .emit_setup_choice("setup-intro", false)
                .await
                .map_err(ConfigCommitError::from),
        }
    }

    async fn connect_setup_provider(
        self: &Arc<Self>,
        provider_name: &str,
        method: SetupConnectionMethod,
        api_key: Option<String>,
    ) -> Result<(), ConfigCommitError> {
        if method == SetupConnectionMethod::ApiKey
            && api_key.as_deref().is_none_or(|key| key.trim().is_empty())
        {
            return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::SetupApiKeyRequired,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
            )));
        }
        if method == SetupConnectionMethod::ApiKey
            && api_key.as_deref().is_some_and(|key| key.contains('\0'))
        {
            return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::SetupApiKeyInvalid,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
            )));
        }
        if method == SetupConnectionMethod::Login && api_key.is_some() {
            return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::SetupLoginApiKeyRejected,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
            )));
        }
        let attempt = self
            .tutorial
            .lock()
            .await
            .begin_setup_connection(provider_name, method, &self.cancellation)
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        if !self
            .emit_setup_attempt_status(&attempt, "setup-connecting", None, true)
            .await?
        {
            return Ok(());
        }
        let attempt_cancellation = attempt.cancellation();
        let operation_cancellation = attempt_cancellation.child_token();
        let call_cancellation = operation_cancellation.clone();
        let checked = wait_for_setup_connection(
            &attempt_cancellation,
            &operation_cancellation,
            SETUP_CONNECTION_TIMEOUT,
            async {
                let capabilities = self
                    .factory
                    .provider_capabilities(provider_name, call_cancellation.clone())
                    .await?;
                self.factory
                    .check_connection(
                        provider_name,
                        &capabilities.default_model,
                        None,
                        api_key.as_deref(),
                        call_cancellation,
                    )
                    .await?;
                Ok::<String, crate::factory::DesktopFactoryError>(capabilities.default_model)
            },
        )
        .await;
        let model = match checked {
            SetupConnectionWait::Completed(Ok(model)) => model,
            SetupConnectionWait::Completed(Err(error)) => {
                let detail = redact_setup_error_detail(
                    &error
                        .format_for_locale(Locale::from_config(&self.runtime.config().ui.language)),
                    api_key.as_deref(),
                );
                self.emit_setup_attempt_failure_serialized(&attempt, provider_name, detail)
                    .await?;
                return Ok(());
            }
            SetupConnectionWait::Cancelled => return Ok(()),
            SetupConnectionWait::TimedOut => {
                self.emit_setup_attempt_failure_serialized(
                    &attempt,
                    provider_name,
                    text(
                        TextKey::SetupConnectionTimeout,
                        Locale::from_config(&self.runtime.config().ui.language),
                    )
                    .to_owned(),
                )
                .await?;
                return Ok(());
            }
        };
        let transaction = self.config_update.begin().await;
        let mut api_key_rollback = None;
        if let Some(api_key) = api_key.as_deref() {
            let provider = setup_provider_name(
                provider_name,
                Locale::from_config(&self.runtime.config().ui.language),
            )?;
            if !self
                .tutorial
                .lock()
                .await
                .setup_attempt_is_current(&attempt)
            {
                return Ok(());
            }
            let previous = self.factory.provider_api_key_value(provider)?;
            if let Err(error) = self
                .factory
                .update_provider_api_key(provider, Some(api_key))
                .await
            {
                let rollback_error = self
                    .factory
                    .restore_provider_api_key(provider, previous.as_deref())
                    .await
                    .err();
                drop(transaction);
                let error = rollback_error.unwrap_or(error);
                let detail = redact_setup_error_detail(
                    &error
                        .format_for_locale(Locale::from_config(&self.runtime.config().ui.language)),
                    Some(api_key),
                );
                self.emit_setup_attempt_failure_serialized(&attempt, provider_name, detail)
                    .await?;
                return Ok(());
            }
            api_key_rollback = Some((provider, previous));
        }
        let completed = match self
            .complete_connected_setup(&attempt, provider_name.to_owned(), model)
            .await
        {
            Ok(completed) => completed,
            Err(error) => {
                let retryable = matches!(
                    self.tutorial.lock().await.setup_phase(),
                    crate::tutorial::SetupPhase::Connecting { .. }
                );
                let rollback_result = self.rollback_setup_api_key(&mut api_key_rollback).await;
                drop(transaction);
                rollback_result?;
                if retryable {
                    let detail = redact_setup_error_detail(
                        &error.format_for_locale(Locale::from_config(
                            &self.runtime_config().ui.language,
                        )),
                        api_key.as_deref(),
                    );
                    self.emit_setup_attempt_failure(&attempt, provider_name, detail)
                        .await?;
                    return Ok(());
                }
                return Err(error);
            }
        };
        if !completed {
            self.rollback_setup_api_key(&mut api_key_rollback).await?;
            return Ok(());
        }
        if !self
            .tutorial
            .lock()
            .await
            .setup_attempt_is_current(&attempt)
        {
            self.rollback_setup_api_key(&mut api_key_rollback).await?;
            return Ok(());
        }
        if let Err(error) = transaction.commit_config(self.runtime.config().revision) {
            self.rollback_setup_api_key(&mut api_key_rollback).await?;
            return Err(error);
        }
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::MetadataChanged)
            .await;
        self.emit_tutorial_intro_sequence(true).await?;
        Ok(())
    }

    async fn rollback_setup_api_key(
        &self,
        rollback: &mut Option<(ProviderName, Option<String>)>,
    ) -> Result<(), ConfigCommitError> {
        let Some((provider, previous)) = rollback.take() else {
            return Ok(());
        };
        self.factory
            .restore_provider_api_key(provider, previous.as_deref())
            .await?;
        Ok(())
    }

    async fn complete_connected_setup(
        self: &Arc<Self>,
        attempt: &crate::tutorial::SetupAttempt,
        provider_name: String,
        model: String,
    ) -> Result<bool, ConfigCommitError> {
        let recovery = self.runtime.config();
        let config = {
            let tutorial = self.tutorial.lock().await;
            if !tutorial.setup_attempt_is_current(attempt) {
                return Ok(false);
            }
            coosenpai_core::config::patch_config(&self.paths, Some(&recovery), |mut current| {
                current.companion.provider = provider_name.clone();
                current.companion.model = model.clone();
                current.observer.provider = provider_name;
                current.observer.model = model;
                Ok(current)
            })?
        };
        if !self.tutorial.lock().await.setup_attempt_is_current(attempt) {
            return Ok(false);
        }
        let provider = self.setup_provider().await?;
        self.activate_setup_tutorial(config, provider).await?;
        Ok(true)
    }

    async fn activate_setup_tutorial(
        self: &Arc<Self>,
        config: Config,
        provider: TutorialProvider,
    ) -> Result<(), ConfigCommitError> {
        self.stop_watch_internal(true).await;
        self.archive_conversation_for_tutorial().await?;
        let agents = self
            .factory
            .build_tutorial_agents(&config, provider.clone())?;
        {
            let mut tutorial = self.tutorial.lock().await;
            tutorial
                .start(provider)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        }
        self.runtime.replace_config(config.clone(), agents).await?;
        self.activate_runtime();
        let onboarding = {
            let tutorial = self.tutorial.lock().await;
            crate::tutorial_projection::TutorialSnapshotData::read(&tutorial)
        };
        self.publish_event(
            crate::snapshot_presenter::SnapshotEvent::TutorialActivated {
                config: Box::new(config),
                tutorial: onboarding,
            },
        )
        .await;
        self.publish_tutorial_state().await;
        self.ui.input(
            crate::ui_events::UiView::Chat,
            crate::ui_events::UiEvent::Close,
        );
        Ok(())
    }

    async fn setup_provider(&self) -> Result<TutorialProvider, ConfigCommitError> {
        self.tutorial.lock().await.provider().ok_or_else(|| {
            ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::SetupUnavailable,
                    Locale::from_config(&self.runtime.config().ui.language),
                )
                .to_owned(),
            ))
        })
    }

    pub(super) async fn reset_setup(self: &Arc<Self>) -> Result<(), RuntimeError> {
        self.reset_setup_state().await?;
        self.emit_setup_language_choice(false).await
    }

    pub(super) async fn dismiss_setup_and_restart(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<(), RuntimeError> {
        self.reset_setup_state().await?;
        bubbles::dismiss(self.as_ref(), id).await;
        Ok(())
    }

    async fn reset_setup_state(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let transaction = self.config_update.begin().await;
        self.tutorial.lock().await.invalidate_setup_attempt();
        let config = self.runtime.config();
        let provider = self
            .factory
            .tutorial_provider_for_locale(
                super::tutorial_state::tutorial_placeholders(&config),
                coosenpai_core::locale::Locale::from_config(&config.ui.language),
            )
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        self.stop_watch_internal(true).await;
        self.runtime.cancel_operations();
        self.runtime
            .enter_degraded(crate::state::startup::setup_runtime_error_for_locale(
                Locale::from_config(&config.ui.language),
            ))
            .await?;
        self.deactivate_runtime().await;
        {
            let mut tutorial = self.tutorial.lock().await;
            tutorial
                .reset_setup()
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            tutorial.attach_setup_provider(provider);
        }
        self.publish_tutorial_state().await;
        transaction
            .commit()
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        self.ui.input(
            crate::ui_events::UiView::Chat,
            crate::ui_events::UiEvent::Close,
        );
        Ok(())
    }

    async fn emit_setup_choice(
        self: &Arc<Self>,
        key: &str,
        require_ack: bool,
    ) -> Result<(), RuntimeError> {
        let providers = self
            .factory
            .available_setup_providers(self.cancellation.child_token())
            .await;
        if setup_choice_key(key, &providers) == "setup-none" {
            return self
                .emit_setup_status("setup-none", None, false, None, require_ack)
                .await;
        }
        let detail = self
            .tutorial
            .lock()
            .await
            .setup_detail()
            .map(ToOwned::to_owned);
        self.emit_setup_status(key, detail, false, Some(providers), require_ack)
            .await
    }

    async fn emit_setup_language_choice(
        self: &Arc<Self>,
        require_ack: bool,
    ) -> Result<(), RuntimeError> {
        let config = self.runtime.config();
        let locale = Locale::from_config(&config.ui.language);
        let interaction = setup_language_interaction(locale);
        bubbles::mutate_checked(
            &self.ui,
            bubbles::BubbleMutation::DismissMessageKind(SETUP_MESSAGE_KIND.to_owned()),
        )
        .await
        .map_err(RuntimeError::Factory)?;
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        let record = BubbleRecord {
            id: "setup-language".to_owned(),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: text(TextKey::SetupLanguageIntro, locale).to_owned(),
            message_kind: SETUP_MESSAGE_KIND.to_owned(),
            notification_priority: "none".to_owned(),
            caused_by: None,
            display_name: config.companion.display_name,
            persona: config.companion.persona,
            avatar_color: config.ui.avatar_color,
            conversation_generation,
            persistent: true,
            interaction: Some(interaction),
        };
        self.present_setup_record(
            record,
            config.notification.bubble_duration_ms,
            require_ack,
            false,
        )
        .await
    }

    pub(crate) async fn emit_setup_attempt_status(
        self: &Arc<Self>,
        attempt: &crate::tutorial::SetupAttempt,
        key: &str,
        detail: Option<String>,
        connecting: bool,
    ) -> Result<bool, RuntimeError> {
        let _transaction = self.config_update.begin().await;
        if !self.tutorial.lock().await.setup_attempt_is_current(attempt) {
            return Ok(false);
        }
        self.emit_setup_status(key, detail, connecting, None, false)
            .await?;
        Ok(true)
    }

    pub(crate) async fn emit_setup_attempt_failure_serialized(
        self: &Arc<Self>,
        attempt: &crate::tutorial::SetupAttempt,
        provider: &str,
        detail: String,
    ) -> Result<bool, RuntimeError> {
        let _transaction = self.config_update.begin().await;
        self.emit_setup_attempt_failure(attempt, provider, detail)
            .await
    }

    async fn emit_setup_attempt_failure(
        self: &Arc<Self>,
        attempt: &crate::tutorial::SetupAttempt,
        provider: &str,
        detail: String,
    ) -> Result<bool, RuntimeError> {
        let changed =
            self.tutorial
                .lock()
                .await
                .setup_connection_failed(attempt, provider, detail.clone());
        if changed {
            self.emit_setup_choice("setup-fail", false).await?;
        }
        Ok(changed)
    }

    async fn emit_setup_status(
        self: &Arc<Self>,
        key: &str,
        detail: Option<String>,
        connecting: bool,
        providers: Option<Vec<String>>,
        require_ack: bool,
    ) -> Result<(), RuntimeError> {
        let config = self.runtime.config();
        let locale = Locale::from_config(&config.ui.language);
        let provider = self.tutorial.lock().await.provider().ok_or_else(|| {
            RuntimeError::Factory(text(TextKey::SetupUnavailable, locale).to_owned())
        })?;
        let message = provider
            .render(key)
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        let (selected, setup_phase) = {
            let tutorial = self.tutorial.lock().await;
            (
                tutorial.setup_selected().to_owned(),
                tutorial.setup_phase().clone(),
            )
        };
        let interaction = if key == "setup-none" {
            Some(setup_none_interaction(locale))
        } else {
            providers.map(|providers| {
                setup_interaction(&setup_phase, &selected, detail, providers, locale)
            })
        };
        bubbles::mutate_checked(
            &self.ui,
            bubbles::BubbleMutation::DismissMessageKind(SETUP_MESSAGE_KIND.to_owned()),
        )
        .await
        .map_err(RuntimeError::Factory)?;
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        let record = BubbleRecord {
            id: format!("setup-{key}"),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message,
            message_kind: SETUP_MESSAGE_KIND.to_owned(),
            notification_priority: "none".to_owned(),
            caused_by: None,
            display_name: config.companion.display_name,
            persona: config.companion.persona,
            avatar_color: config.ui.avatar_color,
            conversation_generation,
            persistent: setup_record_is_persistent(key, connecting, interaction.is_some()),
            interaction,
        };
        self.present_setup_record(
            record,
            config.notification.bubble_duration_ms,
            require_ack,
            false,
        )
        .await
    }

    async fn present_setup_record(
        self: &Arc<Self>,
        record: BubbleRecord,
        duration_ms: u64,
        require_ack: bool,
        redisplay: bool,
    ) -> Result<(), RuntimeError> {
        let locale = Locale::from_config(&self.runtime.config().ui.language);
        let action = if redisplay { "再表示" } else { "表示" };
        let _ = self.logger.write(
            "INFO",
            &format!(
                "初回セットアップ吹き出しの{action}を要求しました: id={} ack-required={require_ack}",
                record.id
            ),
        );
        if require_ack {
            let id = record.id.clone();
            let result = bubbles::show(self.clone(), record, duration_ms)
                .await
                .map_err(|error| {
                    RuntimeError::Factory(
                        text(TextKey::SetupBubbleDisplayFailed, locale)
                            .replace("{error}", &error.to_string()),
                    )
                });
            match &result {
                Ok(bubbles::BubblePresentationOutcome::Acknowledged) => {
                    let _ = self.logger.write(
                        "INFO",
                        &format!("初回セットアップ吹き出しの表示を完了しました: id={id}"),
                    );
                }
                Ok(bubbles::BubblePresentationOutcome::Dismissed) => {
                    let _ = self.logger.write(
                        "INFO",
                        &format!("初回セットアップ吹き出しが手動で閉じられました: id={id}"),
                    );
                }
                Err(_) => {}
            }
            result.map(|_| ())
        } else {
            bubbles::show_best_effort(self.clone(), record, duration_ms).await;
            Ok(())
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn test_emit_setup_stage(
        self: &Arc<Self>,
        key: &str,
        connecting: bool,
    ) -> Result<(), RuntimeError> {
        let providers = (key != "setup-none").then(|| vec!["codex".to_owned()]);
        self.emit_setup_status(key, None, connecting, providers, false)
            .await
    }
}

fn setup_choice_key<'a>(requested: &'a str, providers: &[String]) -> &'a str {
    if providers.is_empty() {
        "setup-none"
    } else {
        requested
    }
}

fn setup_record_is_persistent(key: &str, connecting: bool, interactive: bool) -> bool {
    connecting || interactive || matches!(key, "setup-none" | "setup-ok")
}

fn setup_interaction(
    phase: &SetupPhase,
    selected: &str,
    detail: Option<String>,
    providers: Vec<String>,
    locale: Locale,
) -> BubbleInteraction {
    let (detail, technical_detail) = setup_interaction_detail(detail);
    match phase {
        SetupPhase::SelectingLanguage => setup_language_interaction(locale),
        SetupPhase::Selecting { .. } => {
            let selected = if providers.iter().any(|provider| provider == selected) {
                selected.to_owned()
            } else {
                providers[0].clone()
            };
            BubbleInteraction {
                select: Some(BubbleSelect {
                    options: providers
                        .into_iter()
                        .map(|value| BubbleOption {
                            label: setup_provider_label(&value),
                            value,
                        })
                        .collect(),
                    selected,
                    action: "setup-provider-select".to_owned(),
                    confirm_label: text(TextKey::SetupNext, locale).to_owned(),
                }),
                secret_input: None,
                actions: Vec::new(),
                detail,
                technical_detail,
            }
        }
        SetupPhase::SelectingMethod {
            provider, method, ..
        } => BubbleInteraction {
            select: Some(BubbleSelect {
                options: vec![
                    BubbleOption {
                        value: SetupConnectionMethod::Login.as_str().to_owned(),
                        label: setup_login_label(provider, locale),
                    },
                    BubbleOption {
                        value: SetupConnectionMethod::ApiKey.as_str().to_owned(),
                        label: text(TextKey::SetupApiKeyLabel, locale).to_owned(),
                    },
                ],
                selected: method.as_str().to_owned(),
                action: "setup-method-select".to_owned(),
                confirm_label: text(TextKey::SetupConnectionConfirm, locale).to_owned(),
            }),
            secret_input: (*method == SetupConnectionMethod::ApiKey).then(|| BubbleSecretInput {
                label: text(TextKey::SetupApiKeyLabel, locale).to_owned(),
                placeholder: text(TextKey::SetupApiKeyPlaceholder, locale).to_owned(),
                action: "setup-api-key-submit".to_owned(),
                submit_label: text(TextKey::SetupApiKeySubmit, locale).to_owned(),
            }),
            actions: Vec::new(),
            detail,
            technical_detail,
        },
        SetupPhase::Inactive | SetupPhase::Connecting { .. } => BubbleInteraction {
            select: None,
            secret_input: None,
            actions: Vec::new(),
            detail,
            technical_detail,
        },
    }
}

fn setup_language_interaction(locale: Locale) -> BubbleInteraction {
    BubbleInteraction {
        select: Some(BubbleSelect {
            options: vec![
                BubbleOption {
                    label: text(TextKey::SetupLanguageJa, locale).to_owned(),
                    value: "ja".to_owned(),
                },
                BubbleOption {
                    label: text(TextKey::SetupLanguageEn, locale).to_owned(),
                    value: "en".to_owned(),
                },
            ],
            selected: locale.as_str().to_owned(),
            action: "setup-language-select".to_owned(),
            confirm_label: text(TextKey::SetupLanguageNext, locale).to_owned(),
        }),
        secret_input: None,
        actions: Vec::new(),
        detail: None,
        technical_detail: None,
    }
}

fn setup_interaction_detail(detail: Option<String>) -> (Option<String>, Option<String>) {
    detail.map_or((None, None), |raw| {
        (Some(redact_setup_error_detail(&raw, None)), None)
    })
}

fn setup_login_label(provider: &str, locale: Locale) -> String {
    match provider {
        "codex" => text(TextKey::SetupLoginCodex, locale).to_owned(),
        "claude" => text(TextKey::SetupLoginClaude, locale).to_owned(),
        _ => text(TextKey::SetupLoginCli, locale).to_owned(),
    }
}

fn setup_provider_name(value: &str, locale: Locale) -> Result<ProviderName, ConfigCommitError> {
    match value {
        "codex" => Ok(ProviderName::Codex),
        "claude" => Ok(ProviderName::Claude),
        _ => Err(ConfigCommitError::Runtime(RuntimeError::Factory(
            text(TextKey::SetupProviderInvalid, locale).to_owned(),
        ))),
    }
}

fn setup_provider_label(value: &str) -> String {
    match value {
        "codex" => "OpenAI".to_owned(),
        "claude" => "Anthropic".to_owned(),
        "opencode" => "OpenCode".to_owned(),
        _ => value.to_owned(),
    }
}

fn setup_none_interaction(locale: Locale) -> BubbleInteraction {
    BubbleInteraction {
        select: None,
        secret_input: None,
        actions: vec![
            crate::bubbles::BubbleAction {
                id: "setup-settings".to_owned(),
                label: text(TextKey::SetupSettings, locale).to_owned(),
            },
            crate::bubbles::BubbleAction {
                id: "setup-retry".to_owned(),
                label: text(TextKey::SetupRetry, locale).to_owned(),
            },
        ],
        detail: Some(text(TextKey::SetupNoneDetail, locale).to_owned()),
        technical_detail: None,
    }
}

fn redact_setup_error_detail(detail: &str, secret: Option<&str>) -> String {
    let detail = secret.filter(|secret| !secret.is_empty()).map_or_else(
        || detail.to_owned(),
        |secret| detail.replace(secret, "<redacted>"),
    );
    let redacted = detail
        .split_whitespace()
        .map(|token| {
            let path = token.trim_matches(|character: char| {
                matches!(character, '\'' | '"' | ',' | ':' | '(' | ')')
            });
            if path.starts_with('/') || path.starts_with("file://") {
                token.replace(path, "<path>")
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = redacted.chars();
    let preview = chars.by_ref().take(300).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

