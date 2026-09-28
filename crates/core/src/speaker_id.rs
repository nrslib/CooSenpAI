//! 内部話者 ID の共通契約。
//!
//! 話者 ID は登録時に発行した UUID をそのまま永続化し、短縮表示は各 UI 層だけで行う。

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPromptSpeakerId {
    pub(crate) canonical_id: String,
    pub(crate) prompt_id: String,
}

/// 名前表、会話ログ、索引、prompt の話者 ID を同じ規則で解決する。
/// active registry だけ alias をたどり、prompt では UUID をそのまま使う。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptSpeakerIdResolver {
    active_registry_id: String,
    #[serde(default)]
    aliases: BTreeMap<String, String>,
}

impl PromptSpeakerIdResolver {
    pub fn new(active_registry_id: impl Into<String>, aliases: BTreeMap<String, String>) -> Self {
        Self {
            active_registry_id: active_registry_id.into(),
            aliases,
        }
    }

    pub(crate) fn active_registry_id(&self) -> &str {
        &self.active_registry_id
    }

    pub(crate) fn resolve(
        &self,
        registry_id: &str,
        speaker_id: &str,
    ) -> Option<ResolvedPromptSpeakerId> {
        let raw_id = raw_speaker_id_for_registry(registry_id, speaker_id)?;
        let canonical_id = if registry_id == self.active_registry_id {
            canonical_prompt_speaker_id(raw_id, &self.aliases)
        } else {
            raw_id.to_owned()
        };
        let prompt_id = if registry_id == self.active_registry_id {
            canonical_id.clone()
        } else {
            namespaced_prompt_speaker_id(registry_id, &canonical_id)?
        };
        Some(ResolvedPromptSpeakerId {
            canonical_id,
            prompt_id,
        })
    }

    /// 既にprompt形式へ正規化されたIDを、同じresolver境界内で検証する。
    pub(crate) fn normalize_prompt_id(&self, prompt_id: &str) -> Option<String> {
        if is_valid_speaker_id(prompt_id) {
            return self
                .resolve(&self.active_registry_id, prompt_id)
                .map(|resolved| resolved.prompt_id);
        }
        let (namespace, raw_id) = prompt_id.split_once('/')?;
        (namespace.len() == 18
            && namespace.starts_with("r-")
            && namespace[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
            && is_valid_speaker_id(raw_id))
        .then(|| prompt_id.to_owned())
    }
}

/// 内部話者 ID が、生成時と同じ小文字のハイフン区切り UUID かを判定する。
pub fn is_valid_speaker_id(value: &str) -> bool {
    Uuid::parse_str(value)
        .ok()
        .is_some_and(|uuid| uuid.hyphenated().to_string() == value)
}

pub(crate) fn canonical_prompt_speaker_id(id: &str, aliases: &BTreeMap<String, String>) -> String {
    let mut current = id.to_owned();
    let mut visited = HashSet::new();
    while let Some(next) = aliases.get(&current) {
        if !visited.insert(current.clone()) {
            break;
        }
        current = next.clone();
    }
    current
}

pub(crate) fn namespaced_prompt_speaker_id(registry_id: &str, id: &str) -> Option<String> {
    let uuid = Uuid::parse_str(registry_id).ok()?;
    let digest = Sha256::digest(uuid.as_bytes());
    let namespace = format!(
        "r-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    );
    let prefix = format!("{namespace}/");
    if let Some(raw_id) = id.strip_prefix(&prefix) {
        return is_valid_speaker_id(raw_id).then(|| id.to_owned());
    }
    is_valid_speaker_id(id).then(|| format!("{namespace}/{id}"))
}

pub(crate) fn raw_speaker_id_for_registry<'a>(registry_id: &str, id: &'a str) -> Option<&'a str> {
    let namespaced = namespaced_prompt_speaker_id(registry_id, id)?;
    let raw_id = if namespaced == id {
        id.split_once('/')?.1
    } else {
        id
    };
    is_valid_speaker_id(raw_id).then_some(raw_id)
}
