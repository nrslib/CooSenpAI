use super::{
    build_memory_block, FactStore, MemoryBlock, MemoryFactError, MemoryRetrievalError,
    MemoryRetrievalInput, MemorySearchRecord, MemoryStore, MemoryStoreError,
};
use crate::companion_storage::conversation_entry_from_storage_value;
use crate::config::{ConfigPaths, MemoryConfig};
use crate::persistence::SiblingLock;
use crate::state::{parse_observation, ObservationRecord, DEFAULT_OBSERVATION_LIMITS};
use chrono::{DateTime, Local, Utc};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryContextError {
    #[error(transparent)]
    Store(#[from] MemoryStoreError),
    #[error(transparent)]
    Facts(#[from] MemoryFactError),
    #[error(transparent)]
    Retrieval(#[from] MemoryRetrievalError),
    #[error("記憶検索の I/O に失敗しました: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct MemoryContext {
    paths: ConfigPaths,
    config: MemoryConfig,
}

impl MemoryContext {
    pub fn new(paths: ConfigPaths, config: MemoryConfig) -> Self {
        Self { paths, config }
    }

    pub fn build(
        &self,
        query: &str,
        recent_ids: &HashSet<String>,
        now: DateTime<Utc>,
    ) -> Result<MemoryBlock, MemoryContextError> {
        if !self.config.enabled || !self.config.provider_consent {
            return Ok(MemoryBlock::empty());
        }
        let store = MemoryStore::new(self.paths.clone());
        let facts = FactStore::new(self.paths.clone());
        let mut search_records = load_search_records(&self.paths)?;
        let available_periods = store.available_daily_periods()?;
        let daily = store
            .daily_summaries()?
            .into_iter()
            .filter(|summary| {
                let Ok(period) = chrono::NaiveDate::parse_from_str(&summary.local_date, "%Y-%m-%d")
                else {
                    return false;
                };
                !available_periods.contains(&period)
                    || store
                        .daily_source(&summary.local_date, self.config.source_max_bytes)
                        .is_ok_and(|source| source.source_digest == summary.source_digest)
            })
            .collect::<Vec<_>>();
        let today = now.with_timezone(&Local).date_naive();
        search_records.extend(daily.iter().filter_map(|summary| {
            let date = chrono::NaiveDate::parse_from_str(&summary.local_date, "%Y-%m-%d").ok()?;
            let age = today.signed_duration_since(date).num_days();
            (summary.state == super::SummaryState::Current && (0..=7).contains(&age)).then(|| {
                MemorySearchRecord {
                    id: summary.local_date.clone(),
                    kind: "daily".to_owned(),
                    created_at: summary.generated_at.clone(),
                    text: summary.text.clone(),
                }
            })
        }));
        let active_facts = facts.active_facts()?.into_values().collect::<Vec<_>>();
        let weekly = store.weekly_summaries()?;
        let block = build_memory_block(&MemoryRetrievalInput {
            query,
            today,
            search_records: &search_records,
            daily: &daily,
            facts: &active_facts,
            weekly: &weekly,
            recent_ids,
            source_max_bytes: self.config.source_max_bytes,
            max_bytes: self.config.prompt_max_bytes,
        })?;
        if !block.used_fact_ids.is_empty() {
            let _ = facts.record_usage(
                &block.used_fact_ids,
                &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            );
        }
        Ok(block)
    }

    pub fn facts(&self) -> FactStore {
        FactStore::new(self.paths.clone())
    }

    pub fn config(&self) -> &MemoryConfig {
        &self.config
    }
}

impl MemoryBlock {
    pub fn empty() -> Self {
        Self {
            serialized: String::new(),
            included_ids: Vec::new(),
            used_fact_ids: Vec::new(),
        }
    }
}

fn load_search_records(paths: &ConfigPaths) -> Result<Vec<MemorySearchRecord>, std::io::Error> {
    let mut records = Vec::new();
    let conversation_generation =
        crate::conversation_archive::current_conversation_generation(paths)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
    for path in jsonl_paths(&paths.conversation)? {
        for value in jsonl_values(&path)? {
            if let Some(entry) =
                conversation_entry_from_storage_value(value, conversation_generation)
            {
                records.push(MemorySearchRecord {
                    id: entry.id,
                    kind: "conversation".to_owned(),
                    created_at: entry.created_at,
                    text: entry.message,
                });
            }
        }
    }
    for path in jsonl_paths(&paths.observations)? {
        for value in jsonl_values(&path)? {
            if let Ok(observation) = parse_observation(value, DEFAULT_OBSERVATION_LIMITS) {
                let (id, created_at, text) = match observation {
                    ObservationRecord::Visual(observation) => {
                        let mut parts = vec![observation.data.activity, observation.data.outline];
                        parts.extend(observation.data.changes);
                        (observation.id, observation.created_at, parts.join("\n"))
                    }
                    ObservationRecord::Audio(observation) => {
                        (observation.id, observation.created_at, observation.text)
                    }
                    ObservationRecord::NoChange(_) => continue,
                };
                records.push(MemorySearchRecord {
                    id,
                    kind: "observation".to_owned(),
                    created_at,
                    text,
                });
            }
        }
    }
    Ok(records)
}

fn jsonl_paths(directory: &Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn jsonl_values(path: &Path) -> Result<Vec<Value>, std::io::Error> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| std::io::Error::other("JSONL のファイル名が不正です"))?;
    let _lock = SiblingLock::acquire(&path.with_file_name(format!(".{name}.lock")))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let file = fs::File::open(path)?;
    Ok(BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect())
}
