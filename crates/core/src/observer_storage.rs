use super::*;
use crate::config::{local_date, local_date_at};
use crate::image_processing::{own_window_exclusions, ExcludedBounds};
use crate::outbox::OutboxEntry;
use crate::persistence::{
    atomic_write_json, prune_daily_jsonl, prune_daily_jsonl_at, restore_file_snapshots,
    FileSnapshot, SiblingLock,
};
use crate::ports::{HearingSpeakerCorrection, OwnWindowBounds};
use crate::state::{SpeakerIdentificationStatus, TranscriptRecord};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::path::PathBuf;

const AUDIO_MIGRATION_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioMigrationMarker {
    pub schema_version: u8,
    pub cutoff_at: String,
    #[serde(default)]
    pub delivered_audio_ids: Vec<String>,
    #[serde(default)]
    pub pending_outbox_audio_ids: Vec<String>,
}

pub(crate) struct AudioMigrationDeletionPlan {
    _lock: SiblingLock,
    marker_path: PathBuf,
    marker: Option<AudioMigrationMarker>,
    marker_snapshot: Option<FileSnapshot>,
    archive: Vec<AudioMigrationArchiveEntry>,
    audio_ids: HashSet<String>,
}

struct AudioMigrationArchiveEntry {
    id: String,
    snapshot: FileSnapshot,
}

impl AudioMigrationDeletionPlan {
    pub(crate) fn audio_ids(&self) -> &HashSet<String> {
        &self.audio_ids
    }

