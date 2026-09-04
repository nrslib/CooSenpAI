use super::*;
use crate::config::{local_date, local_date_at};
use crate::image_processing::{own_window_exclusions, ExcludedBounds};
use crate::persistence::{prune_daily_jsonl, prune_daily_jsonl_at};
use crate::ports::OwnWindowBounds;
use crate::state::TranscriptRecord;
use std::path::Path;

pub(super) fn stagnation_identity(
    stagnation: Option<&crate::state::StagnationObservation>,
    fallback_now: DateTime<Utc>,
) -> Result<(String, String), ObserverError> {
    let Some(stagnation) = stagnation else {
        return Ok((Uuid::new_v4().to_string(), timestamp(fallback_now)));
    };
    match (
        stagnation.event_id.as_deref(),
        stagnation.event_created_at.as_deref(),
    ) {
        (None, None) => Ok((Uuid::new_v4().to_string(), timestamp(fallback_now))),
        (Some(id), Some(created_at))
            if !id.is_empty()
                && id.len() <= 200
                && DateTime::parse_from_rfc3339(created_at).is_ok() =>
        {
            Ok((id.to_owned(), created_at.to_owned()))
        }
        _ => Err(PersistenceError::Invalid(
            "停滞イベントの durable identity が不正です".to_owned(),
        )
        .into()),
    }
}

pub fn observation_store(paths: &ConfigPaths) -> JsonlStore {
    JsonlStore::new(paths.observations.join(format!("{}.jsonl", local_date())))
}

pub(super) fn read_latest_observation(
    paths: &ConfigPaths,
    limits: ObservationLimits,
) -> Option<Value> {
    let mut files = fs::read_dir(&paths.observations)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| JsonlStore::new(path).read::<Value>().ok())
        .flatten()
        .filter_map(|value| {
            parse_observation(value.clone(), limits)
                .ok()
                .filter(|record| !record.is_audio())
                .and_then(|record| serde_json::to_value(record).ok())
        })
        .next_back()
}

pub fn append_observation(
    paths: &ConfigPaths,
    retention_days: u64,
    observation: &ObservationRecord,
) -> Result<(), PersistenceError> {
    observation_store(paths).append(observation)?;
    prune_daily_jsonl(&paths.observations, retention_days, 50 * 1024 * 1024)
}

pub fn append_transcript(
    directory: &Path,
    retention_days: u64,
    record: &TranscriptRecord,
    now: DateTime<Utc>,
) -> Result<(), PersistenceError> {
    if record.observation_id.is_empty() {
        return Err(PersistenceError::Invalid(
            "transcript の observationId が空です".to_owned(),
        ));
    }
    let time = DateTime::parse_from_rfc3339(&record.time)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| PersistenceError::Invalid("transcript の time が不正です".to_owned()))?;
    let store = JsonlStore::new(directory.join(format!("{}.jsonl", local_date_at(time))));
    let migrated = store.rewrite::<TranscriptRecord, _>(|records| {
        let already_has_id = records
            .iter()
            .any(|existing| existing.observation_id == record.observation_id);
        let mut matched = false;
        let mut changed = false;
        records.retain_mut(|existing| {
            let is_legacy_match = existing.observation_id.is_empty()
                && existing.time == record.time
                && existing.source == record.source
                && existing.text == record.text;
            if !is_legacy_match {
                return true;
            }
            changed = true;
            if already_has_id || matched {
                return false;
            }
            existing.observation_id = record.observation_id.clone();
            matched = true;
            true
        });
        changed
    })?;
    if !migrated {
        store.append_unique(record, |existing: &TranscriptRecord| {
            !record.observation_id.is_empty() && existing.observation_id == record.observation_id
        })?;
    }
    prune_daily_jsonl_at(directory, retention_days, 50 * 1024 * 1024, now)
}

pub(super) fn reconcile_transcripts(
    observation_directory: &Path,
    transcript_directory: &Path,
    limits: ObservationLimits,
    retention_days: u64,
    now: DateTime<Utc>,
) -> Result<(), PersistenceError> {
    let mut files = fs::read_dir(observation_directory)
        .map_err(PersistenceError::Io)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    files.sort();
    for path in files {
        for value in JsonlStore::new(path).read::<Value>()? {
            let Ok(ObservationRecord::Audio(observation)) = parse_observation(value, limits) else {
                continue;
            };
            let transcript = TranscriptRecord::from_observation(&observation);
            append_transcript(transcript_directory, retention_days, &transcript, now)?;
        }
    }
    prune_daily_jsonl_at(transcript_directory, retention_days, 50 * 1024 * 1024, now)
}

pub fn excluded_bounds_for_self(
    own_windows: &OwnWindowBounds,
    now: chrono::DateTime<Utc>,
) -> Option<Vec<ExcludedBounds>> {
    own_window_exclusions(own_windows, now)
}

pub(super) fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

