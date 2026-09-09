use crate::companion_storage::{CompanionStorage, PendingInput};
use crate::config::ConfigPaths;
use crate::mailbox::archive_mailbox_kind;
use crate::outbox::DurableOutbox;
use crate::persistence::{
    atomic_write_json, set_private_directory_mode, PersistenceError, SiblingLock,
};
use crate::provider::ProviderSession;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[path = "conversation_archive_pending.rs"]
mod conversation_archive_pending;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConversationGeneration {
    schema_version: u8,
    generation: u64,
    /// 旧 snapshot では未保存。未指定時は generation を選択中とみなす。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    started_at: BTreeMap<u64, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    summaries: BTreeMap<u64, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sessions: BTreeMap<u64, ProviderSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationGenerationSummary {
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
    pub entry_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetIntent {
    schema_version: u8,
    archive_key: String,
    target_generation: u64,
    target_user_operation_generation: u64,
    retention_days: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    carry_pending_inputs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<ResetOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "archive")]
    legacy_archive: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ResetOperation {
    GenerationOnly,
    Archive,
}

impl ResetIntent {
    fn operation(&self) -> ResetOperation {
        self.operation.unwrap_or(match self.legacy_archive {
            Some(false) => ResetOperation::GenerationOnly,
            Some(true) | None => ResetOperation::Archive,
        })
    }
}

pub fn current_conversation_generation(paths: &ConfigPaths) -> Result<u64, PersistenceError> {
    with_read_generation_state(paths, |state| {
        Ok(state.selected_generation.unwrap_or(state.generation))
    })
}

pub fn latest_conversation_generation(paths: &ConfigPaths) -> Result<u64, PersistenceError> {
    with_read_generation_state(paths, |state| Ok(state.generation))
}

pub fn list_conversation_generations(
    paths: &ConfigPaths,
) -> Result<Vec<ConversationGenerationSummary>, PersistenceError> {
    fs::create_dir_all(&paths.state)?;
    let _lock = SiblingLock::acquire(&generation_lock_path(paths))?;
    let state = read_generation_state(paths)?;
    let entries = crate::companion_storage::load_all_conversation_with_generations(paths)?;
    let mut entries_by_generation = BTreeMap::<u64, Vec<crate::state::ConversationEntry>>::new();
    for (generation, entry) in entries {
        entries_by_generation
            .entry(generation)
            .or_default()
            .push(entry);
    }

    let mut generations = BTreeSet::from([0, state.generation]);
    if let Some(selected) = state.selected_generation {
        generations.insert(selected);
    }
    generations.extend(state.started_at.keys().copied());
    generations.extend(state.summaries.keys().copied());
    generations.extend(state.sessions.keys().copied());
    generations.extend(entries_by_generation.keys().copied());

    generations
        .into_iter()
        .map(|generation| {
            let entries = entries_by_generation
                .remove(&generation)
                .unwrap_or_default();
            let first = entries.first();
            let started_at = state
                .started_at
                .get(&generation)
                .cloned()
                .or_else(|| first.map(|entry| entry.created_at.clone()));
            Ok(ConversationGenerationSummary {
                generation,
                started_at,
                first_message: first.map(|entry| truncate_first_message(&entry.message)),
                entry_count: entries.len(),
            })
        })
        .collect()
}

pub fn select_conversation_generation(
    paths: &ConfigPaths,
    generation: u64,
) -> Result<(), PersistenceError> {
    fs::create_dir_all(&paths.state)?;
    let _lock = SiblingLock::acquire(&generation_lock_path(paths))?;
    let mut state = read_generation_state(paths)?;
    if generation > state.generation {
        return Err(PersistenceError::Invalid(format!(
            "選択する会話世代が存在しません: {generation}"
        )));
    }
    let entries = crate::companion_storage::load_all_conversation_with_generations(paths)?;
    let known = generation == 0
        || generation == state.generation
        || state.started_at.contains_key(&generation)
        || state.summaries.contains_key(&generation)
        || state.sessions.contains_key(&generation)
        || entries.iter().any(|(value, _)| *value == generation);
    if !known {
        return Err(PersistenceError::Invalid(format!(
            "選択する会話世代が存在しません: {generation}"
        )));
    }
    state.selected_generation = Some(generation);
    write_generation_state(paths, &state)
}