    pub(crate) fn apply(
        &self,
        scope: &crate::conversation_log::ConversationLogDeletionScope,
    ) -> Result<Vec<FileSnapshot>, PersistenceError> {
        let archive_snapshots = self
            .archive
            .iter()
            .filter(|entry| scope.audio_ids.contains(&entry.id))
            .map(|entry| entry.snapshot.clone())
            .collect::<Vec<_>>();
        let mut marker = self.marker.clone();
        let marker_changed = if let Some(marker) = marker.as_mut() {
            let before = marker.pending_outbox_audio_ids.clone();
            marker.pending_outbox_audio_ids.retain(|id| {
                !scope
                    .audio_ids
                    .contains(crate::state::audio_segment_observation_id(id))
            });
            marker.pending_outbox_audio_ids != before
        } else {
            false
        };
        let archive_snapshot_count = archive_snapshots.len();
        let mut snapshots = archive_snapshots;
        if marker_changed {
            snapshots.push(self.marker_snapshot.clone().ok_or_else(|| {
                PersistenceError::Invalid("音声移行記録の snapshot がありません".to_owned())
            })?);
        }
        for snapshot in &snapshots {
            if !snapshot.matches_current()? {
                return Err(PersistenceError::Invalid(format!(
                    "音声移行 archive が削除計画の作成後に変更されました: {}",
                    snapshot.path().display()
                )));
            }
        }

        let result = (|| {
            for snapshot in &snapshots[..archive_snapshot_count] {
                remove_migration_archive_file(snapshot.path())?;
            }
            if marker_changed {
                if let Some(marker) = marker.as_ref() {
                    atomic_write_json(&self.marker_path, marker)?;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(snapshots),
            Err(error) => match restore_file_snapshots(&snapshots) {
                Ok(()) => Err(error),
                Err(rollback) => Err(PersistenceError::Invalid(format!(
                    "音声移行 archive の削除に失敗し、変更の復元にも失敗しました: error={error}; rollback={rollback}"
                ))),
            },
        }
    }
}

pub(crate) fn prepare_audio_migration_deletion(
    paths: &ConfigPaths,
    date: NaiveDate,
) -> Result<AudioMigrationDeletionPlan, PersistenceError> {
    let marker_path = paths.audio_migration.clone();
    let _lock = SiblingLock::acquire(&marker_path.with_extension("lock"))?;
    let marker = read_migration_marker(&marker_path)?;
    let marker_snapshot = marker
        .as_ref()
        .map(|_| FileSnapshot::capture(&marker_path))
        .transpose()?;
    let archive_path = audio_migration_archive_path(paths);
    let mut archive = Vec::new();
    let mut audio_ids = HashSet::new();
    let entries = match fs::read_dir(&archive_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AudioMigrationDeletionPlan {
                _lock,
                marker_path,
                marker,
                marker_snapshot,
                archive,
                audio_ids,
            })
        }
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let value = serde_json::from_slice::<OutboxEntry>(&fs::read(&path)?).map_err(|error| {
            PersistenceError::Invalid(format!(
                "音声移行 archive の envelope が不正です: {}: {error}",
                path.display()
            ))
        })?;
        if value.kind != "observation" {
            continue;
        }
        let record = parse_observation(value.payload, crate::state::DEFAULT_OBSERVATION_LIMITS)
            .map_err(|error| {
                PersistenceError::Invalid(format!(
                    "音声移行 archive の observation が不正です: {}: {error}",
                    path.display()
                ))
            })?;
        let ObservationRecord::Audio(audio) = record else {
            continue;
        };
        let id = crate::state::audio_segment_observation_id(&audio.id).to_owned();
        if audio_local_date(&audio.created_at)? == date {
            audio_ids.insert(id.clone());
        }
        archive.push(AudioMigrationArchiveEntry {
            id,
            snapshot: FileSnapshot::capture(&path)?,
        });
    }
    Ok(AudioMigrationDeletionPlan {
        _lock,
        marker_path,
        marker,
        marker_snapshot,
        archive,
        audio_ids,
    })
}

fn audio_migration_archive_path(paths: &ConfigPaths) -> PathBuf {
    paths
        .audio_migration
        .with_file_name("audio-migration-outbox")
}

fn audio_local_date(value: &str) -> Result<NaiveDate, PersistenceError> {
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map_err(|error| PersistenceError::Invalid(format!("音声の時刻が不正です: {error}")))?
        .with_timezone(&Utc);
    NaiveDate::parse_from_str(&local_date_at(timestamp), "%Y-%m-%d")
        .map_err(|error| PersistenceError::Invalid(format!("音声の日付が不正です: {error}")))
}

fn remove_migration_archive_file(path: &Path) -> Result<(), PersistenceError> {
    fs::remove_file(path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConsumedAudioMarker {
    #[serde(default)]
    audio_ids: Vec<String>,
}

/// 旧版が raw Audio を直接 mailbox へ配達していた期間を、一度だけ境界付ける。
/// 未配達 outbox は移行記録を保存してから退避し、journal から新版の観察へ戻す。
pub fn migrate_legacy_audio(
    paths: &ConfigPaths,
    now: DateTime<Utc>,
) -> Result<(), PersistenceError> {
    let _lock =
        crate::persistence::SiblingLock::acquire(&paths.audio_migration.with_extension("lock"))?;
    let marker = read_migration_marker(&paths.audio_migration)?;
    let pending = read_legacy_pending_audio(&paths.outbox.join("pending"))?;
    if let Some(marker) = marker {
        return archive_legacy_pending_audio(paths, &marker, pending);
    }
    let cutoff_at = timestamp(now);
    let mut audio = HashSet::new();
    for path in jsonl_paths(&paths.observations)? {
        for value in JsonlStore::new(path).read::<Value>()? {
            if let Ok(ObservationRecord::Audio(record)) =
                parse_observation(value, crate::state::DEFAULT_OBSERVATION_LIMITS)
            {
                if record.created_at <= cutoff_at {
                    audio.insert(record.id.clone());
                }
            }
        }
    }
    let pending_evidence = pending
        .iter()
        .map(|(_, record)| record.id.clone())
        .collect::<HashSet<_>>();
    audio.extend(pending_evidence.iter().cloned());
    let done_evidence = scan_audio_ids(&paths.outbox.join("done"), &audio)?;
    let mailbox_evidence = scan_audio_ids(&paths.mailbox, &audio)?;
    let delivered_audio_ids = audio
        .iter()
        .filter(|id| done_evidence.contains(*id) || mailbox_evidence.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    let pending_outbox_audio_ids = audio
        .iter()
        .filter(|id| pending_evidence.contains(*id))
        .filter(|id| !delivered_audio_ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    let marker = AudioMigrationMarker {
        schema_version: AUDIO_MIGRATION_SCHEMA_VERSION,
        cutoff_at,
        delivered_audio_ids,
        pending_outbox_audio_ids,
    };
    atomic_write_json(&paths.audio_migration, &marker)?;
    archive_legacy_pending_audio(paths, &marker, pending)
}

fn read_legacy_pending_audio(
    directory: &Path,
) -> Result<Vec<(PathBuf, AudioObservation)>, PersistenceError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut pending = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry.path().extension().and_then(|v| v.to_str()) != Some("json")
        {
            continue;
        }
        // 壊れた outbox の隔離は DurableOutbox の既存の復旧処理に委ねる。
        let Ok(value) = serde_json::from_slice::<Value>(&fs::read(entry.path())?) else {
            continue;
        };
        let Some(payload) = value.get("payload").filter(|payload| {
            value.get("kind").and_then(Value::as_str) == Some("observation")
                && payload.get("kind").and_then(Value::as_str) == Some("audio")
        }) else {
            continue;
        };
        let record =
            parse_observation(payload.clone(), crate::state::DEFAULT_OBSERVATION_LIMITS)
                .map_err(|_| PersistenceError::Invalid("旧outboxの音声が不正です".to_owned()))?;
        if let ObservationRecord::Audio(record) = record {
            pending.push((entry.path(), record));
        }
    }
    Ok(pending)
}

fn archive_legacy_pending_audio(
    paths: &ConfigPaths,
    marker: &AudioMigrationMarker,
    pending: Vec<(PathBuf, AudioObservation)>,
) -> Result<(), PersistenceError> {
    let archive = audio_migration_archive_path(paths);
    for (path, record) in pending {
        if marker.pending_outbox_audio_ids.contains(&record.id) {
            // journal が消えていても、退避前に同じ ID で復元して新版経路へ渡す。
            append_observation_record(&paths.observations, &ObservationRecord::Audio(record))?;
        } else if !marker.delivered_audio_ids.contains(&record.id) {
            continue;
        }
        let file_name = path
            .file_name()
            .ok_or_else(|| PersistenceError::Invalid("旧outboxのパスが不正です".to_owned()))?;
        fs::create_dir_all(&archive)?;
        crate::persistence::set_private_directory_mode(&archive)?;
        fs::rename(&path, archive.join(file_name))?;
        fs::File::open(&archive)?.sync_all()?;
        fs::File::open(paths.outbox.join("pending"))?.sync_all()?;
    }
    Ok(())
}

fn read_marker<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, PersistenceError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_migration_marker(path: &Path) -> Result<Option<AudioMigrationMarker>, PersistenceError> {
    let marker = read_marker::<AudioMigrationMarker>(path)?;
    if marker.as_ref().is_some_and(|marker| {
        marker.schema_version != AUDIO_MIGRATION_SCHEMA_VERSION
            || DateTime::parse_from_rfc3339(&marker.cutoff_at).is_err()
    }) {
        return Err(PersistenceError::Invalid(
            "音声移行記録が不正です".to_owned(),
        ));
    }
    Ok(marker)
}

fn jsonl_paths(directory: &Path) -> Result<Vec<std::path::PathBuf>, PersistenceError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|v| v.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn scan_audio_ids(
    root: &Path,
    candidates: &HashSet<String>,
) -> Result<HashSet<String>, PersistenceError> {
    let mut found = HashSet::new();
    scan_audio_ids_inner(root, candidates, &mut found)?;
    Ok(found)
}

fn scan_audio_ids_inner(
    root: &Path,
    candidates: &HashSet<String>,
    found: &mut HashSet<String>,
) -> Result<(), PersistenceError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            scan_audio_ids_inner(&path, candidates, found)?;
        } else if path.extension().and_then(|v| v.to_str()) == Some("json") {
            let Ok(value) = serde_json::from_slice::<Value>(&fs::read(path)?) else {
                continue;
            };
            collect_audio_ids(&value, candidates, found);
        }
    }
    Ok(())
}

