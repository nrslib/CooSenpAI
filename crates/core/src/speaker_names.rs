//! 話者 ID の表示名。暗号化された声紋台帳（registry.enc）には名前を保存しないため、
//! 表示名だけを所有者限定の別メタデータとして保持する。識別モデルから名前を推測せず、
//! 利用者が設定した値だけを registry ID ごとに分離して扱う。
use crate::config::ConfigPaths;
use crate::persistence::{atomic_write_json, PersistenceError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;

const FILE_NAME: &str = "speaker-names.json";
const SCHEMA_VERSION: u8 = 3;
pub const SPEAKER_NAME_MAX_CHARS: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameIndex {
    pub schema_version: u8,
    pub registries: BTreeMap<String, BTreeMap<String, SpeakerNameRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_name: Option<PreviousSpeakerName>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviousSpeakerName {
    pub display_name: String,
    pub changed_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerNamePromptContext {
    pub scope: String,
    pub retrieval_status: String,
    pub entries: Vec<SpeakerNamePromptEntry>,
    pub previous_names: Vec<SpeakerNamePromptPreviousEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerNamePromptEntry {
    pub speaker_id: String,
    pub display_name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerNamePromptPreviousEntry {
    pub previous_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_name: Option<String>,
    pub changed_at: String,
}

// 設定・変更・解除のいずれも同じ検証を通す。空白のみは名前なしへの解除とみなさず拒否する。
pub fn validate_speaker_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("話者の表示名が空です".to_owned());
    }
    if name.chars().count() > SPEAKER_NAME_MAX_CHARS {
        return Err(format!(
            "話者の表示名は {SPEAKER_NAME_MAX_CHARS} 文字以内にしてください"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err("話者の表示名に制御文字は使えません".to_owned());
    }
    Ok(name.to_owned())
}

fn path(paths: &ConfigPaths) -> std::path::PathBuf {
    paths.speakers.join(FILE_NAME)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawSpeakerNameIndex {
    schema_version: u8,
    registries: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

fn parse(data: &[u8]) -> Result<SpeakerNameIndex, PersistenceError> {
    let raw: RawSpeakerNameIndex = serde_json::from_slice(data)?;
    if !matches!(raw.schema_version, 2 | SCHEMA_VERSION) {
        return Err(PersistenceError::Invalid(
            "話者の表示名の schemaVersion が不正です".into(),
        ));
    }
    let registries = raw
        .registries
        .into_iter()
        .map(|(registry, names)| {
            let names = names
                .into_iter()
                .map(|(speaker_id, value)| {
                    let record = if raw.schema_version == 2 {
                        SpeakerNameRecord {
                            current_name: Some(serde_json::from_value(value)?),
                            previous_name: None,
                        }
                    } else {
                        serde_json::from_value(value)?
                    };
                    Ok((speaker_id, record))
                })
                .collect::<Result<BTreeMap<_, _>, serde_json::Error>>()?;
            Ok((registry, names))
        })
        .collect::<Result<BTreeMap<_, _>, serde_json::Error>>()?;
    let index = SpeakerNameIndex {
        schema_version: SCHEMA_VERSION,
        registries,
    };
    for (registry, names) in &index.registries {
        if uuid::Uuid::parse_str(registry).is_err()
            || names
                .keys()
                .any(|id| !crate::speaker_id::is_valid_speaker_id(id))
        {
            return Err(PersistenceError::Invalid(
                "話者の表示名の registryId または話者 ID が不正です".into(),
            ));
        }
        for record in names.values() {
            for name in record.current_name.iter().chain(
                record
                    .previous_name
                    .iter()
                    .map(|previous| &previous.display_name),
            ) {
                let validated = validate_speaker_name(name).map_err(PersistenceError::Invalid)?;
                if &validated != name {
                    return Err(PersistenceError::Invalid(
                        "保存された話者の表示名に不要な空白があります".into(),
                    ));
                }
            }
            if let Some(previous) = &record.previous_name {
                chrono::DateTime::parse_from_rfc3339(&previous.changed_at).map_err(|_| {
                    PersistenceError::Invalid("旧話者名の変更時刻が不正です".into())
                })?;
            }
        }
    }
    Ok(index)
}

// 別 registry の名前は読まない。台帳が差し替わっても旧台帳の名前を流用しない。
pub fn load_speaker_name_index(paths: &ConfigPaths) -> Result<SpeakerNameIndex, PersistenceError> {
    let data = match fs::read(path(paths)) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SpeakerNameIndex {
                schema_version: SCHEMA_VERSION,
                registries: BTreeMap::new(),
            })
        }
        Err(error) => return Err(PersistenceError::Io(error)),
    };
    parse(&data)
}

// 発言が記録した registry と索引の registry が一致するときだけ表示名を返す。
pub fn speaker_name_for<'a>(
    index: &'a SpeakerNameIndex,
    registry_id: Option<&str>,
    speaker_tag: &str,
) -> Option<&'a str> {
    index
        .registries
        .get(registry_id?)?
        .get(speaker_tag)
        .and_then(|record| record.current_name.as_deref())
}

pub(crate) fn speaker_name_record_for_prompt_id<'a>(
    index: &'a SpeakerNameIndex,
    registry_id: Option<&str>,
    speaker_id: &str,
    resolver: &crate::speaker_id::PromptSpeakerIdResolver,
) -> Option<&'a SpeakerNameRecord> {
    let registry_id = registry_id?;
    let resolved = resolver.resolve(registry_id, speaker_id)?;
    index
        .registries
        .get(registry_id)?
        .get(&resolved.canonical_id)
}