pub(crate) fn load_conversation_summary(
    paths: &ConfigPaths,
    generation: u64,
) -> Result<Option<String>, PersistenceError> {
    with_read_generation_state(paths, |state| Ok(state.summaries.get(&generation).cloned()))
}

pub(crate) fn save_conversation_summary(
    paths: &ConfigPaths,
    generation: u64,
    summary: &str,
) -> Result<(), PersistenceError> {
    with_generation_state(paths, |state| {
        state.summaries.insert(generation, summary.to_owned());
        Ok(())
    })
}

pub(crate) fn load_conversation_session(
    paths: &ConfigPaths,
    generation: u64,
) -> Result<Option<ProviderSession>, PersistenceError> {
    with_read_generation_state(paths, |state| Ok(state.sessions.get(&generation).cloned()))
}

pub(crate) fn save_conversation_session(
    paths: &ConfigPaths,
    generation: u64,
    session: Option<&ProviderSession>,
) -> Result<(), PersistenceError> {
    with_generation_state(paths, |state| {
        match session {
            Some(session) => {
                state.sessions.insert(generation, session.clone());
            }
            None => {
                state.sessions.remove(&generation);
            }
        }
        Ok(())
    })
}

fn with_generation_state<R>(
    paths: &ConfigPaths,
    operation: impl FnOnce(&mut ConversationGeneration) -> Result<R, PersistenceError>,
) -> Result<R, PersistenceError> {
    fs::create_dir_all(&paths.state)?;
    let _lock = SiblingLock::acquire(&generation_lock_path(paths))?;
    let mut state = read_generation_state(paths)?;
    let result = operation(&mut state)?;
    validate_generation_state(&state)?;
    write_generation_state(paths, &state)?;
    Ok(result)
}

fn with_read_generation_state<R>(
    paths: &ConfigPaths,
    operation: impl FnOnce(&ConversationGeneration) -> Result<R, PersistenceError>,
) -> Result<R, PersistenceError> {
    fs::create_dir_all(&paths.state)?;
    let _lock = SiblingLock::acquire(&generation_lock_path(paths))?;
    let state = read_generation_state(paths)?;
    operation(&state)
}

fn generation_lock_path(paths: &ConfigPaths) -> PathBuf {
    conversation_generation_lock_path(&paths.state)
}

pub fn conversation_generation_lock_path(state_directory: &Path) -> PathBuf {
    state_directory.join(".conversation-generation.lock")
}

fn read_generation_state(paths: &ConfigPaths) -> Result<ConversationGeneration, PersistenceError> {
    let state = match fs::read(&paths.conversation_generation) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ConversationGeneration {
            schema_version: 1,
            generation: 0,
            selected_generation: None,
            started_at: BTreeMap::new(),
            summaries: BTreeMap::new(),
            sessions: BTreeMap::new(),
        },
        Err(error) => return Err(error.into()),
    };
    validate_generation_state(&state)?;
    Ok(state)
}

fn validate_generation_state(state: &ConversationGeneration) -> Result<(), PersistenceError> {
    if state.schema_version != 1 {
        return Err(PersistenceError::Invalid(
            "conversation generation の schemaVersion が不正です".to_owned(),
        ));
    }
    if state
        .selected_generation
        .is_some_and(|generation| generation > state.generation)
        || state
            .started_at
            .keys()
            .chain(state.summaries.keys())
            .chain(state.sessions.keys())
            .any(|generation| *generation > state.generation)
    {
        return Err(PersistenceError::Invalid(
            "conversation generation の世代参照が不正です".to_owned(),
        ));
    }
    Ok(())
}

fn write_generation_state(
    paths: &ConfigPaths,
    state: &ConversationGeneration,
) -> Result<(), PersistenceError> {
    atomic_write_json(&paths.conversation_generation, state)?;
    sync_directory(&paths.state)
}

fn truncate_first_message(message: &str) -> String {
    message.chars().take(80).collect()
}