fn collect_audio_ids(value: &Value, candidates: &HashSet<String>, found: &mut HashSet<String>) {
    match value {
        Value::Object(object) => {
            if object.get("kind").and_then(Value::as_str) == Some("audio") {
                if let Some(id) = object.get("id").and_then(Value::as_str) {
                    if candidates.contains(id) {
                        found.insert(id.to_owned());
                    }
                }
            }
            object
                .values()
                .for_each(|value| collect_audio_ids(value, candidates, found));
        }
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_audio_ids(value, candidates, found)),
        _ => {}
    }
}

fn marker_path(directory: &Path) -> PathBuf {
    directory.parent().map_or_else(
        || directory.join("audio-migration.json"),
        |parent| parent.join("audio-migration.json"),
    )
}

fn consumed_marker_path(directory: &Path) -> PathBuf {
    directory.parent().map_or_else(
        || directory.join("audio-consumed.json"),
        |parent| parent.join("audio-consumed.json"),
    )
}

fn excluded_audio_ids(directory: &Path) -> Result<HashSet<String>, PersistenceError> {
    let mut ids = HashSet::new();
    if let Some(marker) = read_migration_marker(&marker_path(directory))? {
        ids.extend(marker.delivered_audio_ids);
    }
    if let Some(marker) = read_marker::<ConsumedAudioMarker>(&consumed_marker_path(directory))? {
        ids.extend(marker.audio_ids);
    }
    Ok(ids)
}

