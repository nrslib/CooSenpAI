use super::{eval_observations, load_persona, make_provider, ChatArgs};
use anyhow::Result;
use coosenpai_core::companion::{CompanionAgent, DeliveryOwnership};
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::debug::DebugStore;
use coosenpai_core::logging::FileLogger;
use coosenpai_core::memory::MemoryContext;
use coosenpai_core::provider::resolve_login_shell_path;
use coosenpai_core::runtime::RuntimeActor;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub(super) async fn run(
    paths: &ConfigPaths,
    config: Config,
    args: ChatArgs,
    logger: Arc<FileLogger>,
) -> Result<()> {
    let message = match args.message {
        Some(message) => message,
        None => {
            let mut input = String::new();
            tokio::io::stdin().read_to_string(&mut input).await?;
            input
        }
    };
    if message.chars().all(char::is_whitespace) {
        anyhow::bail!("chat の入力が空です")
    }
    let path_value = resolve_login_shell_path(CancellationToken::new()).await;
    let provider = make_provider(
        &config.companion.provider,
        config.companion.executable.as_deref(),
        &path_value,
    )?;
    let persona = load_persona(paths, &config.companion.persona)?;
    let observations = eval_observations::read_recent(paths, config.retention.observation_days)?;
    let companion = CompanionAgent::with_persona_profile(
        provider,
        config.companion.clone(),
        persona,
        None,
        DeliveryOwnership::None,
    )
    .with_storage(paths, config.retention.conversation_days)
    .with_logger(logger.clone())
    .with_memory_context(MemoryContext::new(paths.clone(), config.memory.clone()));
    let companion = if config.debug.enabled {
        companion.with_debug_store(DebugStore::from_paths(paths))
    } else {
        companion
    };
    let runtime = RuntimeActor::spawn_with_logger(config, None, Some(companion), logger);
    let response = runtime.user_message(message, observations).await;
    let shutdown = runtime.shutdown().await;
    let response = response?;
    shutdown?;
    println!("{}", response.message.unwrap_or_default());
    Ok(())
}