pub fn reset_conversation(
    paths: &ConfigPaths,
    now: DateTime<Utc>,
) -> Result<u64, PersistenceError> {
    run_reset_operation(paths, 0, now, ResetOperation::GenerationOnly)?;
    current_conversation_generation(paths)
}

pub fn archive_conversation(
    paths: &ConfigPaths,
    retention_days: u64,
    now: DateTime<Utc>,
) -> Result<PathBuf, PersistenceError> {
    let intent = run_reset_operation(paths, retention_days, now, ResetOperation::Archive)?;
    Ok(paths.archive.join(intent.archive_key))
}

pub fn archive_conversation_after_recovery<F>(
    paths: &ConfigPaths,
    retention_days: u64,
    now: DateTime<Utc>,
    recovery: F,
) -> Result<PathBuf, PersistenceError>
where
    F: FnOnce() -> Result<(), PersistenceError>,
{
    let intent = run_reset_operation_with_recovery(
        paths,
        retention_days,
        now,
        ResetOperation::Archive,
        true,
        recovery,
    )?;
    Ok(paths.archive.join(intent.archive_key))
}

fn run_reset_operation(
    paths: &ConfigPaths,
    retention_days: u64,
    now: DateTime<Utc>,
    operation: ResetOperation,
) -> Result<ResetIntent, PersistenceError> {
    run_reset_operation_with_recovery(paths, retention_days, now, operation, false, || Ok(()))
}