pub fn read_audio_by_ids(
    directory: &Path,
    ids: &HashSet<String>,
) -> Result<Vec<AudioObservation>, PersistenceError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut records = HashMap::new();
    for path in jsonl_paths(directory)? {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        for value in JsonlStore::new(path).read::<Value>()? {
            if let Ok(ObservationRecord::Audio(record)) =
                parse_observation(value, crate::state::DEFAULT_OBSERVATION_LIMITS)
            {
                if ids.contains(&record.id) {
                    records.insert(record.id.clone(), record);
                }
            }
        }
    }
    let mut records = records.into_values().collect::<Vec<_>>();
    records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(records)
}

/// Load only the observation records named by the caller from the known observation journal.
///
/// Callers use this for causal references that are stored directly in a conversation entry
/// rather than embedded in its screen context.  The directory is supplied by `ConfigPaths`,
/// not by user input, and only the journal's JSONL files are considered.
pub fn read_observations_by_ids(
    directory: &Path,
    ids: &[String],
) -> Result<Vec<ObservationRecord>, PersistenceError> {
    let requested = ids.iter().cloned().collect::<HashSet<_>>();
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    let mut records = HashMap::new();
    for path in jsonl_paths(directory)? {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        for value in JsonlStore::new(path).read::<Value>()? {
            if let Ok(record) = parse_observation(value, crate::state::DEFAULT_OBSERVATION_LIMITS) {
                if requested.contains(record.id()) {
                    records.insert(record.id().to_owned(), record);
                }
            }
        }
    }
    Ok(records.into_values().collect())
}

pub fn mark_audio_consumed(paths: &ConfigPaths, ids: &[String]) -> Result<(), PersistenceError> {
    if ids.is_empty() {
        return Ok(());
    }
    let _lock =
        crate::persistence::SiblingLock::acquire(&paths.audio_consumed.with_extension("lock"))?;
    let mut marker = read_marker::<ConsumedAudioMarker>(&paths.audio_consumed)?.unwrap_or_default();
    let mut seen = marker.audio_ids.iter().cloned().collect::<HashSet<_>>();
    for id in ids {
        if !id.is_empty() && seen.insert(id.clone()) {
            marker.audio_ids.push(id.clone());
        }
    }
    // journal が保持する発話の消費記録は件数で捨てない。本文の削除に合わせて掃除する。
    let retained = read_audio_by_ids(&paths.observations, &seen)?
        .into_iter()
        .map(|record| record.id)
        .collect::<HashSet<_>>();
    marker.audio_ids.retain(|id| retained.contains(id));
    atomic_write_json(&paths.audio_consumed, &marker)
}

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
                .filter(|record| {
                    !matches!(record, ObservationRecord::Audio(_))
                        && !matches!(record, ObservationRecord::Visual(v) if v.frame_count == 0)
                })
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

