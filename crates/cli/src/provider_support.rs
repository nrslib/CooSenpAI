use anyhow::{Context, Result};
use coosenpai_core::ports::HelperResolverPort;
use coosenpai_core::provider::{
    resolve_executable, BridgeLaunch, ProviderBridge, ProviderClient, ProviderName,
};
use coosenpai_core::provider_api_keys::bridge_environment;
use std::sync::{Arc, OnceLock};

static BRIDGE: OnceLock<ProviderBridge> = OnceLock::new();

pub(super) fn make_provider(
    name: &str,
    override_path: Option<&str>,
    path_value: &str,
) -> Result<Arc<dyn ProviderClient>> {
    let name = match name {
        "codex" => ProviderName::Codex,
        "claude" => ProviderName::Claude,
        "opencode" => ProviderName::Opencode,
        _ => anyhow::bail!("provider が不正です: {name}"),
    };
    let executable = resolve_executable(override_path.unwrap_or(name.as_str()), path_value)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let bridge = match BRIDGE.get() {
        Some(bridge) => bridge,
        None => {
            let resolver = crate::platform::MacHelperResolver;
            let executable_dir = std::env::current_exe()?
                .parent()
                .map(ToOwned::to_owned)
                .context("実行ファイルのディレクトリを取得できません")?;
            let product_root = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .context("HOME が設定されていません")?
                .join(".coosenpai");
            let development_dist = super::repository_root().join("tools/provider-bridge/dist");
            let script = resolver
                .resolve_provider_bridge(&executable_dir, &product_root, None)
                .or_else(|| {
                    let development_bridge = development_dist.join("bridge.js");
                    development_bridge.is_file().then_some(development_bridge)
                })
                .context("provider bridge が見つかりません。tools/provider-bridge/build.sh を実行してください")?;
            let node = resolver
                .resolve_node(path_value)
                .context("Node.js 18 以上が見つかりません")?;
            let keychain = crate::platform::provider_api_key_store();
            let env = bridge_environment(
                std::env::vars(),
                path_value,
                &product_root,
                keychain.as_ref(),
            )
            .map_err(|_| anyhow::anyhow!("macOS キーチェーンから API キーを読み込めません"))?;
            let candidate = ProviderBridge::new(BridgeLaunch { node, script, env });
            let _ = BRIDGE.set(candidate);
            BRIDGE.get().context("provider bridge を初期化できません")?
        }
    };
    Ok(Arc::new(bridge.provider(name, Some(executable))))
}

pub(super) async fn shutdown_provider_bridge() {
    if let Some(bridge) = BRIDGE.get() {
        bridge.shutdown().await;
    }
}