fn run_reset_operation_with_recovery<F>(
    paths: &ConfigPaths,
    retention_days: u64,
    now: DateTime<Utc>,
    operation: ResetOperation,
    carry_pending_inputs: bool,
    recovery: F,
) -> Result<ResetIntent, PersistenceError>
where
    F: FnOnce() -> Result<(), PersistenceError>,
{
    fs::create_dir_all(&paths.state)?;
    let _guard = SiblingLock::acquire(&paths.state.join(".conversation-reset.lock"))?;
    recovery()?;
    reconcile_locked(paths)?;
    let storage = CompanionStorage::from_paths(paths, retention_days);
    let intent = ResetIntent {
        schema_version: 1,
        archive_key: unique_archive_key(paths, archive_key(now)),
        target_generation: latest_conversation_generation(paths)?.saturating_add(1),
        target_user_operation_generation: storage
            .load_cursor()?
            .user_operation_generation
            .saturating_add(1),
        retention_days,
        started_at: Some(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        carry_pending_inputs,
        operation: Some(operation),
        legacy_archive: None,
    };
    atomic_write_json(&paths.conversation_reset_intent, &intent)?;
    sync_directory(&paths.state)?;
    execute_intent(paths, &intent)?;
    Ok(intent)
}

pub fn reconcile_conversation_reset(paths: &ConfigPaths) -> Result<(), PersistenceError> {
    fs::create_dir_all(&paths.state)?;
    let _guard = SiblingLock::acquire(&paths.state.join(".conversation-reset.lock"))?;
    reconcile_locked(paths).map(|_| ())
}

fn reconcile_locked(paths: &ConfigPaths) -> Result<(), PersistenceError> {
    let bytes = match fs::read(&paths.conversation_reset_intent) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let intent: ResetIntent = serde_json::from_slice(&bytes)?;
    if intent.schema_version != 1 {
        return Err(PersistenceError::Invalid(
            "conversation reset intent の schemaVersion が不正です".to_owned(),
        ));
    }
    execute_intent(paths, &intent)
}

fn execute_intent(paths: &ConfigPaths, intent: &ResetIntent) -> Result<(), PersistenceError> {
    let archive = paths.archive.join(&intent.archive_key);
    let storage = CompanionStorage::from_paths(paths, intent.retention_days);
    let carried_pending_inputs = if intent.carry_pending_inputs {
        storage.load_cursor()?.pending_inputs
    } else {
        Vec::new()
    };
    fs::create_dir_all(&paths.conversation)?;
    fs::create_dir_all(&paths.attachments)?;
    if intent.operation() == ResetOperation::Archive {
        let archived_conversation = archive.join("conversation");
        let archived_attachments = archive.join("attachments");
        for directory in [&archive, &archived_conversation, &archived_attachments] {
            fs::create_dir_all(directory)?;
            set_private_directory_mode(directory)?;
        }
        move_entries(&paths.conversation, &archived_conversation)?;
        move_entries(&paths.attachments, &archived_attachments)?;
        maybe_fail_after_archive(paths)?;
        DurableOutbox::new(paths.outbox.clone())
            .archive_kind("remark", &archive.join("outbox"))
            .map_err(|error| PersistenceError::Invalid(error.to_string()))?;
        archive_mailbox_kind(&paths.mailbox, "remark", &archive.join("mailbox"))
            .map_err(|error| PersistenceError::Invalid(error.to_string()))?;
    }
    let pending_inputs = if intent.carry_pending_inputs {
        conversation_archive_pending::rebase_pending_input_attachments(
            paths,
            &archive,
            carried_pending_inputs,
            Utc::now(),
            intent.retention_days,
        )?
    } else {
        carried_pending_inputs
    };
    storage.update_cursor(|cursor| {
        *cursor = crate::companion_storage::CursorSnapshot {
            emotion_updates_enabled: cursor.emotion_updates_enabled,
            companion_emotions: cursor.companion_emotions,
            emotion_epoch: cursor.emotion_epoch,
            ..Default::default()
        };
        cursor.user_operation_generation = intent.target_user_operation_generation;
        if intent.carry_pending_inputs {
            for pending in pending_inputs {
                let PendingInput::UserMessage(mut input) = pending;
                let next_user_seq = cursor.next_user_seq.checked_add(1).ok_or_else(|| {
                    PersistenceError::Invalid("user seq が上限に達しました".to_owned())
                })?;
                cursor.next_user_seq = next_user_seq;
                cursor.user_epoch = cursor.user_epoch.checked_add(1).ok_or_else(|| {
                    PersistenceError::Invalid("user epoch が上限に達しました".to_owned())
                })?;
                input.conversation_generation = intent.target_generation;
                input.user_seq = next_user_seq;
                cursor.pending_inputs.push(PendingInput::UserMessage(input));
            }
        }
        Ok(())
    })?;
    let started_at = intent
        .started_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    with_generation_state(paths, |state| {
        state.generation = state.generation.max(intent.target_generation);
        state.selected_generation = Some(intent.target_generation);
        state
            .started_at
            .entry(intent.target_generation)
            .or_insert(started_at);
        Ok(())
    })?;
    if intent.operation() == ResetOperation::Archive {
        sync_tree_directories(&archive)?;
        sync_directory(&paths.archive)?;
    }
    sync_directory(&paths.conversation)?;
    sync_directory(&paths.attachments)?;
    sync_directory(&paths.state)?;
    match fs::remove_file(&paths.conversation_reset_intent) {
        Ok(()) => sync_directory(&paths.state)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn move_entries(source: &Path, destination: &Path) -> Result<(), PersistenceError> {
    fs::create_dir_all(source)?;
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()? {
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            continue;
        };
        if name_text.starts_with('.') {
            continue;
        }
        let target = destination.join(&name);
        if target.exists() {
            return Err(PersistenceError::Invalid(format!(
                "会話 archive に同名の項目があります: {name_text}"
            )));
        }
        fs::rename(entry.path(), target)?;
    }
    sync_directory(source)?;
    sync_directory(destination)?;
    Ok(())
}

fn unique_archive_key(paths: &ConfigPaths, base: String) -> String {
    if !paths.archive.join(&base).exists() {
        return base;
    }
    let mut suffix = 1_u64;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !paths.archive.join(&candidate).exists() {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn archive_key(now: DateTime<Utc>) -> String {
    now.format("%Y%m%dT%H%M%S%.3fZ").to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn sync_directory(path: &Path) -> Result<(), PersistenceError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn sync_tree_directories(path: &Path) -> Result<(), PersistenceError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree_directories(&entry.path())?;
        }
    }
    sync_directory(path)
}

#[cfg(not(test))]
fn maybe_fail_after_archive(_paths: &ConfigPaths) -> Result<(), PersistenceError> {
    Ok(())
}

