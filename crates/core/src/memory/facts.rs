use crate::config::{ConfigPaths, MemoryConfig};
use crate::persistence::{atomic_write_json, JsonlStore, PersistenceError, SiblingLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use thiserror::Error;

const FACT_TEXT_MAX_CHARS: usize = 500;
const FACT_SOURCE_IDS_MAX: usize = 10;
const FACT_ITEMS_PER_CALL_MAX: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactCandidate {
    pub id: String,
    pub text: String,
    pub source_user_message_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FactUpdateOperation {
    Expire,
    Merge,
    Rewrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactUpdate {
    pub id: String,
    pub operation: FactUpdateOperation,
    pub fact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactCandidatesSnapshot {
    pub schema_version: u8,
    pub candidates: Vec<FactCandidate>,
    pub updates: Vec<FactUpdate>,
}

impl Default for FactCandidatesSnapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            candidates: Vec::new(),
            updates: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactRecord {
    pub schema_version: u8,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_user_message_ids: Vec<String>,
    pub confirmation_id: String,
    pub confirmed_at: String,
    pub tombstone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FactUsage {
    #[serde(default = "schema_version")]
    schema_version: u8,
    #[serde(default)]
    items: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum MemoryFactError {
    #[error("fact の永続化に失敗しました: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("fact の I/O に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("fact JSON が不正です: {0}")]
    Json(#[from] serde_json::Error),
    #[error("fact の入力が不正です: {path}: {message}")]
    Validation { path: String, message: String },
    #[error("fact candidate が見つかりません")]
    CandidateNotFound,
    #[error("fact update が見つかりません")]
    UpdateNotFound,
    #[error("確認済み fact の容量上限に達しました")]
    Capacity,
}

#[derive(Debug, Clone)]
pub struct FactStore {
    paths: ConfigPaths,
}

impl FactStore {
    pub fn new(paths: ConfigPaths) -> Self {
        Self { paths }
    }

    pub fn validate_candidates(
        &self,
        raw_candidates: &serde_json::Value,
        raw_updates: &serde_json::Value,
        allowed_user_ids: &HashSet<String>,
        now: &str,
        config: &MemoryConfig,
    ) -> Result<FactCandidatesSnapshot, MemoryFactError> {
        let candidates = parse_candidates(raw_candidates, allowed_user_ids, now)?;
        let active_fact_ids = self.active_facts()?.into_keys().collect::<HashSet<_>>();
        let updates = parse_updates(raw_updates, &active_fact_ids, now)?;
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".fact-candidates.lock"))?;
        let mut snapshot = self.load_candidates_unlocked()?;
        for candidate in candidates {
            if !snapshot
                .candidates
                .iter()
                .any(|item| item.id == candidate.id)
            {
                snapshot.candidates.push(candidate);
            }
        }
        for update in updates {
            if !snapshot.updates.iter().any(|item| item.id == update.id) {
                snapshot.updates.push(update);
            }
        }
        trim_candidates(&mut snapshot, config)?;
        atomic_write_json(&self.paths.memory_fact_candidates, &snapshot)?;
        Ok(snapshot)
    }

    pub fn load_candidates(&self) -> Result<FactCandidatesSnapshot, MemoryFactError> {
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".fact-candidates.lock"))?;
        self.load_candidates_unlocked()
    }

    pub fn reconcile(&self, config: &MemoryConfig) -> Result<(), MemoryFactError> {
        let active_ids = self.active_facts()?.into_keys().collect::<HashSet<_>>();
        {
            let _lock = SiblingLock::acquire(&self.paths.memory.join(".fact-candidates.lock"))?;
            let mut snapshot = self.load_candidates_unlocked()?;
            let before = snapshot.clone();
            trim_candidates(&mut snapshot, config)?;
            if snapshot != before {
                atomic_write_json(&self.paths.memory_fact_candidates, &snapshot)?;
            }
        }
        if self.paths.memory_fact_usage.exists() {
            self.update_usage(|usage| usage.items.retain(|id, _| active_ids.contains(id)))?;
        }
        Ok(())
    }

    fn load_candidates_unlocked(&self) -> Result<FactCandidatesSnapshot, MemoryFactError> {
        if !self.paths.memory_fact_candidates.exists() {
            return Ok(FactCandidatesSnapshot::default());
        }
        Ok(serde_json::from_slice(&fs::read(
            &self.paths.memory_fact_candidates,
        )?)?)
    }

    pub fn confirm(
        &self,
        candidate_id: &str,
        confirmation_id: &str,
        confirmed_at: &str,
        config: &MemoryConfig,
    ) -> Result<FactRecord, MemoryFactError> {
        require_nonempty("confirmationId", confirmation_id)?;
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".fact-candidates.lock"))?;
        let mut candidates = self.load_candidates_unlocked()?;
        if let Some(existing) = self
            .active_facts()?
            .into_values()
            .find(|record| record.confirmation_id == confirmation_id)
        {
            candidates
                .candidates
                .retain(|candidate| candidate.id != candidate_id);
            atomic_write_json(&self.paths.memory_fact_candidates, &candidates)?;
            return Ok(existing);
        }
        let candidate = candidates
            .candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
            .cloned()
            .ok_or(MemoryFactError::CandidateNotFound)?;
        let record = FactRecord {
            schema_version: 1,
            id: fact_id(confirmation_id),
            text: Some(candidate.text),
            source_user_message_ids: candidate.source_user_message_ids,
            confirmation_id: confirmation_id.to_owned(),
            confirmed_at: confirmed_at.to_owned(),
            tombstone: false,
        };
        let active = self.active_facts()?;
        if !active.contains_key(&record.id) {
            ensure_fact_capacity(&active, &record, config)?;
        }
        JsonlStore::new(self.paths.memory_facts.clone())
            .append_idempotent(&record, |existing: &FactRecord| {
                existing.confirmation_id == confirmation_id
            })?;
        candidates
            .candidates
            .retain(|candidate| candidate.id != candidate_id);
        atomic_write_json(&self.paths.memory_fact_candidates, &candidates)?;
        Ok(record)
    }

    pub fn reject(&self, candidate_id: &str) -> Result<(), MemoryFactError> {
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".fact-candidates.lock"))?;
        let mut candidates = self.load_candidates_unlocked()?;
        candidates
            .candidates
            .retain(|candidate| candidate.id != candidate_id);
        atomic_write_json(&self.paths.memory_fact_candidates, &candidates)?;
        Ok(())
    }

    pub fn reject_update(&self, update_id: &str) -> Result<(), MemoryFactError> {
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".fact-candidates.lock"))?;
        let mut candidates = self.load_candidates_unlocked()?;
        candidates.updates.retain(|update| update.id != update_id);
        atomic_write_json(&self.paths.memory_fact_candidates, &candidates)?;
        Ok(())
    }

    pub fn confirm_update(
        &self,
        update_id: &str,
        confirmation_id: &str,
        confirmed_at: &str,
        config: &MemoryConfig,
    ) -> Result<(), MemoryFactError> {
        require_nonempty("updateId", update_id)?;
        require_nonempty("confirmationId", confirmation_id)?;
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".fact-candidates.lock"))?;
        let mut proposals = self.load_candidates_unlocked()?;
        let Some(update) = proposals
            .updates
            .iter()
            .find(|update| update.id == update_id)
            .cloned()
        else {
            let derived_prefix = format!("{confirmation_id}:");
            let already_applied = self.fact_records()?.iter().any(|record| {
                record.confirmation_id == confirmation_id
                    || record.confirmation_id.starts_with(&derived_prefix)
            });
            return if already_applied {
                Ok(())
            } else {
                Err(MemoryFactError::UpdateNotFound)
            };
        };
        self.apply_confirmed_update(&update, confirmation_id, confirmed_at, config)?;
        self.update_usage(|usage| {
            for fact_id in &update.fact_ids {
                usage.items.remove(fact_id);
            }
        })?;
        proposals.updates.retain(|item| item.id != update_id);
        atomic_write_json(&self.paths.memory_fact_candidates, &proposals)?;
        Ok(())
    }

    fn apply_confirmed_update(
        &self,
        update: &FactUpdate,
        confirmation_id: &str,
        confirmed_at: &str,
        config: &MemoryConfig,
    ) -> Result<(), MemoryFactError> {
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".facts.jsonl.lock"))?;
        let mut records = read_fact_records_unlocked(&self.paths.memory_facts)?;
        let derived_prefix = format!("{confirmation_id}:");
        if records.iter().any(|record| {
            record.confirmation_id == confirmation_id
                || record.confirmation_id.starts_with(&derived_prefix)
        }) {
            return Ok(());
        }
        let mut active = active_from_records(&records);
        if matches!(
            update.operation,
            FactUpdateOperation::Merge | FactUpdateOperation::Rewrite
        ) {
            let replacement_confirmation = format!("{confirmation_id}:replacement");
            let mut source_user_message_ids = update
                .fact_ids
                .iter()
                .filter_map(|id| active.get(id))
                .flat_map(|fact| fact.source_user_message_ids.iter().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            source_user_message_ids.sort();
            let replacement = FactRecord {
                schema_version: 1,
                id: fact_id(&replacement_confirmation),
                text: update.replacement.clone(),
                source_user_message_ids,
                confirmation_id: replacement_confirmation,
                confirmed_at: confirmed_at.to_owned(),
                tombstone: false,
            };
            for id in &update.fact_ids {
                active.remove(id);
            }
            ensure_fact_capacity(&active, &replacement, config)?;
            records.push(replacement);
        }
        records.extend(update.fact_ids.iter().map(|id| FactRecord {
            schema_version: 1,
            id: id.clone(),
            text: None,
            source_user_message_ids: Vec::new(),
            confirmation_id: format!("{confirmation_id}:delete:{id}"),
            confirmed_at: confirmed_at.to_owned(),
            tombstone: true,
        }));
        #[cfg(test)]
        fact_update_failpoint()?;
        crate::persistence::atomic_write_bytes(
            &self.paths.memory_facts,
            &serialize_fact_records(&records)?,
        )?;
        Ok(())
    }

    pub fn delete(
        &self,
        fact_id: &str,
        confirmation_id: &str,
        confirmed_at: &str,
    ) -> Result<(), MemoryFactError> {
        require_nonempty("factId", fact_id)?;
        require_nonempty("confirmationId", confirmation_id)?;
        self.append_tombstone(fact_id, confirmation_id, confirmed_at)?;
        self.update_usage(|usage| {
            usage.items.remove(fact_id);
        })?;
        Ok(())
    }

    fn append_tombstone(
        &self,
        fact_id: &str,
        confirmation_id: &str,
        confirmed_at: &str,
    ) -> Result<(), MemoryFactError> {
        let tombstone = FactRecord {
            schema_version: 1,
            id: fact_id.to_owned(),
            text: None,
            source_user_message_ids: Vec::new(),
            confirmation_id: confirmation_id.to_owned(),
            confirmed_at: confirmed_at.to_owned(),
            tombstone: true,
        };
        JsonlStore::new(self.paths.memory_facts.clone())
            .append_idempotent(&tombstone, |existing: &FactRecord| {
                existing.confirmation_id == confirmation_id
            })?;
        Ok(())
    }

    pub fn active_facts(&self) -> Result<HashMap<String, FactRecord>, MemoryFactError> {
        Ok(active_from_records(&self.fact_records()?))
    }

    fn fact_records(&self) -> Result<Vec<FactRecord>, MemoryFactError> {
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".facts.jsonl.lock"))?;
        read_fact_records_unlocked(&self.paths.memory_facts)
    }

    pub fn record_usage(&self, fact_ids: &[String], used_at: &str) -> Result<(), MemoryFactError> {
        let active = self.active_facts()?;
        self.update_usage(|usage| {
            for id in fact_ids.iter().filter(|id| active.contains_key(*id)) {
                usage.items.insert(id.clone(), used_at.to_owned());
            }
            usage.items.retain(|id, _| active.contains_key(id));
        })
    }

    pub fn compact(&self) -> Result<(), MemoryFactError> {
        let store = JsonlStore::new(self.paths.memory_facts.clone());
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".facts.jsonl.lock"))?;
        let mut active = HashMap::new();
        for record in read_fact_records_unlocked(store.path())? {
            if record.tombstone {
                active.remove(&record.id);
            } else {
                active.insert(record.id.clone(), record);
            }
        }
        let mut records = active.into_values().collect::<Vec<_>>();
        records.sort_by(|left, right| left.id.cmp(&right.id));
        let mut output = Vec::new();
        for (index, record) in records.iter().enumerate() {
            if index > 0 {
                output.push(b'\n');
            }
            output.extend(serde_json::to_vec(record)?);
        }
        if !output.is_empty() {
            output.push(b'\n');
        }
        crate::persistence::atomic_write_bytes(store.path(), &output)?;
        Ok(())
    }

    fn update_usage<F>(&self, update: F) -> Result<(), MemoryFactError>
    where
        F: FnOnce(&mut FactUsage),
    {
        let lock_path = self.paths.memory.join(".fact-usage.lock");
        let _lock = SiblingLock::acquire(&lock_path)?;
        let mut usage = if self.paths.memory_fact_usage.exists() {
            serde_json::from_slice(&fs::read(&self.paths.memory_fact_usage)?)?
        } else {
            FactUsage {
                schema_version: 1,
                items: BTreeMap::new(),
            }
        };
        update(&mut usage);
        atomic_write_json(&self.paths.memory_fact_usage, &usage)?;
        Ok(())
    }
}

