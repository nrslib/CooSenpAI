use super::*;
use crate::bubbles::{self, BubbleInteraction, BubbleOption, BubbleRecord, BubbleSelect};
use crate::tutorial::{
    tutorial_step_can_be_skipped, TUTORIAL_AUTO_ADVANCE_MESSAGE, TUTORIAL_SKIP_ACTION,
};
use coosenpai_core::onboarding::TutorialProvider;
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
        let (needs_setup, tutorial_active) = {
            let tutorial = self.tutorial.lock().await;
            (
                tutorial.state().needs_setup(),
                tutorial.state().tutorial_active(),
            )
        };
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
                "この吹き出しの操作は期限切れです".to_owned(),
            )));
        }
        match action {
            TUTORIAL_SKIP_ACTION => {
                let step = self.tutorial_current_step().await.ok_or_else(|| {
                    ConfigCommitError::Runtime(RuntimeError::Factory(
                        "現在のチュートリアル step がありません".to_owned(),
                    ))
                })?;
                if !tutorial_step_can_be_skipped(step) {
                    return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                        TUTORIAL_AUTO_ADVANCE_MESSAGE.to_owned(),
                    )));
                }
                self.command_finish_tutorial_step(permit, step, true)
                    .await?;
                Ok(())
            }
            "setup-connect" => {
                let provider = value.ok_or_else(|| {
                    ConfigCommitError::Runtime(RuntimeError::Factory(
                        "provider を選んでください".to_owned(),
                    ))
                })?;
                self.connect_setup_provider(provider).await
            }
            "setup-settings" => {
                crate::windows::show_main(&self.app);
                let _ = tauri::Emitter::emit(&self.app, "coosenpai:settings:requested", ());
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
                self.tutorial_settings_opened().await?;
                crate::windows::show_main(&self.app);
                let _ = tauri::Emitter::emit(&self.app, "coosenpai:settings:requested", ());
                let _ = tauri::Emitter::emit(&self.app, "coosenpai:settings:focus", "watch");
                Ok(())
            }
            _ => Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                "吹き出しの操作が不正です".to_owned(),
            ))),
        }
    }

    async fn connect_setup_provider(
        self: &Arc<Self>,
        provider_name: &str,
    ) -> Result<(), ConfigCommitError> {
        let attempt = self
            .tutorial
            .lock()
            .await
            .begin_setup_connection(provider_name, &self.cancellation)
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
                let detail = error.to_string();
                self.emit_setup_attempt_failure_serialized(&attempt, provider_name, detail)
                    .await?;
                return Ok(());
            }
            SetupConnectionWait::Cancelled => return Ok(()),
            SetupConnectionWait::TimedOut => {
                self.emit_setup_attempt_failure_serialized(
                    &attempt,
                    provider_name,
                    "接続確認がタイムアウトしました".to_owned(),
                )
                .await?;
                return Ok(());
            }
        };
        let transaction = self.config_update.begin().await;
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
                if retryable {
                    let detail = error.format_for_user();
                    self.emit_setup_attempt_failure(&attempt, provider_name, detail)
                        .await?;
                    return Ok(());
                }
                return Err(error);
            }
        };
        if !completed {
            return Ok(());
        }
        if !self
            .tutorial
            .lock()
            .await
            .setup_attempt_is_current(&attempt)
        {
            return Ok(());
        }
        transaction.commit()?;
        self.emit_tutorial_intro_sequence(true).await?;
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
            crate::snapshot::OnboardingView::from_state_and_resume(
                tutorial.state(),
                tutorial.resume_pending(),
            )
        };
        self.publish(|snapshot| {
            snapshot.apply_config(config);
            snapshot.onboarding = onboarding;
        })
        .await;
        self.publish_tutorial_state().await;
        crate::windows::sync_tutorial(&self.app, true);
        crate::windows::hide_main(&self.app);
        Ok(())
    }

    async fn setup_provider(&self) -> Result<TutorialProvider, ConfigCommitError> {
        self.tutorial.lock().await.provider().ok_or_else(|| {
            ConfigCommitError::Runtime(RuntimeError::Factory(
                "初回セットアップを開始できません".to_owned(),
            ))
        })
    }

    pub(super) async fn reset_setup(self: &Arc<Self>) -> Result<(), RuntimeError> {
        self.reset_setup_state().await?;
        self.emit_setup_choice("setup-intro", false).await
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
            .tutorial_provider(super::tutorial_state::tutorial_placeholders(&config))
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        self.stop_watch_internal(true).await;
        self.runtime.cancel_operations();
        self.runtime
            .enter_degraded(crate::state::startup::setup_runtime_error())
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
        crate::windows::hide_main(&self.app);
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
        let provider =
            self.tutorial.lock().await.provider().ok_or_else(|| {
                RuntimeError::Factory("初回セットアップを開始できません".to_owned())
            })?;
        let message = provider
            .render(key)
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        let config = self.runtime.config();
        let selected = self.tutorial.lock().await.setup_selected().to_owned();
        let interaction = if key == "setup-none" {
            Some(setup_none_interaction())
        } else {
            providers.map(|providers| setup_interaction(&selected, detail, providers))
        };
        if self
            .bubbles
            .lock()
            .await
            .dismiss_message_kind(SETUP_MESSAGE_KIND)
        {
            let _ = bubbles::sync_window(self).await;
        }
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
            open_url: None,
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
                    RuntimeError::Factory(format!("初回セットアップを表示できません: {error}"))
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
    selected: &str,
    detail: Option<String>,
    providers: Vec<String>,
) -> BubbleInteraction {
    let selected = if providers.iter().any(|provider| provider == selected) {
        selected.to_owned()
    } else {
        providers[0].clone()
    };
    let (detail, technical_detail) = detail.map_or((None, None), |raw| {
        (
            Some(setup_failure_message(&selected, &raw)),
            Some(redact_setup_error_detail(&raw)),
        )
    });
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
            action: "setup-connect".to_owned(),
            confirm_label: "このAIで接続を確認".to_owned(),
        }),
        actions: Vec::new(),
        detail,
        technical_detail,
    }
}

