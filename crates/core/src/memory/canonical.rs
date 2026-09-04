use super::jobs::{DailySummary, SummaryState, WeeklyDependency};
use super::types::{CanonicalRecord, SourceSnapshot};
use chrono::DateTime;
use chrono::Datelike;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CanonicalError {
    #[error("canonical JSON を生成できません: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceInputKind {
    Conversation,
    Observation,
}

#[derive(Debug, Clone, Copy)]
pub struct SourceInput<'a> {
    pub kind: SourceInputKind,
    pub line: &'a [u8],
}

pub fn canonical_daily_source(
    inputs: &[SourceInput<'_>],
    max_bytes: usize,
) -> Result<SourceSnapshot, CanonicalError> {
    let mut skipped_invalid_count = 0;
    let mut seen = HashSet::<(SourceInputKind, String)>::new();
    let mut records = Vec::new();
    for input in inputs {
        let Some(record) = parse_record(*input) else {
            skipped_invalid_count += 1;
            continue;
        };
        let key = (input.kind, record.id().to_owned());
        if seen.insert(key) {
            records.push(record);
        }
    }
    records.sort_by(|left, right| {
        left.created_at_ms()
            .cmp(&right.created_at_ms())
            .then_with(|| left.kind().as_bytes().cmp(right.kind().as_bytes()))
            .then_with(|| left.id().as_bytes().cmp(right.id().as_bytes()))
    });
    let mut encoded = records
        .iter()
        .map(encode_record)
        .collect::<Result<Vec<Vec<u8>>, _>>()?;
    let mut total = encoded.iter().map(Vec::len).sum::<usize>();
    let mut removed = 0;
    while total > max_bytes && removed < encoded.len() {
        total -= encoded[removed].len();
        removed += 1;
    }
    let canonical_bytes = encoded.drain(removed..).flatten().collect::<Vec<_>>();
    let source_ids = records
        .into_iter()
        .skip(removed)
        .map(|record| record.id().to_owned())
        .collect();
    Ok(SourceSnapshot {
        source_digest: sha256_hex(&canonical_bytes),
        source_ids,
        truncated: removed > 0,
        skipped_invalid_count,
        canonical_bytes,
    })
}

pub fn memory_job_id(
    kind: &str,
    period: &str,
    source_digest: &str,
    prompt_version: u32,
    provider: &str,
    model: &str,
) -> String {
    let prompt_version = prompt_version.to_string();
    let mut bytes = Vec::new();
    for value in [
        kind,
        period,
        source_digest,
        prompt_version.as_str(),
        provider,
        model,
    ] {
        bytes.extend_from_slice(value.len().to_string().as_bytes());
        bytes.push(b':');
        bytes.extend_from_slice(value.as_bytes());
    }
    sha256_hex(&bytes)
}

pub(crate) fn canonical_weekly_source(
    daily: &[DailySummary],
    period: &str,
    max_bytes: usize,
) -> Result<(SourceSnapshot, Vec<WeeklyDependency>), CanonicalError> {
    let mut summaries = daily
        .iter()
        .filter(|summary| {
            summary.state == SummaryState::Current
                && DateTime::parse_from_rfc3339(&summary.generated_at).is_ok()
                && chrono::NaiveDate::parse_from_str(&summary.local_date, "%Y-%m-%d").is_ok_and(
                    |date| {
                        let week = date.iso_week();
                        format!("{}-W{:02}", week.year(), week.week()) == period
                    },
                )
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| left.local_date.cmp(&right.local_date));
    let mut encoded = summaries
        .iter()
        .map(|summary| {
            let mut bytes = serde_json::to_vec(&sorted_object([
                ("kind", Value::String("daily".to_owned())),
                ("localDate", Value::String(summary.local_date.clone())),
                ("text", Value::String(summary.text.clone())),
                ("textDigest", Value::String(summary.text_digest.clone())),
            ]))?;
            bytes.push(b'\n');
            Ok(bytes)
        })
        .collect::<Result<Vec<Vec<u8>>, serde_json::Error>>()?;
    let mut total = encoded.iter().map(Vec::len).sum::<usize>();
    let mut removed = 0;
    while total > max_bytes && removed < encoded.len() {
        total -= encoded[removed].len();
        removed += 1;
    }
    let canonical_bytes = encoded.drain(removed..).flatten().collect::<Vec<_>>();
    let remaining = summaries.into_iter().skip(removed).collect::<Vec<_>>();
    let dependencies = remaining
        .iter()
        .map(|summary| WeeklyDependency {
            local_date: summary.local_date.clone(),
            text_digest: summary.text_digest.clone(),
        })
        .collect::<Vec<_>>();
    Ok((
        SourceSnapshot {
            source_digest: sha256_hex(&canonical_bytes),
            source_ids: remaining
                .iter()
                .map(|summary| summary.local_date.clone())
                .collect(),
            truncated: removed > 0,
            skipped_invalid_count: 0,
            canonical_bytes,
        },
        dependencies,
    ))
}

fn parse_record(input: SourceInput<'_>) -> Option<CanonicalRecord> {
    let value: Value = serde_json::from_slice(input.line).ok()?;
    let object = value.as_object()?;
    if object.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let id = required_string(object, "id")?.to_owned();
    let created_at_ms = DateTime::parse_from_rfc3339(required_string(object, "createdAt")?)
        .ok()?
        .timestamp_millis();
    match input.kind {
        SourceInputKind::Conversation => parse_conversation(object, id, created_at_ms),
        SourceInputKind::Observation => parse_observation(object, id, created_at_ms),
    }
}

fn parse_conversation(
    object: &Map<String, Value>,
    id: String,
    created_at_ms: i64,
) -> Option<CanonicalRecord> {
    let role = required_string(object, "role")?;
    if !matches!(role, "user" | "companion") {
        return None;
    }
    Some(CanonicalRecord::Conversation {
        id,
        created_at_ms,
        role: role.to_owned(),
        message: required_string(object, "message")?.to_owned(),
    })
}

fn parse_observation(
    object: &Map<String, Value>,
    id: String,
    created_at_ms: i64,
) -> Option<CanonicalRecord> {
    let outline = parse_outline(object)?;
    let changes = object
        .get("changes")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let guess = match object.get("guess") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return None,
    };
    Some(CanonicalRecord::Observation {
        id,
        created_at_ms,
        activity: required_string(object, "activity")?.to_owned(),
        outline,
        changes,
        guess,
    })
}

fn parse_outline(object: &Map<String, Value>) -> Option<String> {
    let current = match object.get("outline") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return None,
    };
    let legacy = match object.get("textExcerpts") {
        None => None,
        Some(value) => Some(parse_legacy_outline(value)?),
    };
    match (current, legacy) {
        (Some(current), Some(legacy)) if current.is_empty() => Some(legacy),
        (Some(current), Some(legacy)) if legacy.is_empty() => Some(current),
        (Some(current), Some(legacy)) => Some(format!("{current}\n{legacy}")),
        (Some(current), None) => Some(current),
        (None, Some(legacy)) => Some(legacy),
        (None, None) => None,
    }
}

