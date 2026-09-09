use async_trait::async_trait;
use coosenpai_core::companion::{CompanionAgent, DeliveryOwnership};
use coosenpai_core::companion_assertiveness::TemporaryAssertiveness;
use coosenpai_core::config::{Config, ConfigPaths, ConfigValidationIssue};
use coosenpai_core::debug::DebugStore;
use coosenpai_core::locale::{
    localize_config_issue_message, localize_factory_message, text, Locale, TextKey,
};
use coosenpai_core::logging::FileLogger;
use coosenpai_core::mailbox::Mailbox;
use coosenpai_core::memory::{MemoryContext, MemoryService, MemoryStore};
use coosenpai_core::observer::ObserverAgent;
use coosenpai_core::onboarding::{TutorialPlaceholders, TutorialProvider, TutorialScript};
use coosenpai_core::persona::{load_persona, PersonaProfile};
use coosenpai_core::ports::{HelperResolverPort, ProviderApiKeyStore};
use coosenpai_core::provider::{
    resolve_executable, validate_node_version, BridgeLaunch, MockProvider, ProviderBridge,
    ProviderCall, ProviderClient, ProviderName, SessionRequest,
};
use coosenpai_core::provider_api_keys::{
    bridge_environment, bridge_environment_with_api_key, ProviderApiKeyStatus,
};
use coosenpai_core::runtime::{RuntimeAgents, RuntimeFactory};
use coosenpai_core::work::{harness_environment, Harness, HarnessLaunch};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelOptions {
    pub provider: ProviderName,
    pub default_model: String,
    pub candidates: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("{}: {}", issue.path, issue.message)]
pub struct DesktopFactoryError {
    pub issue: ConfigValidationIssue,
}

impl DesktopFactoryError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            issue: ConfigValidationIssue {
                path: path.into(),
                message: message.into(),
            },
        }
    }

    pub(crate) fn format_for_locale(&self, locale: Locale) -> String {
        let mut issue = self.issue.clone();
        let factory_message = localize_factory_message(&issue.message, locale);
        issue.message = if factory_message != issue.message {
            factory_message
        } else {
            localize_config_issue_message(&issue.message, locale)
        };
        format!("{}: {}", issue.path, issue.message)
    }
}

#[derive(Clone)]
pub struct DesktopRuntimeFactory {
    paths: ConfigPaths,
    pub(crate) work: Arc<crate::work::WorkController>,
    incoming: Mailbox,
    outgoing: Vec<Mailbox>,
    logger: Arc<FileLogger>,
    cancellation: CancellationToken,
    bridge: Arc<OnceLock<ProviderBridge>>,
    executable_dir: PathBuf,
    resource_root: Option<PathBuf>,
    temporary_assertiveness: TemporaryAssertiveness,
    keychain: Arc<dyn ProviderApiKeyStore>,
}

/// 承認審査用のClaude bridgeと、作業を実行するハーネスの起動情報。
/// 審査用のClaudeが使えなくても作業自体は起動し、審査失敗として手動確認へ戻す。
pub(crate) struct WorkSession {
    pub reviewer: Option<(ProviderBridge, Arc<dyn ProviderClient>)>,
    pub launch: HarnessLaunch,
}

impl DesktopRuntimeFactory {
    /// ハーネスの起動情報（必須）と承認審査用の Claude bridge（任意）を解決する。
    pub(crate) async fn work_session(
        &self,
        harness: Harness,
        cancellation: CancellationToken,
    ) -> Result<WorkSession, DesktopFactoryError> {
        let path_value =
            coosenpai_core::provider::resolve_login_shell_path(cancellation.clone()).await;
        let executable = resolve_executable(harness.executable_name(), &path_value)
            .map_err(|error| DesktopFactoryError::new("work", error.to_string()))?;
        let provider = match harness {
            Harness::Claude => ProviderName::Claude,
            Harness::Codex => ProviderName::Codex,
        };
        // キーチェーンの key はハーネス自身のログインを上書く場合だけ使う。読めなくても
        // ハーネス自身の認証で動くため、読み取り失敗は起動失敗にしない。
        let api_key = self.keychain.read(provider).ok().flatten();
        let launch = HarnessLaunch {
            harness,
            executable,
            env: harness_environment(std::env::vars(), &path_value, harness, api_key.as_deref()),
        };
        let reviewer = self.reviewer(&path_value);
        Ok(WorkSession { reviewer, launch })
    }