fn setup_provider_label(value: &str) -> String {
    match value {
        "codex" => "Codex（ChatGPT）".to_owned(),
        "claude" => "Claude".to_owned(),
        "opencode" => "OpenCode".to_owned(),
        _ => value.to_owned(),
    }
}

fn setup_none_interaction() -> BubbleInteraction {
    BubbleInteraction {
        select: None,
        actions: vec![
            crate::bubbles::BubbleAction {
                id: "setup-settings".to_owned(),
                label: "設定を開く".to_owned(),
            },
            crate::bubbles::BubbleAction {
                id: "setup-retry".to_owned(),
                label: "もう一度調べる".to_owned(),
            },
        ],
        detail: Some(
            "Codex CLI、Claude Code、OpenCode のいずれかをインストールし、その CLI でログインしたあと、もう一度調べてください。".to_owned(),
        ),
        technical_detail: None,
    }
}

fn setup_failure_message(provider: &str, detail: &str) -> String {
    let normalized = detail.to_ascii_lowercase();
    let authentication = ["auth", "login", "credential", "copyfile"]
        .iter()
        .any(|needle| normalized.contains(needle));
    match (provider, authentication) {
        ("codex", true) => {
            "Codex のログイン情報が見つかりません。codex login を実行してください。".to_owned()
        }
        ("claude", true) => {
            "Claude のログイン情報を確認できません。claude を起動してログインしてください。"
                .to_owned()
        }
        ("opencode", true) => {
            "OpenCode のログイン情報を確認できません。OpenCode の設定を確認してください。"
                .to_owned()
        }
        _ => format!(
            "{} に接続できませんでした。CLI の設定を確認してください。",
            setup_provider_label(provider)
        ),
    }
}

fn redact_setup_error_detail(detail: &str) -> String {
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