pub(super) fn append_observation_record(
    directory: &Path,
    record: &ObservationRecord,
) -> Result<(), PersistenceError> {
    let created_at = DateTime::parse_from_rfc3339(record.created_at())
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| PersistenceError::Invalid("観察の createdAt が不正です".to_owned()))?;
    let path = directory.join(format!("{}.jsonl", local_date_at(created_at)));
    let value = serde_json::to_value(record)?;
    JsonlStore::new(path).append_unique(&value, |existing: &Value| {
        existing.get("id").and_then(Value::as_str) == Some(record.id())
    })?;
    Ok(())
}

/// Confirmed audio is durable before waiting for an Observer or Companion provider.
pub fn record_audio_observation(
    paths: &ConfigPaths,
    retention_days: u64,
    observation: &AudioObservation,
    now: DateTime<Utc>,
) -> Result<(), PersistenceError> {
    append_observation_record(
        &paths.observations,
        &ObservationRecord::Audio(observation.clone()),
    )?;
    for transcript in TranscriptRecord::periods_from_observation(observation) {
        append_transcript(&paths.transcripts, retention_days, &transcript, now)?;
    }
    prune_daily_jsonl_at(&paths.observations, retention_days, 50 * 1024 * 1024, now)
}

pub fn apply_speaker_corrections(
    paths: &ConfigPaths,
    retention_days: u64,
    corrections: &[HearingSpeakerCorrection],
    now: DateTime<Utc>,
) -> Result<usize, PersistenceError> {
    if corrections.is_empty() {
        return Ok(0);
    }
    if corrections.iter().any(|correction| !correction.is_valid()) {
        return Err(PersistenceError::Invalid(
            "話者訂正の対象または話者 ID が不正です".to_owned(),
        ));
    }
    let mut changed_observations = Vec::new();
    let mut transcript_observations = Vec::new();
    for path in jsonl_paths(&paths.observations)? {
        JsonlStore::new(path).rewrite::<ObservationRecord, _>(|records| {
            let mut changed = false;
            for record in records.iter_mut() {
                let ObservationRecord::Audio(observation) = record else {
                    continue;
                };
                let mut observation_changed = false;
                let mut transcript_needed = false;
                for correction in corrections {
                    let correction_was_applied = observation
                        .has_speaker_correction(correction)
                        .is_ok_and(|applied| applied);
                    let correction_changed = observation
                        .apply_speaker_correction(correction)
                        .is_ok_and(|applied| applied);
                    if correction_changed {
                        observation_changed = true;
                    }
                    if correction_changed || correction_was_applied {
                        transcript_needed = true;
                    }
                }
                if observation_changed {
                    changed = true;
                    changed_observations.push(observation.clone());
                }
                if transcript_needed {
                    transcript_observations.push(observation.clone());
                }
            }
            changed
        })?;
    }
    let mut unique_observations = HashMap::new();
    for observation in changed_observations {
        unique_observations.insert(observation.id.clone(), observation);
    }
    let changed_count = unique_observations.len();
    let mut unique_transcript_observations = HashMap::new();
    for observation in transcript_observations {
        unique_transcript_observations.insert(observation.id.clone(), observation);
    }
    for observation in unique_transcript_observations.into_values() {
        for transcript in TranscriptRecord::periods_from_observation(&observation) {
            append_transcript(&paths.transcripts, retention_days, &transcript, now)?;
        }
    }
    prune_daily_jsonl_at(&paths.observations, retention_days, 50 * 1024 * 1024, now)?;
    Ok(changed_count)
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
    if record.audio_start_ms.is_some() != record.audio_end_ms.is_some() {
        return Err(PersistenceError::Invalid(
            "transcript の音声時刻は開始と終了がそろっている必要があります".to_owned(),
        ));
    }
    let time = DateTime::parse_from_rfc3339(&record.time)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| PersistenceError::Invalid("transcript の time が不正です".to_owned()))?;
    let store = JsonlStore::new(directory.join(format!("{}.jsonl", local_date_at(time))));
    let mut invalid_identity = false;
    let mut identity_seen = false;
    let changed = store.rewrite::<TranscriptRecord, _>(|records| {
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
            if is_legacy_match {
                changed = true;
                if already_has_id || matched {
                    return false;
                }
                existing.observation_id = record.observation_id.clone();
                matched = true;
                return true;
            }
            if !same_transcript_identity(existing, record) {
                return true;
            }
            identity_seen = true;
            if existing.time != record.time
                || existing.source != record.source
                || existing.text != record.text
                || existing.audio_end_ms != record.audio_end_ms
            {
                invalid_identity = true;
                return true;
            }
            if existing.speaker_status == Some(SpeakerIdentificationStatus::Identified) {
                if record.speaker_status == Some(SpeakerIdentificationStatus::Identified)
                    && (existing.speaker_tag != record.speaker_tag
                        || existing.speaker_registry_id != record.speaker_registry_id)
                {
                    invalid_identity = true;
                }
                return true;
            }
            if record.speaker_status == Some(SpeakerIdentificationStatus::Identified) {
                existing.speaker_tag = record.speaker_tag.clone();
                existing.speaker_registry_id = record.speaker_registry_id.clone();
                existing.speaker_status = record.speaker_status;
                changed = true;
            }
            true
        });
        changed
    })?;
    if invalid_identity {
        return Err(PersistenceError::Invalid(
            "同じ期間 ID の transcript が異なる内容を持ちます".to_owned(),
        ));
    }
    if !changed && !identity_seen {
        store.append_unique(record, |existing: &TranscriptRecord| {
            // 期間行は (observation_id, audio_start_ms, audio_end_ms) の組で重複排除し、
            // 音声時刻を持たない従来行は従来どおり observation_id のみで判定する。
            !record.observation_id.is_empty()
                && existing.observation_id == record.observation_id
                && existing.audio_start_ms == record.audio_start_ms
                && existing.audio_end_ms == record.audio_end_ms
        })?;
    }
    prune_daily_jsonl_at(directory, retention_days, 50 * 1024 * 1024, now)
}