fn parse_candidates(
    value: &serde_json::Value,
    allowed_user_ids: &HashSet<String>,
    now: &str,
) -> Result<Vec<FactCandidate>, MemoryFactError> {
    let items = value
        .as_array()
        .ok_or_else(|| validation("factCandidates", "配列で指定してください。"))?;
    if items.len() > FACT_ITEMS_PER_CALL_MAX {
        return Err(validation("factCandidates", "5件以下で指定してください。"));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let object = item.as_object().ok_or_else(|| {
                validation(
                    format!("factCandidates[{index}]"),
                    "object で指定してください。",
                )
            })?;
            reject_unknown_keys(
                object,
                &["text", "sourceUserMessageIds"],
                format!("factCandidates[{index}]"),
            )?;
            let text = object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .filter(|text| {
                    !text.trim().is_empty() && text.chars().count() <= FACT_TEXT_MAX_CHARS
                })
                .ok_or_else(|| {
                    validation(
                        format!("factCandidates[{index}].text"),
                        "1〜500文字で指定してください。",
                    )
                })?;
            let source_ids = object
                .get("sourceUserMessageIds")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    validation(
                        format!("factCandidates[{index}].sourceUserMessageIds"),
                        "配列で指定してください。",
                    )
                })?;
            if source_ids.is_empty() || source_ids.len() > FACT_SOURCE_IDS_MAX {
                return Err(validation(
                    format!("factCandidates[{index}].sourceUserMessageIds"),
                    "1〜10件で指定してください。",
                ));
            }
            let source_user_message_ids = source_ids
                .iter()
                .map(|id| id.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    validation(
                        format!("factCandidates[{index}].sourceUserMessageIds"),
                        "文字列 ID で指定してください。",
                    )
                })?;
            if source_user_message_ids
                .iter()
                .any(|id| !allowed_user_ids.contains(id))
            {
                return Err(validation(
                    format!("factCandidates[{index}].sourceUserMessageIds"),
                    "当該呼び出しの user entry だけを参照できます。",
                ));
            }
            Ok(FactCandidate {
                id: candidate_id(text, &source_user_message_ids),
                text: text.to_owned(),
                source_user_message_ids,
                created_at: now.to_owned(),
            })
        })
        .collect()
}

