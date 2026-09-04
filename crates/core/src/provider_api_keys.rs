use crate::ports::{PortError, ProviderApiKeyStore};
use crate::provider::ProviderName;
use serde::Serialize;
use std::path::Path;

pub const SUPPORTED_PROVIDERS: [ProviderName; 3] = [
    ProviderName::Codex,
    ProviderName::Claude,
    ProviderName::Opencode,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderApiKeyStatus {
    pub codex: bool,
    pub claude: bool,
    pub opencode: bool,
}

impl ProviderApiKeyStatus {
    pub fn read(store: &dyn ProviderApiKeyStore) -> Result<Self, PortError> {
        Ok(Self {
            codex: store.read(ProviderName::Codex)?.is_some(),
            claude: store.read(ProviderName::Claude)?.is_some(),
            opencode: store.read(ProviderName::Opencode)?.is_some(),
        })
    }
}

pub fn environment_names(provider: ProviderName) -> &'static [&'static str] {
    match provider {
        ProviderName::Codex => &["OPENAI_API_KEY"],
        ProviderName::Claude => &["ANTHROPIC_API_KEY"],
        ProviderName::Opencode => &["OPENCODE_API_KEY", "OPENCODE_ZEN_API_KEY"],
    }
}

pub fn bridge_environment(
    base_environment: impl IntoIterator<Item = (String, String)>,
    path_value: &str,
    product_root: &Path,
    store: &dyn ProviderApiKeyStore,
) -> Result<Vec<(String, String)>, PortError> {
    let mut environment = base_environment
        .into_iter()
        .filter(|(key, _)| key != "PATH")
        .collect::<Vec<_>>();
    environment.push(("PATH".to_owned(), path_value.to_owned()));
    environment.push((
        "COOSENPAI_HOME".to_owned(),
        product_root.to_string_lossy().into_owned(),
    ));

    for provider in SUPPORTED_PROVIDERS {
        let Some(api_key) = store.read(provider)? else {
            continue;
        };
        for name in environment_names(provider) {
            environment.retain(|(key, _)| key != name);
            environment.push(((*name).to_owned(), api_key.clone()));
        }
    }
    Ok(environment)
}