fn same_transcript_identity(left: &TranscriptRecord, right: &TranscriptRecord) -> bool {
    !right.observation_id.is_empty()
        && left.observation_id == right.observation_id
        && left.audio_start_ms == right.audio_start_ms
        && left.audio_end_ms == right.audio_end_ms
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
            for transcript in TranscriptRecord::periods_from_observation(&observation) {
                append_transcript(transcript_directory, retention_days, &transcript, now)?;
            }
        }
    }
    prune_daily_jsonl_at(transcript_directory, retention_days, 50 * 1024 * 1024, now)
}

/// 共通観察と移行・消費記録を除外して、journal の未処理発話を読む。
pub fn read_unobserved_audio(directory: &Path) -> Result<Vec<AudioObservation>, PersistenceError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut audio = std::collections::BTreeMap::new();
    let mut observed = HashSet::new();
    let excluded = excluded_audio_ids(directory)?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|v| v.to_str()) != Some("jsonl") {
            continue;
        }
        for value in JsonlStore::new(path).read::<Value>()? {
            let record = parse_observation(value, crate::state::DEFAULT_OBSERVATION_LIMITS)
                .map_err(|_| PersistenceError::Invalid("音声観察 journal が不正です".to_owned()))?;
            match record {
                ObservationRecord::Audio(record) => {
                    audio.insert(record.id.clone(), record);
                }
                ObservationRecord::Visual(record) => {
                    observed.extend(record.audio_segments.into_iter().map(|segment| {
                        crate::state::audio_segment_observation_id(&segment.id).to_owned()
                    }));
                }
                ObservationRecord::NoChange(_) => {}
            }
        }
    }
    let mut pending = audio
        .into_values()
        .filter(|v| !observed.contains(&v.id) && !excluded.contains(&v.id))
        .collect::<Vec<_>>();
    pending.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(pending)
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