    /// 審査は Claude で行う。claude CLI や bridge が使えない環境（Codex のみの利用者など）では
    /// None を返し、呼び出し側は審査失敗＝手動確認へのフォールバックとして扱う。
    fn reviewer(&self, path_value: &str) -> Option<(ProviderBridge, Arc<dyn ProviderClient>)> {
        let resolver = crate::platform::MacHelperResolver;
        let script = resolver.resolve_provider_bridge(
            &self.executable_dir,
            &self.paths.root,
            self.resource_root.as_deref(),
        )?;
        let node = resolver.resolve_node(path_value)?;
        let claude = resolve_executable("claude", path_value).ok()?;
        let claude_key = self.keychain.read(ProviderName::Claude).ok().flatten();
        let env = harness_environment(
            std::env::vars(),
            path_value,
            Harness::Claude,
            claude_key.as_deref(),
        );
        let bridge =
            ProviderBridge::new_with_explicit_environment(BridgeLaunch { node, script, env });
        let reviewer = Arc::new(bridge.provider(ProviderName::Claude, Some(claude)));
        Some((bridge, reviewer))
    }

    #[cfg(test)]
    pub fn new(
        paths: ConfigPaths,
        logger: Arc<FileLogger>,
        cancellation: CancellationToken,
    ) -> Result<Self, String> {
        Self::new_with_keychain(
            paths,
            logger,
            cancellation,
            crate::platform::provider_api_key_store(),
        )
    }

