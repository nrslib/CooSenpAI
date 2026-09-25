use crate::config::ConfigPaths;
use crate::observer::{
    canonical_prompt_speaker_id, load_speaker_aliases, prepare_audio_migration_deletion,
};
use crate::persistence::{restore_file_snapshots, FileSnapshot, JsonlStore, PersistenceError};
use crate::speaker_names::{load_speaker_name_index, speaker_name_for};
use crate::state::{
    parse_observation, ConversationEntry, ObservationRecord, SpeakerIdentificationStatus,
    TranscriptRecord, UserScreenContext, DEFAULT_OBSERVATION_LIMITS,
};
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerDecisionView {
    #[serde(flatten)]
    pub decision: crate::speaker_decision::SpeakerDecisionDetails,
    pub candidate_names: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_comparison_sources: Option<Vec<SpeakerRecentComparisonSource>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerRecentComparisonSource {
    pub segment_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationLogEntry {
    pub observation_id: String,
    pub time: String,
    pub source: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_status: Option<SpeakerIdentificationStatus>,
    // 表示時の名前解決。過去の記録は書き換えず、現在の表示名だけを載せる。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_name: Option<String>,
    // 同一観察の期間行は observationId と時刻が重複するため、行識別の複合キーに使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_start_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_end_ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub speaker_decision_details: Vec<SpeakerDecisionView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationLog {
    pub dates: Vec<String>,
    pub selected_date: String,
    pub entries: Vec<ConversationLogEntry>,
    pub truncated: bool,
}

pub fn conversation_log_window(today: NaiveDate, days: i64) -> Vec<NaiveDate> {
    (0..days)
        .map(|offset| today - Duration::days(offset))
        .rev()
        .collect()
}

// 本文は transcripts、話者情報は観察記録から組む。どちらも dataflow_log と同じ日次 JSONL の読み手を使う。
pub fn read_conversation_log(
    paths: &ConfigPaths,
    date: Option<NaiveDate>,
    days: i64,
    limit: usize,
) -> Result<ConversationLog, PersistenceError> {
    let today = Local::now().date_naive();
    let window = conversation_log_window(today, days);
    let selected = match date {
        Some(date) => date,
        None => window
            .iter()
            .rev()
            .find(|date| transcript_path(paths, **date).is_file())
            .copied()
            .unwrap_or(today),
    };
    let speakers = speaker_index(paths, selected)?;
    let transcript_records =
        JsonlStore::new(transcript_path(paths, selected)).read::<TranscriptRecord>()?;
    let mut comparison_sources = HashMap::new();
    for transcript in &transcript_records {
        if transcript.source != "speaker" {
            continue;
        }
        let Some(info) = speakers.get(&transcript.observation_id) else {
            continue;
        };
        let Some(segment_id) = info.segment_id.as_deref() else {
            continue;
        };
        let (Some(start), Some(end)) = (transcript.audio_start_ms, transcript.audio_end_ms) else {
            continue;
        };
        comparison_sources
            .entry((segment_id.to_owned(), start, end))
            .or_insert_with(|| (transcript.time.clone(), transcript.text.clone()));
    }
    // 統合は過去の記録を書き換えず別名索引へ残るため、表示用の話者 ID は観察 prompt と同じ索引で統合先へ解決する。
    let aliases = load_speaker_aliases(paths)
        .map_err(|error| PersistenceError::Invalid(error.to_string()))?;
    // 表示名も台帳・記録を書き換えず、現在の registry の索引から表示時に解決する。
    let names = load_speaker_name_index(paths)?;
    let mut entries = Vec::new();
    for record in transcript_records {
        let observed = speakers.get(&record.observation_id);
        let details = observed
            .map(|info| match (record.audio_start_ms, record.audio_end_ms) {
                (Some(start), Some(end)) => info
                    .period_details
                    .get(&(start, end))
                    .cloned()
                    .unwrap_or_default(),
                (None, None) => info.details.clone(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        let speaker_decision_details = details
            .into_iter()
            .map(|decision| {
                let candidate_names = decision
                    .candidates
                    .iter()
                    .filter_map(|candidate| {
                        let registry = decision.registry_id.as_deref();
                        let id = if registry == Some(aliases.registry_id.as_str()) {
                            canonical_prompt_speaker_id(&candidate.speaker_id, &aliases.aliases)
                        } else {
                            candidate.speaker_id.clone()
                        };
                        speaker_name_for(&names, registry, &id)
                            .map(|name| (candidate.speaker_id.clone(), name.to_owned()))
                    })
                    .collect();
                let recent_comparison_sources =
                    decision.recent_comparisons.as_ref().map(|comparisons| {
                        comparisons
                            .iter()
                            .map(|comparison| {
                                let source = comparison_sources.get(&(
                                    comparison.segment_id.clone(),
                                    comparison.start_ms,
                                    comparison.end_ms,
                                ));
                                SpeakerRecentComparisonSource {
                                    segment_id: comparison.segment_id.clone(),
                                    start_ms: comparison.start_ms,
                                    end_ms: comparison.end_ms,
                                    time: source.map(|(time, _)| time.clone()),
                                    text: source.map(|(_, text)| text.clone()),
                                }
                            })
                            .collect()
                    });
                SpeakerDecisionView {
                    decision,
                    candidate_names,
                    recent_comparison_sources,
                }
            })
            .collect();
        let registry_id = record
            .speaker_registry_id
            .clone()
            .or_else(|| observed.and_then(|info| info.registry_id.clone()));
        let tag = record
            .speaker_tag
            .or_else(|| observed.and_then(|info| info.tag.clone()));
        let current_registry = registry_id.as_deref() == Some(aliases.registry_id.as_str());
        let tag = tag.map(|tag| {
            if current_registry {
                canonical_prompt_speaker_id(&tag, &aliases.aliases)
            } else {
                tag
            }
        });
        let speaker_name = tag
            .as_deref()
            .and_then(|tag| speaker_name_for(&names, registry_id.as_deref(), tag))
            .map(str::to_owned);
        entries.push(ConversationLogEntry {
            observation_id: record.observation_id.clone(),
            time: record.time,
            source: record.source,
            text: record.text,
            speaker_tag: tag,
            speaker_status: record
                .speaker_status
                .or_else(|| observed.and_then(|info| info.status)),
            speaker_name,
            audio_start_ms: record.audio_start_ms,
            audio_end_ms: record.audio_end_ms,
            speaker_decision_details,
        });
    }
    entries.sort_by(|left, right| left.time.cmp(&right.time));
    let truncated = entries.len() > limit;
    if truncated {
        entries.drain(..entries.len() - limit);
    }
    Ok(ConversationLog {
        dates: window
            .iter()
            .map(|date| date.format("%Y-%m-%d").to_string())
            .collect(),
        selected_date: selected.format("%Y-%m-%d").to_string(),
        entries,
        truncated,
    })
}

/// 削除操作が対象とする観察 ID の集合。
///
/// `observation_ids` は Coo の pending/cursor/mailbox/outbox から古い参照を取り除くために
/// 使う。Visual の音声区間だけを永続 observation から取り除く場合も、その Visual 自体を
/// Coo の保留配達から外す。`audio_ids` は cursor が持つ共有音声 journal 参照用である。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConversationLogDeletionScope {
    pub(crate) date: NaiveDate,
    pub(crate) observation_ids: HashSet<String>,
    pub(crate) audio_ids: HashSet<String>,
    pub(crate) transcript_exists: bool,
}

struct ObservationRewrite {
    path: PathBuf,
    original: Vec<serde_json::Value>,
    updated: Vec<serde_json::Value>,
    snapshot: FileSnapshot,
}

struct ConversationRewrite {
    path: PathBuf,
    original: Vec<serde_json::Value>,
    updated: Vec<serde_json::Value>,
    snapshot: FileSnapshot,
}

pub(crate) struct ConversationLogDeletionPlan {
    scope: ConversationLogDeletionScope,
    rewrites: Vec<ObservationRewrite>,
    conversation_rewrites: Vec<ConversationRewrite>,
    transcript: PathBuf,
    transcript_exists: bool,
    transcript_snapshot: Option<FileSnapshot>,
    audio_migration: crate::observer::AudioMigrationDeletionPlan,
}

pub(crate) struct ConversationLogDeletionReceipt {
    // 削除適用後に呼び出し側が復元できる rollback 境界を維持する。
    #[allow(dead_code)]
    snapshots: Vec<FileSnapshot>,
}

impl ConversationLogDeletionReceipt {
    // 現行の呼び出し側は受領のみだが、削除後 rollback API を保持する。
    #[allow(dead_code)]
    pub(crate) fn rollback(&self) -> Result<(), PersistenceError> {
        restore_file_snapshots(&self.snapshots)
    }
}

/// 指定日の本文と音声観察の削除計画を作る。
///
/// observation の保存先は Visual の作成時刻で決まるため、選択日の JSONL だけに限定せず、
/// 全 observation JSONL の音声区間時刻を調べる。純粋な Vision/NoChange は変更しない。
pub(crate) fn prepare_conversation_log_day_deletion(
    paths: &ConfigPaths,
    date: NaiveDate,
) -> Result<ConversationLogDeletionPlan, PersistenceError> {
    build_deletion_plan(paths, date)
}

impl ConversationLogDeletionPlan {
    pub(crate) fn scope(&self) -> &ConversationLogDeletionScope {
        &self.scope
    }

    pub(crate) fn apply(&self) -> Result<ConversationLogDeletionReceipt, PersistenceError> {
        let mut applied_snapshots = Vec::new();
        for rewrite in &self.rewrites {
            let expected = rewrite.original.clone();
            let updated = rewrite.updated.clone();
            match JsonlStore::new(rewrite.path.clone()).rewrite::<serde_json::Value, _>(|records| {
                if *records != expected {
                    return false;
                }
                *records = updated;
                true
            }) {
                Ok(true) => applied_snapshots.push(rewrite.snapshot.clone()),
                Ok(false) => {
                    return Err(compensate_deletion_failure(
                        PersistenceError::Invalid(format!(
                            "観察 JSONL が削除計画の作成後に変更されました: {}",
                            rewrite.path.display()
                        )),
                        &applied_snapshots,
                        None,
                    ));
                }
                Err(error) => {
                    return Err(compensate_deletion_failure(error, &applied_snapshots, None));
                }
            }
        }

        if self.transcript_exists {
            if let Err(error) = before_transcript_remove(&self.transcript) {
                return Err(compensate_deletion_failure(error, &applied_snapshots, None));
            }
            if let Some(snapshot) = &self.transcript_snapshot {
                match snapshot.matches_current() {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err(compensate_deletion_failure(
                            PersistenceError::Invalid(
                                "transcript が削除計画の作成後に変更されました".to_owned(),
                            ),
                            &applied_snapshots,
                            None,
                        ));
                    }
                    Err(error) => {
                        return Err(compensate_deletion_failure(error, &applied_snapshots, None));
                    }
                }
            }
            match JsonlStore::new(self.transcript.clone()).remove() {
                Ok(true) => {}
                Ok(false) => {
                    return Err(compensate_deletion_failure(
                        PersistenceError::Invalid(
                            "削除対象の transcript が計画適用時に見つかりません".to_owned(),
                        ),
                        &applied_snapshots,
                        self.transcript_snapshot.as_ref(),
                    ));
                }
                Err(error) => {
                    return Err(compensate_deletion_failure(
                        error,
                        &applied_snapshots,
                        self.transcript_snapshot.as_ref(),
                    ));
                }
            }
            if let Err(error) = after_transcript_remove(&self.transcript) {
                return Err(compensate_deletion_failure(
                    error,
                    &applied_snapshots,
                    self.transcript_snapshot.as_ref(),
                ));
            }
        }

        let migration_snapshots = match self.audio_migration.apply(&self.scope) {
            Ok(snapshots) => snapshots,
            Err(error) => {
                return Err(compensate_deletion_failure(
                    error,
                    &applied_snapshots,
                    self.transcript_snapshot.as_ref(),
                ));
            }
        };
        let mut snapshots = applied_snapshots;
        if let Some(snapshot) = &self.transcript_snapshot {
            snapshots.push(snapshot.clone());
        }
        snapshots.extend(migration_snapshots);
        for rewrite in &self.conversation_rewrites {
            let expected = rewrite.original.clone();
            let updated = rewrite.updated.clone();
            match JsonlStore::new(rewrite.path.clone()).rewrite::<serde_json::Value, _>(|records| {
                if *records != expected {
                    return false;
                }
                *records = updated;
                true
            }) {
                Ok(true) => snapshots.push(rewrite.snapshot.clone()),
                Ok(false) => {
                    return Err(compensate_deletion_failure(
                        PersistenceError::Invalid(format!(
                            "conversation JSONL が削除計画の作成後に変更されました: {}",
                            rewrite.path.display()
                        )),
                        &snapshots,
                        None,
                    ));
                }
                Err(error) => {
                    return Err(compensate_deletion_failure(error, &snapshots, None));
                }
            }
        }
        Ok(ConversationLogDeletionReceipt { snapshots })
    }
}

/// 指定日の会話本文と、その本文を復元できる Audio 観察を削除する。
///
/// 観察の日次ファイルには Vision や NoChange も混在するため、ファイル全体は削除しない。
/// `NaiveDate` は呼び出し側で日付窓を検証済みであることを前提に、パスは日付から組み立てる。
pub fn delete_conversation_log_day(
    paths: &ConfigPaths,
    date: NaiveDate,
) -> Result<(), PersistenceError> {
    prepare_conversation_log_day_deletion(paths, date)?.apply()?;
    Ok(())
}

fn build_deletion_plan(
    paths: &ConfigPaths,
    date: NaiveDate,
) -> Result<ConversationLogDeletionPlan, PersistenceError> {
    let transcript = transcript_path(paths, date);
    let transcript_exists = existing_file(&transcript)?;
    let transcript_records =
        JsonlStore::new(transcript.clone()).read_strict::<TranscriptRecord>()?;
    for record in &transcript_records {
        validate_transcript_record(record)?;
    }
    let transcript_snapshot = if transcript_exists {
        Some(FileSnapshot::capture(&transcript)?)
    } else {
        None
    };

    let migration = prepare_audio_migration_deletion(paths, date)?;
    let mut files = Vec::new();
    let mut audio_ids = migration.audio_ids().clone();
    let mut observation_ids = migration.audio_ids().clone();
    for record in &transcript_records {
        if !record.observation_id.is_empty() {
            observation_ids.insert(record.observation_id.clone());
            audio_ids.insert(record.observation_id.clone());
        }
    }
    for path in observation_paths(paths)? {
        let original = JsonlStore::new(path.clone()).read_strict::<serde_json::Value>()?;
        for value in &original {
            let record =
                parse_observation(value.clone(), DEFAULT_OBSERVATION_LIMITS).map_err(|error| {
                    PersistenceError::Invalid(format!(
                        "観察 JSONL の observation が不正です: {}: {error}",
                        path.display()
                    ))
                })?;
            validate_observation_timestamps(&record)?;
            match record {
                ObservationRecord::Audio(audio)
                    if local_date_of(&audio.created_at) == Some(date) =>
                {
                    audio_ids
                        .insert(crate::state::audio_segment_observation_id(&audio.id).to_owned());
                }
                ObservationRecord::Visual(visual) => {
                    for segment in visual.audio_segments {
                        if local_date_of(&segment.time) == Some(date) {
                            audio_ids.insert(
                                crate::state::audio_segment_observation_id(&segment.id).to_owned(),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        files.push((path, original));
    }

    let mut scope = ConversationLogDeletionScope {
        date,
        observation_ids: std::mem::take(&mut observation_ids),
        audio_ids,
        transcript_exists,
    };
    let mut rewrites = Vec::new();
    for (path, original) in files {
        let mut updated = Vec::with_capacity(original.len());
        let mut changed = false;
        for value in &original {
            let record =
                parse_observation(value.clone(), DEFAULT_OBSERVATION_LIMITS).map_err(|error| {
                    PersistenceError::Invalid(format!(
                        "観察 JSONL の observation が不正です: {}: {error}",
                        path.display()
                    ))
                })?;
            let (record, removed_audio_ids) =
                remove_audio_from_record(record, date, &scope.audio_ids)?;
            let record_changed = !removed_audio_ids.is_empty();
            if record_changed {
                changed = true;
                let id = value_id(value);
                if id.is_empty() {
                    return Err(PersistenceError::Invalid(
                        "削除対象 observation の ID が空です".to_owned(),
                    ));
                }
                scope.observation_ids.insert(id);
            }
            if let Some(record) = record {
                updated.push(serde_json::to_value(record)?);
            }
        }
        if changed {
            rewrites.push(ObservationRewrite {
                snapshot: FileSnapshot::capture(&path)?,
                path,
                original,
                updated,
            });
        }
    }
    let conversation_rewrites = prepare_conversation_rewrites(paths, &mut scope)?;

    Ok(ConversationLogDeletionPlan {
        scope,
        rewrites,
        conversation_rewrites,
        transcript,
        transcript_exists,
        transcript_snapshot,
        audio_migration: migration,
    })
}

fn existing_file(path: &Path) -> Result<bool, PersistenceError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(PersistenceError::Invalid(format!(
            "削除対象のパスがファイルではありません: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn observation_paths(paths: &ConfigPaths) -> Result<Vec<PathBuf>, PersistenceError> {
    match fs::metadata(&paths.observations) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(PersistenceError::Invalid(format!(
                "observations のパスがディレクトリではありません: {}",
                paths.observations.display()
            )))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    }
    if !paths.observations.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&paths.observations)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn conversation_paths(paths: &ConfigPaths) -> Result<Vec<PathBuf>, PersistenceError> {
    let directories = crate::companion_storage::conversation_log_directories(
        &paths.conversation,
        &paths.archive,
    )?;
    let mut files = Vec::new();
    for directory in directories {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let is_daily = path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    value.len() == 16
                        && value.ends_with(".jsonl")
                        && value[..10]
                            .chars()
                            .all(|character| character.is_ascii_digit() || character == '-')
                });
            if is_daily {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn prepare_conversation_rewrites(
    paths: &ConfigPaths,
    scope: &mut ConversationLogDeletionScope,
) -> Result<Vec<ConversationRewrite>, PersistenceError> {
    let files = conversation_paths(paths)?
        .into_iter()
        .map(|path| {
            let records = JsonlStore::new(path.clone()).read_strict::<serde_json::Value>()?;
            Ok((path, records))
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?;

    // 先に全 conversation の埋込み参照を走査し、後続の pending/outbox と同じ scope を確定する。
    for (_, records) in &files {
        for value in records {
            let normalized = normalized_conversation_value(value);
            let Some(context) = conversation_screen_context(&normalized)? else {
                continue;
            };
            collect_screen_context_scope(&context, scope);
        }
    }

    let mut rewrites = Vec::new();
    for (path, original) in files {
        let mut updated = Vec::with_capacity(original.len());
        let mut changed = false;
        for value in &original {
            let sanitized = sanitize_conversation_value(value, scope)?;
            changed |= sanitized != *value;
            updated.push(sanitized);
        }
        if changed {
            rewrites.push(ConversationRewrite {
                snapshot: FileSnapshot::capture(&path)?,
                path,
                original,
                updated,
            });
        }
    }
    Ok(rewrites)
}

fn normalized_conversation_value(value: &serde_json::Value) -> serde_json::Value {
    let mut normalized = value.clone();
    crate::companion_storage::normalize_conversation_audio_context(&mut normalized);
    normalized
}

fn conversation_screen_context(
    value: &serde_json::Value,
) -> Result<Option<UserScreenContext>, PersistenceError> {
    let Some(context) = value.get("screenContext") else {
        return Ok(None);
    };
    serde_json::from_value(context.clone())
        .map(Some)
        .map_err(|error| {
            PersistenceError::Invalid(format!("conversation の screenContext が不正です: {error}"))
        })
}

fn collect_screen_context_scope(
    context: &UserScreenContext,
    scope: &mut ConversationLogDeletionScope,
) {
    for observation in &context.observations {
        match observation {
            ObservationRecord::Audio(audio)
                if scope
                    .audio_ids
                    .contains(crate::state::audio_segment_observation_id(&audio.id))
                    || is_selected_audio_date(&audio.created_at, scope) =>
            {
                let id = crate::state::audio_segment_observation_id(&audio.id).to_owned();
                scope.audio_ids.insert(id.clone());
                scope.observation_ids.insert(id);
            }
            ObservationRecord::Audio(_) => {}
            ObservationRecord::Visual(visual) => {
                let mut audio_removed = false;
                for segment in &visual.audio_segments {
                    let id = crate::state::audio_segment_observation_id(&segment.id);
                    if scope.audio_ids.contains(id) || is_selected_audio_date(&segment.time, scope)
                    {
                        scope.audio_ids.insert(id.to_owned());
                        audio_removed = true;
                    }
                }
                if audio_removed {
                    scope.observation_ids.insert(visual.id.clone());
                }
            }
            ObservationRecord::NoChange(_) => {}
        }
    }
    for audio in &context.pending_audio {
        if scope
            .audio_ids
            .contains(crate::state::audio_segment_observation_id(&audio.id))
            || is_selected_audio_date(&audio.created_at, scope)
        {
            let id = crate::state::audio_segment_observation_id(&audio.id).to_owned();
            scope.audio_ids.insert(id.clone());
            scope.observation_ids.insert(id);
        }
    }
    for id in &context.pending_audio_ids {
        let observation_id = crate::state::audio_segment_observation_id(id);
        if scope.audio_ids.contains(observation_id) {
            scope.observation_ids.insert(observation_id.to_owned());
        }
    }
}

fn sanitize_conversation_value(
    value: &serde_json::Value,
    scope: &ConversationLogDeletionScope,
) -> Result<serde_json::Value, PersistenceError> {
    let normalized = normalized_conversation_value(value);
    let Some(original_context) = normalized.get("screenContext") else {
        return Ok(value.clone());
    };
    let context = conversation_screen_context(&normalized)?.ok_or_else(|| {
        PersistenceError::Invalid("conversation の screenContext がありません".to_owned())
    })?;
    let entry_date_is_selected = value
        .get("createdAt")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|created_at| is_selected_audio_date(created_at, scope));
    let (sanitized, _) =
        sanitize_user_screen_context(context.clone(), scope, entry_date_is_selected)?;
    if sanitized == context {
        return Ok(value.clone());
    }

    let mut sanitized_context = serde_json::to_value(&sanitized)?;
    if original_context.get("pendingAudio").is_some() {
        sanitized_context
            .as_object_mut()
            .ok_or_else(|| {
                PersistenceError::Invalid(
                    "sanitized screenContext が object ではありません".to_owned(),
                )
            })?
            .insert(
                "pendingAudio".to_owned(),
                serde_json::to_value(&sanitized.pending_audio)?,
            );
    }
    let mut updated = normalized;
    let object = updated.as_object_mut().ok_or_else(|| {
        PersistenceError::Invalid("conversation entry が object ではありません".to_owned())
    })?;
    if sanitized.is_empty() {
        object.remove("screenContext");
    } else {
        object.insert("screenContext".to_owned(), sanitized_context);
    }
    Ok(updated)
}

pub(crate) fn sanitize_conversation_entry(
    entry: &ConversationEntry,
    scope: &ConversationLogDeletionScope,
) -> Result<ConversationEntry, PersistenceError> {
    let Some(context) = entry.screen_context.clone() else {
        return Ok(entry.clone());
    };
    let entry_date_is_selected = is_selected_audio_date(&entry.created_at, scope);
    let (context, _) = sanitize_user_screen_context(context, scope, entry_date_is_selected)?;
    let mut sanitized = entry.clone();
    sanitized.screen_context = (!context.is_empty()).then_some(context);
    Ok(sanitized)
}

fn sanitize_user_screen_context(
    mut context: UserScreenContext,
    scope: &ConversationLogDeletionScope,
    clear_hearing_context: bool,
) -> Result<(UserScreenContext, HashSet<String>), PersistenceError> {
    crate::hearing_context::validate_contexts(&context.hearing_context)
        .map_err(|error| PersistenceError::Invalid(error.to_owned()))?;
    let mut audio_ids_to_remove = scope.audio_ids.clone();
    for observation in &context.observations {
        match observation {
            ObservationRecord::Audio(audio)
                if scope
                    .audio_ids
                    .contains(crate::state::audio_segment_observation_id(&audio.id))
                    || is_selected_audio_date(&audio.created_at, scope) =>
            {
                audio_ids_to_remove
                    .insert(crate::state::audio_segment_observation_id(&audio.id).to_owned());
            }
            ObservationRecord::Audio(_) => {}
            ObservationRecord::Visual(visual) => {
                for segment in &visual.audio_segments {
                    if scope
                        .audio_ids
                        .contains(crate::state::audio_segment_observation_id(&segment.id))
                        || is_selected_audio_date(&segment.time, scope)
                    {
                        audio_ids_to_remove.insert(
                            crate::state::audio_segment_observation_id(&segment.id).to_owned(),
                        );
                    }
                }
            }
            ObservationRecord::NoChange(_) => {}
        }
    }
    let observations = context
        .observations
        .iter()
        .map(|observation| sanitize_observation_record(observation, scope))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let pending_audio = context
        .pending_audio
        .iter()
        .map(|audio| {
            if audio.id.is_empty()
                || crate::state::audio_segment_observation_id(&audio.id).is_empty()
            {
                return Err(PersistenceError::Invalid(
                    "pending audio の ID が不正です".to_owned(),
                ));
            }
            validate_audio_timestamps(audio)?;
            let remove = audio_ids_to_remove
                .contains(crate::state::audio_segment_observation_id(&audio.id))
                || is_selected_audio_date(&audio.created_at, scope);
            if remove {
                audio_ids_to_remove
                    .insert(crate::state::audio_segment_observation_id(&audio.id).to_owned());
            }
            Ok((!remove).then(|| audio.clone()))
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let pending_audio_ids = context
        .pending_audio_ids
        .iter()
        .map(|id| {
            if id.is_empty() || crate::state::audio_segment_observation_id(id).is_empty() {
                return Err(PersistenceError::Invalid(
                    "pending audio の ID が不正です".to_owned(),
                ));
            }
            Ok(
                (!audio_ids_to_remove.contains(crate::state::audio_segment_observation_id(id)))
                    .then(|| id.clone()),
            )
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let audio_context_removed = observations != context.observations
        || pending_audio.len() != context.pending_audio.len()
        || pending_audio_ids.len() != context.pending_audio_ids.len();
    context.observations = observations;
    context.pending_audio = pending_audio;
    context.pending_audio_ids = pending_audio_ids;
    if audio_context_removed || clear_hearing_context {
        context.hearing_context.clear();
    }
    Ok((context, audio_ids_to_remove))
}

fn remove_audio_from_record(
    record: ObservationRecord,
    date: NaiveDate,
    audio_ids: &HashSet<String>,
) -> Result<(Option<ObservationRecord>, Vec<String>), PersistenceError> {
    match record {
        ObservationRecord::Audio(audio) => {
            let observation_id = crate::state::audio_segment_observation_id(&audio.id);
            if audio_ids.contains(observation_id) || local_date_of(&audio.created_at) == Some(date)
            {
                Ok((None, vec![observation_id.to_owned()]))
            } else {
                Ok((Some(ObservationRecord::Audio(audio)), Vec::new()))
            }
        }
        ObservationRecord::Visual(mut visual) => {
            let mut removed_ids = Vec::new();
            visual.audio_segments.retain(|segment| {
                let observation_id = crate::state::audio_segment_observation_id(&segment.id);
                let remove = audio_ids.contains(observation_id)
                    || local_date_of(&segment.time) == Some(date);
                if remove {
                    removed_ids.push(observation_id.to_owned());
                }
                !remove
            });
            if removed_ids.is_empty() {
                return Ok((Some(ObservationRecord::Visual(visual)), removed_ids));
            }
            // outline は音声由来か画面由来かを区別する provenance を持たないため、
            // 対象区間に触れた Visual では削除本文の残留を避ける。frame/activity/changes
            // などの画面記録は保持し、Visual 全体は削除しない。
            visual.data.outline.clear();
            let remove_entire_record = visual.frame_count == 0 && visual.audio_segments.is_empty();
            Ok((
                (!remove_entire_record).then_some(ObservationRecord::Visual(visual)),
                removed_ids,
            ))
        }
        other => Ok((Some(other), Vec::new())),
    }
}

fn local_date_of(value: &str) -> Option<NaiveDate> {
    let timestamp = DateTime::parse_from_rfc3339(value)
        .ok()?
        .with_timezone(&Utc);
    NaiveDate::parse_from_str(&crate::config::local_date_at(timestamp), "%Y-%m-%d").ok()
}

fn parse_timestamp(value: &str, label: &str) -> Result<NaiveDate, PersistenceError> {
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map_err(|error| PersistenceError::Invalid(format!("{label} の時刻が不正です: {error}")))?;
    Ok(timestamp.with_timezone(&Utc).date_naive())
}

fn validate_transcript_record(record: &TranscriptRecord) -> Result<(), PersistenceError> {
    parse_timestamp(&record.time, "transcript")?;
    if record.audio_start_ms.is_some() != record.audio_end_ms.is_some() {
        return Err(PersistenceError::Invalid(
            "transcript の音声時刻は開始と終了がそろっている必要があります".to_owned(),
        ));
    }
    Ok(())
}

fn validate_observation_timestamps(record: &ObservationRecord) -> Result<(), PersistenceError> {
    if record.id().is_empty() {
        return Err(PersistenceError::Invalid(
            "observation の ID が空です".to_owned(),
        ));
    }
    parse_timestamp(record.created_at(), "observation")?;
    match record {
        ObservationRecord::Audio(audio) => {
            if crate::state::audio_segment_observation_id(&audio.id).is_empty() {
                return Err(PersistenceError::Invalid(
                    "audio の observation ID が空です".to_owned(),
                ));
            }
            validate_audio_timestamps(audio)?;
        }
        ObservationRecord::Visual(visual) => {
            parse_timestamp(&visual.window_start, "visual windowStart")?;
            parse_timestamp(&visual.window_end, "visual windowEnd")?;
            for segment in &visual.audio_segments {
                if crate::state::audio_segment_observation_id(&segment.id).is_empty() {
                    return Err(PersistenceError::Invalid(
                        "audio segment の observation ID が空です".to_owned(),
                    ));
                }
                parse_timestamp(&segment.time, "audio segment")?;
            }
        }
        ObservationRecord::NoChange(no_change) => {
            parse_timestamp(&no_change.window_start, "no-change windowStart")?;
            parse_timestamp(&no_change.window_end, "no-change windowEnd")?;
        }
    }
    Ok(())
}

fn validate_audio_timestamps(
    audio: &crate::state::AudioObservation,
) -> Result<(), PersistenceError> {
    parse_timestamp(&audio.created_at, "audio createdAt")?;
    parse_timestamp(&audio.window_start, "audio windowStart")?;
    parse_timestamp(&audio.window_end, "audio windowEnd")?;
    Ok(())
}

fn is_selected_audio_date(value: &str, scope: &ConversationLogDeletionScope) -> bool {
    local_date_of(value) == Some(scope.date)
}

pub(crate) fn sanitize_observation_value(
    value: &serde_json::Value,
    scope: &ConversationLogDeletionScope,
) -> Result<Option<serde_json::Value>, PersistenceError> {
    let record = parse_observation(value.clone(), DEFAULT_OBSERVATION_LIMITS).map_err(|error| {
        PersistenceError::Invalid(format!("observation payload が不正です: {error}"))
    })?;
    validate_observation_timestamps(&record)?;
    match record {
        ObservationRecord::Audio(audio)
            if scope
                .audio_ids
                .contains(crate::state::audio_segment_observation_id(&audio.id))
                || is_selected_audio_date(&audio.created_at, scope) =>
        {
            Ok(None)
        }
        ObservationRecord::Visual(mut visual) => {
            let before = visual.audio_segments.len();
            visual.audio_segments.retain(|segment| {
                let observation_id = crate::state::audio_segment_observation_id(&segment.id);
                !(scope.audio_ids.contains(observation_id)
                    || is_selected_audio_date(&segment.time, scope))
            });
            if visual.audio_segments.len() == before {
                Ok(Some(value.clone()))
            } else if visual.frame_count == 0 && visual.audio_segments.is_empty() {
                Ok(None)
            } else {
                // 音声との provenance がない派生 outline だけを落とし、画面記録は残す。
                visual.data.outline.clear();
                Ok(Some(serde_json::to_value(ObservationRecord::Visual(
                    visual,
                ))?))
            }
        }
        _ => Ok(Some(value.clone())),
    }
}

pub(crate) fn sanitize_observation_record(
    record: &ObservationRecord,
    scope: &ConversationLogDeletionScope,
) -> Result<Option<ObservationRecord>, PersistenceError> {
    let value = serde_json::to_value(record)?;
    let Some(value) = sanitize_observation_value(&value, scope)? else {
        return Ok(None);
    };
    parse_observation(value, DEFAULT_OBSERVATION_LIMITS)
        .map(Some)
        .map_err(|error| {
            PersistenceError::Invalid(format!("sanitized observation が不正です: {error}"))
        })
}

pub(crate) fn sanitize_pending_user_message(
    input: &crate::companion_storage::PendingUserMessage,
    scope: &ConversationLogDeletionScope,
) -> Result<crate::companion_storage::PendingUserMessage, PersistenceError> {
    parse_timestamp(&input.created_at, "pending user input")?;
    crate::hearing_context::validate_contexts(&input.hearing_context)
        .map_err(|error| PersistenceError::Invalid(error.to_owned()))?;
    let observations = input
        .observations
        .iter()
        .map(|observation| sanitize_observation_record(observation, scope))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut removed_audio_ids = HashSet::new();
    let pending_audio: Vec<crate::state::AudioObservation> = input
        .pending_audio
        .iter()
        .map(|audio| {
            if audio.id.is_empty() {
                return Err(PersistenceError::Invalid(
                    "pending audio の ID が空です".to_owned(),
                ));
            }
            if crate::state::audio_segment_observation_id(&audio.id).is_empty() {
                return Err(PersistenceError::Invalid(
                    "pending audio の observation ID が空です".to_owned(),
                ));
            }
            validate_audio_timestamps(audio)?;
            let remove = scope
                .audio_ids
                .contains(crate::state::audio_segment_observation_id(&audio.id))
                || is_selected_audio_date(&audio.created_at, scope);
            if remove {
                removed_audio_ids
                    .insert(crate::state::audio_segment_observation_id(&audio.id).to_owned());
            }
            Ok((!remove).then(|| audio.clone()))
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?
        .into_iter()
        .flatten()
        .collect();
    let mut audio_ids_to_remove = scope.audio_ids.clone();
    audio_ids_to_remove.extend(removed_audio_ids);
    let pending_audio_ids = input
        .pending_audio_ids
        .iter()
        .map(|id| {
            if id.is_empty() {
                return Err(PersistenceError::Invalid(
                    "pending audio の ID が空です".to_owned(),
                ));
            }
            if crate::state::audio_segment_observation_id(id).is_empty() {
                return Err(PersistenceError::Invalid(
                    "pending audio の observation ID が空です".to_owned(),
                ));
            }
            Ok(
                (!audio_ids_to_remove.contains(crate::state::audio_segment_observation_id(id)))
                    .then(|| id.clone()),
            )
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let prepared_response = input.prepared_response.as_ref().map(|response| {
        let mut response = response.clone();
        response.audio_ids.retain(|id| {
            !audio_ids_to_remove.contains(crate::state::audio_segment_observation_id(id))
        });
        response
    });
    let audio_context_removed = observations != input.observations
        || pending_audio.len() != input.pending_audio.len()
        || pending_audio_ids.len() != input.pending_audio_ids.len();
    let input_date_is_selected = is_selected_audio_date(&input.created_at, scope);
    let mut sanitized = input.clone();
    sanitized.observations = observations;
    sanitized.pending_audio = pending_audio;
    sanitized.pending_audio_ids = pending_audio_ids;
    sanitized.prepared_response = prepared_response;
    if audio_context_removed || input_date_is_selected {
        sanitized.hearing_context.clear();
    }
    Ok(sanitized)
}

fn value_id(value: &serde_json::Value) -> String {
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn compensate_deletion_failure(
    error: PersistenceError,
    applied: &[FileSnapshot],
    transcript_snapshot: Option<&FileSnapshot>,
) -> PersistenceError {
    let mut snapshots = applied.to_vec();
    if let Some(snapshot) = transcript_snapshot {
        snapshots.push(snapshot.clone());
    }
    match restore_file_snapshots(&snapshots) {
        Ok(()) => error,
        Err(rollback) => PersistenceError::Invalid(format!(
            "会話ログ削除に失敗し、変更の復元にも失敗しました: error={error}; rollback={rollback}"
        )),
    }
}

fn before_transcript_remove(path: &std::path::Path) -> Result<(), PersistenceError> {
    #[cfg(not(test))]
    {
        let _ = path;
        Ok(())
    }
}

fn after_transcript_remove(path: &std::path::Path) -> Result<(), PersistenceError> {
    #[cfg(not(test))]
    {
        let _ = path;
        Ok(())
    }
}

fn transcript_path(paths: &ConfigPaths, date: NaiveDate) -> std::path::PathBuf {
    paths
        .transcripts
        .join(format!("{}.jsonl", date.format("%Y-%m-%d")))
}

#[derive(Default)]
struct SpeakerInfo {
    segment_id: Option<String>,
    tag: Option<String>,
    registry_id: Option<String>,
    status: Option<SpeakerIdentificationStatus>,
    details: Vec<crate::speaker_decision::SpeakerDecisionDetails>,
    period_details: HashMap<(u64, u64), Vec<crate::speaker_decision::SpeakerDecisionDetails>>,
}

fn speaker_index(
    paths: &ConfigPaths,
    date: NaiveDate,
) -> Result<HashMap<String, SpeakerInfo>, PersistenceError> {
    let path = paths
        .observations
        .join(format!("{}.jsonl", date.format("%Y-%m-%d")));
    let mut index = HashMap::new();
    for value in JsonlStore::new(path).read::<serde_json::Value>()? {
        if let Ok(ObservationRecord::Audio(record)) =
            parse_observation(value, DEFAULT_OBSERVATION_LIMITS)
        {
            index.insert(
                record.id,
                SpeakerInfo {
                    segment_id: record.segment_id,
                    tag: record.speaker_id,
                    registry_id: record.speaker_registry_id,
                    status: record.speaker_status,
                    details: record.speaker_decision_details,
                    period_details: record
                        .speaker_segments
                        .into_iter()
                        .map(|segment| {
                            ((segment.start_ms, segment.end_ms), segment.decision_details)
                        })
                        .collect(),
                },
            );
        }
    }
    Ok(index)
}