fn parse_updates(
    value: &serde_json::Value,
    active_fact_ids: &HashSet<String>,
    now: &str,
) -> Result<Vec<FactUpdate>, MemoryFactError> {
    let items = value
        .as_array()
        .ok_or_else(|| validation("factUpdates", "配列で指定してください。"))?;
    if items.len() > FACT_ITEMS_PER_CALL_MAX {
        return Err(validation("factUpdates", "5件以下で指定してください。"));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let object = item.as_object().ok_or_else(|| {
                validation(
                    format!("factUpdates[{index}]"),
                    "object で指定してください。",
                )
            })?;
            reject_unknown_keys(
                object,
                &["operation", "factIds", "replacement", "reason"],
                format!("factUpdates[{index}]"),
            )?;
            let operation = match object.get("operation").and_then(serde_json::Value::as_str) {
                Some("expire") => FactUpdateOperation::Expire,
                Some("merge") => FactUpdateOperation::Merge,
                Some("rewrite") => FactUpdateOperation::Rewrite,
                _ => {
                    return Err(validation(
                        format!("factUpdates[{index}].operation"),
                        "expire、merge、rewrite のいずれかで指定してください。",
                    ))
                }
            };
            let fact_ids = object
                .get("factIds")
                .and_then(serde_json::Value::as_array)
                .and_then(|items| {
                    items
                        .iter()
                        .map(|id| id.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
                .filter(|ids| !ids.is_empty() && ids.len() <= FACT_SOURCE_IDS_MAX)
                .ok_or_else(|| {
                    validation(
                        format!("factUpdates[{index}].factIds"),
                        "1〜10件の文字列 ID で指定してください。",
                    )
                })?;
            if fact_ids.iter().any(|id| !active_fact_ids.contains(id)) {
                return Err(validation(
                    format!("factUpdates[{index}].factIds"),
                    "存在する確認済み fact だけを参照できます。",
                ));
            }
            let replacement = object
                .get("replacement")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if matches!(
                operation,
                FactUpdateOperation::Merge | FactUpdateOperation::Rewrite
            ) && replacement.as_deref().is_none_or(|value| {
                value.trim().is_empty() || value.chars().count() > FACT_TEXT_MAX_CHARS
            }) {
                return Err(validation(
                    format!("factUpdates[{index}].replacement"),
                    "merge / rewrite では1〜500文字で指定してください。",
                ));
            }
            let reason = object
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .filter(|reason| {
                    !reason.trim().is_empty() && reason.chars().count() <= FACT_TEXT_MAX_CHARS
                })
                .ok_or_else(|| {
                    validation(
                        format!("factUpdates[{index}].reason"),
                        "1〜500文字で指定してください。",
                    )
                })?;
            let id = update_id(operation, &fact_ids, replacement.as_deref(), reason);
            Ok(FactUpdate {
                id,
                operation,
                fact_ids,
                replacement,
                reason: reason.to_owned(),
                created_at: now.to_owned(),
            })
        })
        .collect()
}

fn trim_candidates(
    snapshot: &mut FactCandidatesSnapshot,
    config: &MemoryConfig,
) -> Result<(), MemoryFactError> {
    snapshot.candidates.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    snapshot.updates.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    while snapshot.candidates.len() + snapshot.updates.len() > config.candidate_limit
        || serde_json::to_vec_pretty(snapshot)?.len() > config.candidate_max_bytes
    {
        let remove_candidate = match (snapshot.candidates.first(), snapshot.updates.first()) {
            (Some(candidate), Some(update)) => candidate.created_at <= update.created_at,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return Err(MemoryFactError::Capacity),
        };
        if remove_candidate {
            snapshot.candidates.remove(0);
        } else if !snapshot.updates.is_empty() {
            snapshot.updates.remove(0);
        } else {
            return Err(MemoryFactError::Capacity);
        }
    }
    Ok(())
}

fn reject_unknown_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    path: String,
) -> Result<(), MemoryFactError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Err(validation(format!("{path}.{key}"), "未知のキーです。"))
    } else {
        Ok(())
    }
}