fn parse_legacy_outline(value: &Value) -> Option<String> {
    value
        .as_array()?
        .iter()
        .map(|value| {
            let object = value.as_object()?;
            if object
                .keys()
                .any(|key| !["region", "app", "text"].contains(&key.as_str()))
            {
                return None;
            }
            match object.get("region").and_then(Value::as_str) {
                Some("terminal" | "editor" | "browser" | "chat" | "other") => {}
                _ => return None,
            }
            match object.get("app") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) => Some(value),
                Some(_) => return None,
            };
            Some(required_string(object, "text")?.to_owned())
        })
        .collect::<Option<Vec<_>>>()
        .map(|texts| texts.join("\n"))
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

fn encode_record(record: &CanonicalRecord) -> Result<Vec<u8>, serde_json::Error> {
    let value = match record {
        CanonicalRecord::Conversation {
            id,
            created_at_ms,
            role,
            message,
        } => sorted_object([
            ("createdAt", Value::from(*created_at_ms)),
            ("id", Value::String(id.clone())),
            ("kind", Value::String("conversation".to_owned())),
            ("message", Value::String(message.clone())),
            ("role", Value::String(role.clone())),
        ]),
        CanonicalRecord::Observation {
            id,
            created_at_ms,
            activity,
            outline,
            changes,
            guess,
        } => sorted_object([
            ("activity", Value::String(activity.clone())),
            (
                "changes",
                Value::Array(changes.iter().cloned().map(Value::String).collect()),
            ),
            ("createdAt", Value::from(*created_at_ms)),
            ("guess", guess.clone().map_or(Value::Null, Value::String)),
            ("id", Value::String(id.clone())),
            ("kind", Value::String("observation".to_owned())),
            ("outline", Value::String(outline.clone())),
        ]),
    };
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sorted_object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let sorted = entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect::<BTreeMap<_, _>>();
    Value::Object(sorted.into_iter().collect())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