    pub(crate) fn new_with_keychain(
        paths: ConfigPaths,
        logger: Arc<FileLogger>,
        cancellation: CancellationToken,
        keychain: Arc<dyn ProviderApiKeyStore>,
    ) -> Result<Self, String> {
        let incoming =
            Mailbox::open(paths.mailbox.clone(), "companion").map_err(|error| error.to_string())?;
        let outgoing = ["app", "notify"]
            .into_iter()
            .map(|recipient| Mailbox::open(paths.mailbox.clone(), recipient))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let executable_dir = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .parent()
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                text(TextKey::FactoryExecutableDirectoryUnavailable, Locale::Ja).to_owned()
            })?;
        let resource_root = paths
            .builtin_personas
            .as_deref()
            .and_then(resource_root_for_builtin_personas);
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "ホームディレクトリを取得できません".to_owned())?;
        Ok(Self {
            work: Arc::new(crate::work::WorkController::new(
                paths.state.join("work/latest.json"),
                home,
                coosenpai_core::work::ApprovalMode::Manual,
            )),
            paths,
            incoming,
            outgoing,
            logger,
            cancellation,
            bridge: Arc::new(OnceLock::new()),
            executable_dir,
            resource_root,
            temporary_assertiveness: TemporaryAssertiveness::default(),
            keychain,
        })
    }

    pub fn temporary_assertiveness(&self) -> TemporaryAssertiveness {
        self.temporary_assertiveness.clone()
    }

    pub fn provider_api_key_status(&self) -> Result<ProviderApiKeyStatus, DesktopFactoryError> {
        ProviderApiKeyStatus::read(self.keychain.as_ref()).map_err(|_| {
            DesktopFactoryError::new(
                "provider.apiKey",
                text(TextKey::FactoryApiKeyCheckFailed, Locale::Ja),
            )
        })
    }

    pub(crate) fn provider_api_key_value(
        &self,
        provider: ProviderName,
    ) -> Result<Option<String>, DesktopFactoryError> {
        self.keychain.read(provider).map_err(|_| {
            DesktopFactoryError::new(
                "provider.apiKey",
                text(TextKey::FactoryApiKeyReadFailed, Locale::Ja),
            )
        })
    }

    pub async fn update_provider_api_key(
        &self,
        provider: ProviderName,
        api_key: Option<&str>,
    ) -> Result<ProviderApiKeyStatus, DesktopFactoryError> {
        let previous = self.provider_api_key_value(provider)?;
        self.write_provider_api_key_value(provider, api_key)?;
        if let Err(error) = self.refresh_bridge_environment().await {
            return match self
                .restore_provider_api_key(provider, previous.as_deref())
                .await
            {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(rollback_error),
            };
        }
        match self.provider_api_key_status() {
            Ok(status) => Ok(status),
            Err(error) => {
                match self
                    .restore_provider_api_key(provider, previous.as_deref())
                    .await
                {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(rollback_error),
                }
            }
        }
    }

    pub(crate) async fn restore_provider_api_key(
        &self,
        provider: ProviderName,
        previous: Option<&str>,
    ) -> Result<(), DesktopFactoryError> {
        self.write_provider_api_key_value(provider, previous)?;
        self.refresh_bridge_environment().await
    }

    fn write_provider_api_key_value(
        &self,
        provider: ProviderName,
        api_key: Option<&str>,
    ) -> Result<(), DesktopFactoryError> {
        match api_key {
            Some(api_key) => self.keychain.write(provider, api_key).map_err(|_| {
                DesktopFactoryError::new(
                    "provider.apiKey",
                    text(TextKey::FactoryApiKeySaveFailed, Locale::Ja),
                )
            }),
            None => self.keychain.delete(provider).map_err(|_| {
                DesktopFactoryError::new(
                    "provider.apiKey",
                    text(TextKey::FactoryApiKeyDeleteFailed, Locale::Ja),
                )
            }),
        }
    }

    fn bridge_environment(
        &self,
        path_value: &str,
    ) -> Result<Vec<(String, String)>, DesktopFactoryError> {
        bridge_environment(
            std::env::vars(),
            path_value,
            &self.paths.root,
            self.keychain.as_ref(),
        )
        .map_err(|_| {
            DesktopFactoryError::new(
                "provider.apiKey",
                text(TextKey::FactoryApiKeyReadFailed, Locale::Ja),
            )
        })
    }

    fn bridge_environment_with_api_key(
        &self,
        path_value: &str,
        provider: ProviderName,
        api_key: &str,
    ) -> Result<Vec<(String, String)>, DesktopFactoryError> {
        bridge_environment_with_api_key(
            std::env::vars(),
            path_value,
            &self.paths.root,
            self.keychain.as_ref(),
            Some((provider, api_key)),
        )
        .map_err(|_| {
            DesktopFactoryError::new(
                "provider.apiKey",
                text(TextKey::FactoryApiKeyReadFailed, Locale::Ja),
            )
        })
    }

    async fn refresh_bridge_environment(&self) -> Result<(), DesktopFactoryError> {
        let Some(bridge) = self.bridge.get() else {
            return Ok(());
        };
        let path_value =
            coosenpai_core::provider::resolve_login_shell_path(self.cancellation.child_token())
                .await;
        let environment = self.bridge_environment(&path_value)?;
        bridge.update_environment(environment).await.map_err(|_| {
            DesktopFactoryError::new(
                "provider.apiKey",
                text(TextKey::FactoryAuthApplyFailed, Locale::Ja),
            )
        })
    }

    async fn refresh_bridge_environment_for_path(
        &self,
        path_value: &str,
    ) -> Result<(), DesktopFactoryError> {
        let Some(bridge) = self.bridge.get() else {
            return Ok(());
        };
        let environment = self.bridge_environment(path_value)?;
        bridge.update_environment(environment).await.map_err(|_| {
            DesktopFactoryError::new(
                "provider.apiKey",
                text(TextKey::FactoryAuthApplyFailed, Locale::Ja),
            )
        })
    }

    fn provider(
        &self,
        section: &str,
        name: &str,
        override_path: Option<&str>,
        path_value: &str,
        bridge_env: Option<Vec<(String, String)>>,
    ) -> Result<Arc<dyn coosenpai_core::provider::ProviderClient>, DesktopFactoryError> {
        let provider = parse_provider_name(section, name)?;
        if provider == ProviderName::Mock {
            return Ok(Arc::new(MockProvider::new()));
        }
        let executable = resolve_executable(override_path.unwrap_or(provider.as_str()), path_value)
            .map_err(|error| {
                DesktopFactoryError::new(
                    format!(
                        "{section}.{}",
                        if override_path.is_some() {
                            "executable"
                        } else {
                            "provider"
                        }
                    ),
                    error.to_string(),
                )
            })?;
        let bridge = match self.bridge.get() {
            Some(bridge) => bridge,
            None => {
                let resolver = crate::platform::MacHelperResolver;
                let script = resolver
                    .resolve_provider_bridge(
                        &self.executable_dir,
                        &self.paths.root,
                        self.resource_root.as_deref(),
                    )
                    .ok_or_else(|| {
                        DesktopFactoryError::new(
                            format!("{section}.provider"),
                            text(TextKey::FactoryBridgeMissing, Locale::Ja),
                        )
                    })?;
                let node = resolver.resolve_node(path_value).ok_or_else(|| {
                    DesktopFactoryError::new(
                        format!("{section}.provider"),
                        text(TextKey::FactoryNodeMissing, Locale::Ja),
                    )
                })?;
                let env = match bridge_env {
                    Some(environment) => environment,
                    None => self.bridge_environment(path_value)?,
                };
                let candidate = ProviderBridge::new(BridgeLaunch { node, script, env });
                let _ = self.bridge.set(candidate);
                self.bridge.get().ok_or_else(|| {
                    DesktopFactoryError::new(
                        format!("{section}.provider"),
                        text(TextKey::FactoryBridgeInitFailed, Locale::Ja),
                    )
                })?
            }
        };
        Ok(Arc::new(bridge.provider(provider, Some(executable))))
    }

    async fn validate_node(
        &self,
        path_value: &str,
        section: &str,
        cancellation: CancellationToken,
    ) -> Result<(), DesktopFactoryError> {
        let node = crate::platform::MacHelperResolver
            .resolve_node(path_value)
            .ok_or_else(|| {
                DesktopFactoryError::new(
                    format!("{section}.provider"),
                    text(TextKey::FactoryNodeMissing, Locale::Ja),
                )
            })?;
        validate_node_version(&node, path_value, cancellation)
            .await
            .map_err(|error| {
                DesktopFactoryError::new(format!("{section}.provider"), error.to_string())
            })
    }

    pub async fn check_connection(
        &self,
        provider_name: &str,
        model: &str,
        executable: Option<&str>,
        api_key: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<(), DesktopFactoryError> {
        let path_value =
            coosenpai_core::provider::resolve_login_shell_path(cancellation.clone()).await;
        if provider_name != ProviderName::Mock.as_str() {
            self.validate_node(&path_value, "companion", cancellation.clone())
                .await?;
        }
        if let Some(api_key) = api_key {
            return self
                .check_connection_with_api_key(
                    provider_name,
                    model,
                    executable,
                    &path_value,
                    api_key,
                    cancellation,
                )
                .await;
        }
        self.refresh_bridge_environment_for_path(&path_value)
            .await?;
        let provider = self.provider("companion", provider_name, executable, &path_value, None)?;
        run_connection_check(provider, model, cancellation)
            .await
            .map_err(|message| DesktopFactoryError::new("companion.provider", message))
    }

    async fn check_connection_with_api_key(
        &self,
        provider_name: &str,
        model: &str,
        executable: Option<&str>,
        path_value: &str,
        api_key: &str,
        cancellation: CancellationToken,
    ) -> Result<(), DesktopFactoryError> {
        let provider = parse_provider_name("companion", provider_name)?;
        let original_environment = self.bridge_environment(path_value)?;
        let temporary_environment =
            self.bridge_environment_with_api_key(path_value, provider, api_key)?;
        if let Some(bridge) = self.bridge.get() {
            bridge
                .update_environment(temporary_environment.clone())
                .await
                .map_err(|_| {
                    DesktopFactoryError::new(
                        "provider.apiKey",
                        text(TextKey::FactoryAuthApplyFailed, Locale::Ja),
                    )
                })?;
        }

        let checked = match self.provider(
            "companion",
            provider_name,
            executable,
            path_value,
            Some(temporary_environment),
        ) {
            Ok(provider) => run_connection_check(provider, model, cancellation)
                .await
                .map_err(|message| DesktopFactoryError::new("companion.provider", message)),
            Err(error) => Err(error),
        };
        self.restore_bridge_environment(original_environment)
            .await?;
        checked
    }

    async fn restore_bridge_environment(
        &self,
        environment: Vec<(String, String)>,
    ) -> Result<(), DesktopFactoryError> {
        let Some(bridge) = self.bridge.get() else {
            return Ok(());
        };
        bridge.update_environment(environment).await.map_err(|_| {
            DesktopFactoryError::new(
                "provider.apiKey",
                text(TextKey::FactoryAuthRestoreFailed, Locale::Ja),
            )
        })
    }

    pub async fn build_tutorial_candidate(
        &self,
        config: &Config,
        placeholders: TutorialPlaceholders,
    ) -> Result<(RuntimeAgents, TutorialProvider), DesktopFactoryError> {
        let provider = self
            .tutorial_provider_for_locale(placeholders, Locale::from_config(&config.ui.language))?;
        let agents = self.build_tutorial_agents(config, provider.clone())?;
        Ok((agents, provider))
    }

    pub fn tutorial_provider_for_locale(
        &self,
        placeholders: TutorialPlaceholders,
        locale: Locale,
    ) -> Result<TutorialProvider, DesktopFactoryError> {
        let path = self.paths.builtin_tutorial.as_ref().ok_or_else(|| {
            DesktopFactoryError::new("tutorial", text(TextKey::FactoryTutorialMissing, locale))
        })?;
        let path = match locale {
            Locale::Ja => path,
            Locale::En => self.paths.builtin_tutorial_en.as_ref().ok_or_else(|| {
                DesktopFactoryError::new(
                    "tutorial",
                    text(TextKey::FactoryEnglishTutorialMissing, locale),
                )
            })?,
        };
        let script = TutorialScript::load(path)
            .map_err(|error| DesktopFactoryError::new("tutorial", error.to_string()))?;
        Ok(TutorialProvider::new_with_locale(
            script,
            placeholders,
            locale,
        ))
    }

    pub fn build_tutorial_agents(
        &self,
        config: &Config,
        provider: TutorialProvider,
    ) -> Result<RuntimeAgents, DesktopFactoryError> {
        let provider_client: Arc<dyn ProviderClient> = Arc::new(provider.clone());
        // チュートリアルでは撮影・OCR・observer 呼び出しまでを通すが、
        // 観察を本番の履歴や mailbox へ残して終了後に再処理させない。
        let observer = ObserverAgent::new(provider_client.clone(), config.observer.clone())
            .with_logger(self.logger.clone());
        let companion = CompanionAgent::with_persona_profile(
            provider_client,
            config.companion.clone(),
            self.persona(&config.companion.persona)?,
            None,
            DeliveryOwnership::Owner,
        )
        .with_locale(Locale::from_config(&config.ui.language))
        .with_temporary_assertiveness(self.temporary_assertiveness.clone())
        .with_storage(&self.paths, config.retention.conversation_days)
        .with_logger(self.logger.clone());
        Ok(RuntimeAgents {
            observation_delivery: coosenpai_core::runtime::ObservationDelivery::CallerOnly,
            observer: Some(observer),
            companion: Some(companion),
            memory: None,
        })
    }

    fn persona(&self, name: &str) -> Result<PersonaProfile, DesktopFactoryError> {
        load_persona(&self.paths, name)
            .map_err(|error| DesktopFactoryError::new("companion.persona", error.to_string()))
    }

    fn attachment_ocr(&self, config: &Config) -> Option<Arc<dyn coosenpai_core::ports::OcrPort>> {
        let resolver = crate::platform::MacHelperResolver;
        let helper = resolver.resolve_ocr_helper(
            &self.executable_dir,
            &self.paths.root,
            config.watch.ocr_gate.executable.as_deref(),
        )?;
        Some(Arc::new(crate::platform::MacOcr::new(Some(helper))))
    }

    pub async fn build_candidate(
        &self,
        config: &Config,
    ) -> Result<RuntimeAgents, DesktopFactoryError> {
        self.build_candidate_with_notice(config, None).await
    }

    pub async fn build_candidate_with_notice(
        &self,
        config: &Config,
        notice: Option<String>,
    ) -> Result<RuntimeAgents, DesktopFactoryError> {
        if self.cancellation.is_cancelled() {
            return Err(DesktopFactoryError::new(
                "config",
                text(TextKey::FactoryShutdown, Locale::Ja),
            ));
        }
        let path_value =
            coosenpai_core::provider::resolve_login_shell_path(self.cancellation.child_token())
                .await;
        if config.companion.provider != ProviderName::Mock.as_str()
            || config.observer.provider != ProviderName::Mock.as_str()
        {
            self.validate_node(&path_value, "companion", self.cancellation.child_token())
                .await?;
        }
        self.refresh_bridge_environment_for_path(&path_value)
            .await?;
        let observer_provider = self.provider(
            "observer",
            &config.observer.provider,
            config.observer.executable.as_deref(),
            &path_value,
            None,
        )?;
        let companion_provider = self.provider(
            "companion",
            &config.companion.provider,
            config.companion.executable.as_deref(),
            &path_value,
            None,
        )?;
        let companion_provider_name = parse_provider_name("companion", &config.companion.provider)?;
        let observer = ObserverAgent::new(observer_provider, config.observer.clone())
            .with_usage_path(self.paths.usage.clone())
            .with_observation_store_without_read(&self.paths, config.retention.observation_days)
            .with_mailbox(self.incoming.clone())
            .with_logger(self.logger.clone());
        let observer = if config.debug.enabled {
            observer.with_debug_store(DebugStore::from_paths(&self.paths))
        } else {
            observer
        };
        let companion = CompanionAgent::with_persona_profile(
            companion_provider.clone(),
            config.companion.clone(),
            self.persona(&config.companion.persona)?,
            None,
            DeliveryOwnership::Owner,
        )
        .with_locale(Locale::from_config(&config.ui.language))
        .with_temporary_assertiveness(self.temporary_assertiveness.clone())
        .with_storage(&self.paths, config.retention.conversation_days)
        .with_incoming_mailbox(self.incoming.clone())
        .with_outgoing_mailboxes(self.outgoing.clone())
        .with_logger(self.logger.clone())
        .with_memory_context(MemoryContext::new(
            self.paths.clone(),
            config.memory.clone(),
        ));
        // 作業のハーネスは会話用providerで決める。対応providerでなければ作業能力を持たせない。
        let companion = match Harness::from_provider(companion_provider_name) {
            Ok(harness) => companion.with_work_executor(Arc::new(crate::work::DesktopChatWork {
                controller: self.work.clone(),
                factory: Arc::new(self.clone()),
                harness,
            })),
            Err(_) => companion,
        };
        let companion = match self.attachment_ocr(config) {
            Some(ocr) => companion.with_attachment_ocr(ocr),
            None => companion,
        };
        let companion = match notice {
            Some(notice) => companion.with_context_notice(notice),
            None => companion,
        };
        let companion = if config.debug.enabled {
            companion.with_debug_store(DebugStore::from_paths(&self.paths))
        } else {
            companion
        };
        let memory = MemoryService::new(
            companion_provider,
            MemoryStore::new(self.paths.clone()),
            config.memory.clone(),
            config.companion.clone(),
        )
        .with_logger(self.logger.clone());
        let memory = if config.debug.enabled {
            memory.with_debug_store(DebugStore::from_paths(&self.paths))
        } else {
            memory
        };
        Ok(RuntimeAgents {
            observation_delivery: coosenpai_core::runtime::ObservationDelivery::Companion,
            observer: Some(observer),
            companion: Some(companion),
            memory: Some(memory),
        })
    }

    pub async fn provider_capabilities(
        &self,
        name: &str,
        cancellation: CancellationToken,
    ) -> Result<coosenpai_core::provider::ProviderCapabilities, DesktopFactoryError> {
        let path_value =
            coosenpai_core::provider::resolve_login_shell_path(cancellation.clone()).await;
        if name != ProviderName::Mock.as_str() {
            self.validate_node(&path_value, "provider", cancellation.clone())
                .await?;
        }
        self.refresh_bridge_environment_for_path(&path_value)
            .await?;
        let provider = self.provider("provider", name, None, &path_value, None)?;
        provider
            .resolve_capabilities(cancellation, std::time::Duration::from_secs(10))
            .await
            .map_err(|error| DesktopFactoryError::new("provider", error.message))?
            .ok_or_else(|| {
                DesktopFactoryError::new(
                    "provider",
                    text(TextKey::FactoryModelInfoMissing, Locale::Ja),
                )
            })
    }

    pub async fn available_setup_providers(&self, cancellation: CancellationToken) -> Vec<String> {
        let path_value = coosenpai_core::provider::resolve_login_shell_path(cancellation).await;
        available_provider_names(&path_value)
    }

    pub async fn provider_model_options(
        &self,
        config: &Config,
        tutorial_active: bool,
    ) -> Result<Vec<ProviderModelOptions>, DesktopFactoryError> {
        if tutorial_active {
            return Ok(tutorial_provider_model_options(config));
        }
        let mut values = Vec::new();
        for provider in [
            ProviderName::Codex,
            ProviderName::Claude,
            ProviderName::Opencode,
        ] {
            let capabilities = self
                .provider_capabilities(provider.as_str(), self.cancellation.child_token())
                .await?;
            values.push(ProviderModelOptions {
                provider,
                default_model: capabilities.default_model,
                candidates: capabilities.model_candidates,
            });
        }
        Ok(values)
    }

    pub async fn build_companion_candidate(
        &self,
        config: &Config,
    ) -> Result<CompanionAgent, DesktopFactoryError> {
        self.build_companion_candidate_with_notice(config, None)
            .await
    }

    pub async fn build_companion_candidate_with_notice(
        &self,
        config: &Config,
        notice: Option<String>,
    ) -> Result<CompanionAgent, DesktopFactoryError> {
        if self.cancellation.is_cancelled() {
            return Err(DesktopFactoryError::new(
                "config",
                text(TextKey::FactoryShutdown, Locale::Ja),
            ));
        }
        let path_value =
            coosenpai_core::provider::resolve_login_shell_path(self.cancellation.child_token())
                .await;
        if config.companion.provider != ProviderName::Mock.as_str() {
            self.validate_node(&path_value, "companion", self.cancellation.child_token())
                .await?;
        }
        self.refresh_bridge_environment_for_path(&path_value)
            .await?;
        let provider = self.provider(
            "companion",
            &config.companion.provider,
            config.companion.executable.as_deref(),
            &path_value,
            None,
        )?;
        let agent = CompanionAgent::with_persona_profile(
            provider,
            config.companion.clone(),
            self.persona(&config.companion.persona)?,
            None,
            DeliveryOwnership::Owner,
        )
        .with_locale(Locale::from_config(&config.ui.language))
        .with_temporary_assertiveness(self.temporary_assertiveness.clone())
        .with_storage(&self.paths, config.retention.conversation_days)
        .with_incoming_mailbox(self.incoming.clone())
        .with_outgoing_mailboxes(self.outgoing.clone())
        .with_logger(self.logger.clone())
        .with_memory_context(MemoryContext::new(
            self.paths.clone(),
            config.memory.clone(),
        ));
        let agent = match self.attachment_ocr(config) {
            Some(ocr) => agent.with_attachment_ocr(ocr),
            None => agent,
        };
        let agent = if config.debug.enabled {
            agent.with_debug_store(DebugStore::from_paths(&self.paths))
        } else {
            agent
        };
        Ok(match notice {
            Some(notice) => agent.with_context_notice(notice),
            None => agent,
        })
    }
}