fn ensure_fact_capacity(
    active: &HashMap<String, FactRecord>,
    candidate: &FactRecord,
    config: &MemoryConfig,
) -> Result<(), MemoryFactError> {
    if active.len() >= config.fact_limit {
        return Err(MemoryFactError::Capacity);
    }
    let current = active
        .values()
        .map(|record| serde_json::to_vec(record).map(|bytes| bytes.len() + 1))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<usize>();
    if current + serde_json::to_vec(candidate)?.len() + 1 > config.fact_max_bytes {
        return Err(MemoryFactError::Capacity);
    }
    Ok(())
}

fn candidate_id(text: &str, ids: &[String]) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(text.as_bytes());
    for id in ids {
        bytes.push(0);
        bytes.extend_from_slice(id.as_bytes());
    }
    format!("candidate-{:x}", Sha256::digest(bytes))
}

fn update_id(
    operation: FactUpdateOperation,
    ids: &[String],
    replacement: Option<&str>,
    reason: &str,
) -> String {
    candidate_id(
        &format!(
            "{operation:?}\0{}\0{reason}",
            replacement.unwrap_or_default()
        ),
        ids,
    )
    .replacen("candidate-", "update-", 1)
}

fn fact_id(confirmation_id: &str) -> String {
    format!(
        "fact-{:x}",
        Sha256::digest([b"fact\0".as_slice(), confirmation_id.as_bytes()].concat())
    )
}

fn require_nonempty(path: &str, value: &str) -> Result<(), MemoryFactError> {
    if value.trim().is_empty() {
        Err(validation(path, "空にできません。"))
    } else {
        Ok(())
    }
}

fn validation(path: impl Into<String>, message: impl Into<String>) -> MemoryFactError {
    MemoryFactError::Validation {
        path: path.into(),
        message: message.into(),
    }
}

fn schema_version() -> u8 {
    1
}

fn read_fact_records_unlocked(path: &std::path::Path) -> Result<Vec<FactRecord>, MemoryFactError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

fn active_from_records(records: &[FactRecord]) -> HashMap<String, FactRecord> {
    let mut active = HashMap::new();
    for record in records {
        if record.tombstone {
            active.remove(&record.id);
        } else {
            active.insert(record.id.clone(), record.clone());
        }
    }
    active
}

fn serialize_fact_records(records: &[FactRecord]) -> Result<Vec<u8>, serde_json::Error> {
    let mut output = Vec::new();
    for record in records {
        output.extend(serde_json::to_vec(record)?);
        output.push(b'\n');
    }
    Ok(output)
}

#[cfg(test)]
static FACT_UPDATE_FAILPOINT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