pub(crate) fn speaker_name_for_prompt_id<'a>(
    index: &'a SpeakerNameIndex,
    registry_id: Option<&str>,
    speaker_id: &str,
    resolver: &crate::speaker_id::PromptSpeakerIdResolver,
) -> Option<&'a str> {
    let registry_id = registry_id?;
    let resolved = resolver.resolve(registry_id, speaker_id)?;
    speaker_name_for(index, Some(registry_id), &resolved.canonical_id)
}

pub fn load_speaker_names(
    paths: &ConfigPaths,
    registry_id: &str,
) -> Result<BTreeMap<String, String>, PersistenceError> {
    let mut index = load_speaker_name_index(paths)?;
    Ok(index
        .registries
        .remove(registry_id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(speaker_id, record)| record.current_name.map(|name| (speaker_id, name)))
        .collect())
}

// 呼び出し側の話者操作キューが read-modify-write の順序を所有するため、ここでは file lock を持たない。
// 表示名の設定・変更・解除を原子的に保存する。統合で消えた元 ID の名前は保持し、
// 統合の取り消しで元 ID が戻ったときにそのまま再表示できるようにする。
pub fn save_speaker_name(
    paths: &ConfigPaths,
    registry_id: &str,
    speaker_id: &str,
    name: Option<&str>,
) -> Result<(), PersistenceError> {
    if !crate::speaker_id::is_valid_speaker_id(speaker_id) {
        return Err(PersistenceError::Invalid(format!(
            "話者 ID が不正です: {speaker_id}"
        )));
    }
    if uuid::Uuid::parse_str(registry_id).is_err() {
        return Err(PersistenceError::Invalid(
            "話者台帳の registryId が不正です".to_owned(),
        ));
    }
    let mut index = load_speaker_name_index(paths)?;
    let name = name
        .map(validate_speaker_name)
        .transpose()
        .map_err(PersistenceError::Invalid)?;
    let names = index.registries.entry(registry_id.to_owned()).or_default();
    let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    match name {
        Some(raw) => match names.get_mut(speaker_id) {
            Some(record) if record.current_name.as_deref() == Some(&raw) => return Ok(()),
            Some(record) => {
                record.previous_name =
                    record
                        .current_name
                        .take()
                        .map(|display_name| PreviousSpeakerName {
                            display_name,
                            changed_at: now(),
                        });
                record.current_name = Some(raw);
            }
            None => {
                names.insert(
                    speaker_id.to_owned(),
                    SpeakerNameRecord {
                        current_name: Some(raw),
                        previous_name: None,
                    },
                );
            }
        },
        None => {
            let Some(record) = names.get_mut(speaker_id) else {
                return Ok(());
            };
            let Some(display_name) = record.current_name.take() else {
                return Ok(());
            };
            record.previous_name = Some(PreviousSpeakerName {
                display_name,
                changed_at: now(),
            });
        }
    }
    index.schema_version = SCHEMA_VERSION;
    atomic_write_json(&path(paths), &index)
}