pub(crate) fn available_provider_names(path_value: &str) -> Vec<String> {
    [ProviderName::Codex, ProviderName::Claude]
        .into_iter()
        .filter(|provider| resolve_executable(provider.as_str(), path_value).is_ok())
        .map(|provider| provider.as_str().to_owned())
        .collect()
}

fn tutorial_provider_model_options(config: &Config) -> Vec<ProviderModelOptions> {
    [
        (
            ProviderName::Codex,
            "default",
            &["default", "gpt-5.4", "gpt-5.3-codex"][..],
        ),
        (
            ProviderName::Claude,
            "sonnet",
            &["sonnet", "opus", "haiku"][..],
        ),
        (
            ProviderName::Opencode,
            "opencode/big-pickle",
            &["opencode/big-pickle"][..],
        ),
    ]
    .into_iter()
    .map(|(provider, default_model, static_candidates)| {
        let mut candidates = static_candidates
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        for (configured_provider, configured_model) in [
            (&config.observer.provider, &config.observer.model),
            (&config.companion.provider, &config.companion.model),
        ] {
            if configured_provider == provider.as_str() && !candidates.contains(configured_model) {
                candidates.push(configured_model.clone());
            }
        }
        ProviderModelOptions {
            provider,
            default_model: default_model.to_owned(),
            candidates,
        }
    })
    .collect()
}

