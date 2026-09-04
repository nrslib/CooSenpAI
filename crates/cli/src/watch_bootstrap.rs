use crate::shutdown::{self, BootstrapOutcome, ShutdownSignals};
use crate::{load_persona, make_provider};
use anyhow::{Context, Result};
use async_trait::async_trait;
use coosenpai_core::companion::{CompanionAgent, DeliveryOwnership};
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::debug::DebugStore;
use coosenpai_core::logging::FileLogger;
use coosenpai_core::mailbox::Mailbox;
use coosenpai_core::memory::{MemoryContext, MemoryService, MemoryStore};
use coosenpai_core::observer::ObserverAgent;
use coosenpai_core::persistence::WatchLock;
use coosenpai_core::ports::{HelperResolverPort, RuntimeLogger};
use coosenpai_core::runtime::{RuntimeAgents, RuntimeFactory};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) struct WatchBootstrap {
    agents: Option<RuntimeAgents>,
    factory: Arc<dyn RuntimeFactory>,
    signals: ShutdownSignals,
    cancellation: CancellationToken,
    _watch_lock: WatchLock,
}

impl WatchBootstrap {
    pub(super) async fn start(
        paths: &ConfigPaths,
        config: &Config,
        logger: Arc<FileLogger>,
    ) -> Result<Option<Self>> {
        let outcome = shutdown::bootstrap(|cancellation| async move {
            let initial_permission = crate::platform::screen_capture_permission();
            let permission = if initial_permission.requestable {
                crate::platform::request_screen_capture_permission()
            } else {
                initial_permission
            };
            let presentation = permission.presentation();
            logger.write("INFO", &format!("画面収録権限: {}", presentation.status))?;
            if presentation.status != "granted" {
                anyhow::bail!(
                    "{}",
                    presentation.message.unwrap_or("画面収録の権限がありません")
                )
            }
            let watch_lock =
                WatchLock::acquire(&paths.watch_lock).context("watch はすでに起動しています")?;
            let incoming_mailbox = Mailbox::new(paths.mailbox.clone(), "companion")?;
            let outgoing_mailboxes = ["app", "notify", "cli"]
                .into_iter()
                .map(|recipient| Mailbox::new(paths.mailbox.clone(), recipient))
                .collect::<Result<Vec<_>, _>>()?;
            let factory = Arc::new(WatchRuntimeFactory {
                paths: paths.clone(),
                incoming_mailbox,
                outgoing_mailboxes,
                logger,
                cancellation,
            });
            let agents = factory
                .build(config)
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            Ok::<_, anyhow::Error>((agents, factory, watch_lock))
        })
        .await?;
        let BootstrapOutcome::Started { value, signals } = outcome else {
            return Ok(None);
        };
        let (agents, factory, watch_lock) = value?;
        let cancellation = signals.cancellation();
        Ok(Some(Self {
            agents: Some(agents),
            factory,
            signals,
            cancellation,
            _watch_lock: watch_lock,
        }))
    }

    pub(super) fn take_agents(&mut self) -> Result<RuntimeAgents> {
        self.agents
            .take()
            .context("watch bootstrap の agent は取得済みです")
    }

    pub(super) fn factory(&self) -> Arc<dyn RuntimeFactory> {
        self.factory.clone()
    }

    pub(super) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub(super) async fn stop(self) {
        self.signals.stop().await;
    }
}

#[derive(Clone)]
struct WatchRuntimeFactory {
    paths: ConfigPaths,
    incoming_mailbox: Mailbox,
    outgoing_mailboxes: Vec<Mailbox>,
    logger: Arc<FileLogger>,
    cancellation: CancellationToken,
}

#[async_trait]
impl RuntimeFactory for WatchRuntimeFactory {
    async fn build(&self, config: &Config) -> Result<RuntimeAgents, String> {
        let path_value =
            coosenpai_core::provider::resolve_login_shell_path(self.cancellation.clone()).await;
        let node = crate::platform::MacHelperResolver
            .resolve_node(&path_value)
            .ok_or_else(|| "Node.js 18 以上が見つかりません".to_owned())?;
        coosenpai_core::provider::validate_node_version(
            &node,
            &path_value,
            self.cancellation.child_token(),
        )
        .await
        .map_err(|error| error.to_string())?;
        let observer_provider = make_provider(
            &config.observer.provider,
            config.observer.executable.as_deref(),
            &path_value,
        )
        .map_err(|error| error.to_string())?;
        let companion_provider = make_provider(
            &config.companion.provider,
            config.companion.executable.as_deref(),
            &path_value,
        )
        .map_err(|error| error.to_string())?;
        let persona = load_persona(&self.paths, &config.companion.persona)
            .map_err(|error| error.to_string())?;
        let observer = ObserverAgent::new(observer_provider, config.observer.clone())
            .with_usage_path(self.paths.usage.clone())
            .with_observation_store_without_read(&self.paths, config.retention.observation_days)
            .with_mailbox(self.incoming_mailbox.clone())
            .with_logger(self.logger.clone());
        let observer = if config.debug.enabled {
            observer.with_debug_store(DebugStore::from_paths(&self.paths))
        } else {
            observer
        };
        let companion = CompanionAgent::with_persona_profile(
            companion_provider.clone(),
            config.companion.clone(),
            persona,
            None,
            DeliveryOwnership::Owner,
        )
        .with_storage(&self.paths, config.retention.conversation_days)
        .with_incoming_mailbox(self.incoming_mailbox.clone())
        .with_outgoing_mailboxes(self.outgoing_mailboxes.clone())
        .with_logger(self.logger.clone())
        .with_memory_context(MemoryContext::new(
            self.paths.clone(),
            config.memory.clone(),
        ));
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
            observer: Some(observer),
            companion: Some(companion),
            memory: Some(memory),
        })
    }

    async fn shutdown(&self) {
        crate::provider_support::shutdown_provider_bridge().await;
    }
}