async fn run_connection_check(
    provider: Arc<dyn ProviderClient>,
    model: &str,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let result = provider
        .call(
            ProviderCall {
                system_prompt: "接続確認です。ユーザーの指示に短く答えてください。".to_owned(),
                prompt: "Return OK".to_owned(),
                images: Vec::new(),
                tools_disabled: true,
                output_schema: None,
                output_validation_schema: None,
                session: SessionRequest::Ephemeral,
                model: Some(model.to_owned()),
                effort: None,
                timeout: Duration::from_secs(30),
                tutorial_response_key: None,
            },
            cancellation,
        )
        .await
        .map_err(|error| error.to_string())?;
    if result.text.trim().is_empty() {
        return Err(text(TextKey::FactoryConnectionEmpty, Locale::Ja).to_owned());
    }
    Ok(())
}

pub fn persona_names(paths: &ConfigPaths) -> Vec<String> {
    let mut names = BTreeSet::new();
    for directory in [
        Some(paths.personas.as_path()),
        paths.builtin_personas.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) == Some("md") {
                    if let Some(name) = path.file_stem().and_then(|value| value.to_str()) {
                        names.insert(name.to_owned());
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaOption {
    pub id: String,
    pub display_name: String,
    pub builtin: bool,
}

pub fn persona_options(paths: &ConfigPaths) -> Result<Vec<PersonaOption>, String> {
    coosenpai_core::persona_store::PersonaStore::from_paths(paths)
        .and_then(|store| store.list())
        .map(|values| {
            values
                .into_iter()
                .map(|value| PersonaOption {
                    id: value.id,
                    display_name: value.display_name,
                    builtin: value.builtin,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[async_trait]
impl RuntimeFactory for DesktopRuntimeFactory {
    async fn build(&self, config: &Config) -> Result<RuntimeAgents, String> {
        self.build_candidate(config)
            .await
            .map_err(|error| error.to_string())
    }

    async fn shutdown(&self) {
        if let Some(bridge) = self.bridge.get() {
            bridge.shutdown().await;
        }
    }
}

fn parse_provider_name(section: &str, name: &str) -> Result<ProviderName, DesktopFactoryError> {
    ProviderName::from_config_name(name).ok_or_else(|| {
        DesktopFactoryError::new(
            format!("{section}.provider"),
            text(TextKey::FactoryProviderInvalid, Locale::Ja).replace("{name}", name),
        )
    })
}

pub fn bundled_persona_directory(resource_dir: PathBuf) -> PathBuf {
    resource_dir.join("prompts/facets/personas")
}

fn resource_root_for_builtin_personas(directory: &std::path::Path) -> Option<PathBuf> {
    let parent = directory.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("facets") {
        return parent.parent()?.parent().map(ToOwned::to_owned);
    }
    Some(parent.to_owned())
}

