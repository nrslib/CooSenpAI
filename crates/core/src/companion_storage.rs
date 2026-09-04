pub use crate::companion_cursor::{
    ActiveTurnCommit, CursorSnapshot, ObservationAttempt, ObservationConsumption,
    PendingAttachmentFailure, PendingDelivery, PendingInput, PendingUserMessage,
    PreparedUserResponse, TurnCommitKind, TurnCommitPhase, TurnCommitRecoveryAttempt,
    UserDispatchLease,
};
use crate::config::ConfigPaths;
use crate::frame_buffer::FrameBuffer;
use crate::logging::FileLogger;
use crate::outbox::DurableOutbox;
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use crate::ports::RuntimeLogger;
use crate::state::{
    parse_observation, ConversationEntry, ConversationRole, ObservationRecord, PendingFrameContext,
    DEFAULT_OBSERVATION_LIMITS,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

const MAX_CURSOR_IDS: usize = 500;
const MAX_CONVERSATION_ENTRIES: usize = 200;
const CURSOR_SCHEMA_VERSION: u64 = 2;
pub(super) const OVERSIZED_DELIVERY_REASON: &str = "pending-delivery-payload-too-large";

#[path = "companion_storage_quarantine.rs"]
mod quarantine;
use quarantine::delivery_quarantine_record;
#[path = "companion_storage_attachments.rs"]
mod attachments;
#[path = "companion_storage_usage.rs"]
mod usage_recovery;

#[derive(Debug, Clone)]
pub struct CompanionStorage {
    pub state_directory: PathBuf,
    pub observation_directory: PathBuf,
    pub mailbox_directory: PathBuf,
    pub outbox_directory: PathBuf,
    pub usage_path: PathBuf,
    pub watch_stagnation_path: PathBuf,
    pub cursor_path: PathBuf,
    pub pending_quarantine_path: PathBuf,
    pub pending_delivery_quarantine_path: PathBuf,
    pub turn_commit_quarantine_path: PathBuf,
    pub log_path: PathBuf,
    pub conversation_directory: PathBuf,
    pub attachments_directory: PathBuf,
    frame_buffer: FrameBuffer,
    pub retention_days: u64,
}

impl CompanionStorage {
    pub fn from_paths(paths: &ConfigPaths, retention_days: u64) -> Self {
        Self {
            state_directory: paths.state.clone(),
            observation_directory: paths.observations.clone(),
            mailbox_directory: paths.mailbox.clone(),
            outbox_directory: paths.outbox.clone(),
            usage_path: paths.companion_usage.clone(),
            watch_stagnation_path: paths.watch_stagnation.clone(),
            cursor_path: paths.observation_cursor.clone(),
            pending_quarantine_path: paths
                .state
                .join("failed/companion-pending-observations.jsonl"),
            pending_delivery_quarantine_path: paths
                .state
                .join("failed/companion-pending-deliveries.jsonl"),
            turn_commit_quarantine_path: paths
                .state
                .join("failed/companion-active-turn-commits.jsonl"),
            log_path: paths.log.clone(),
            conversation_directory: paths.conversation.clone(),
            attachments_directory: paths.attachments.clone(),
            frame_buffer: FrameBuffer::new(paths.frame_buffer.clone()),
            retention_days,
        }
    }

    pub fn outbox(&self) -> DurableOutbox {
        DurableOutbox::new(self.outbox_directory.clone()).with_log_path(self.log_path.clone())
    }

    pub(crate) fn record_stagnation_reaction_at(&self, reacted_at: chrono::DateTime<chrono::Utc>) {
        let result =
            crate::watch_coordinator::WatchStagnationStore::new(self.watch_stagnation_path.clone())
                .record_reaction(reacted_at);
        if let Err(error) = result {
            if let Ok(logger) = FileLogger::new(self.log_path.clone()) {
                let _ = logger.write(
                    "WARN",
                    &format!(
                        "停滞エピソードの反応を保存できませんでした: error-type=persistence ({error})"
                    ),
                );
            }
        }
    }

    pub fn conversation_generation(&self) -> Result<u64, PersistenceError> {
        let paths = ConfigPaths::from_root(
            self.state_directory
                .parent()
                .ok_or_else(|| PersistenceError::Invalid("state directory が不正です".to_owned()))?
                .to_path_buf(),
        );
        crate::conversation_archive::current_conversation_generation(&paths)
    }

    pub fn load_conversation(&self) -> Result<Vec<ConversationEntry>, PersistenceError> {
        load_recent_conversation(
            &self.conversation_directory,
            MAX_CONVERSATION_ENTRIES,
            self.conversation_generation()?,
        )
    }

    pub fn load_recent_observations(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<ObservationRecord>, PersistenceError> {
        crate::recent_observations::read_recent_observations(&self.observation_directory, now)
    }

    pub(crate) fn observation_frame_paths(
        &self,
        observations: &[ObservationRecord],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<HashMap<String, Vec<PathBuf>>, PersistenceError> {
        let frame_paths = self.frame_buffer.paths_for_ids(
            observations
                .iter()
                .flat_map(ObservationRecord::source_frame_ids)
                .map(String::as_str),
            now,
        )?;
        Ok(observations
            .iter()
            .filter_map(|observation| {
                let paths = observation
                    .source_frame_ids()
                    .iter()
                    .filter_map(|frame_id| frame_paths.get(frame_id).cloned())
                    .collect::<Vec<_>>();
                (!paths.is_empty()).then(|| (observation.id().to_owned(), paths))
            })
            .collect())
    }

    pub(crate) fn load_all_conversation(&self) -> Result<Vec<ConversationEntry>, PersistenceError> {
        load_recent_conversation(
            &self.conversation_directory,
            usize::MAX,
            self.conversation_generation()?,
        )
    }

    pub(crate) fn completed_user_response(
        &self,
        input_id: &str,
    ) -> Result<Option<ConversationEntry>, PersistenceError> {
        Ok(self
            .load_all_conversation()?
            .into_iter()
            .rev()
            .find(|entry| {
                entry.role == ConversationRole::Companion
                    && entry.caused_by_ids.iter().any(|cause| cause == input_id)
            }))
    }

    pub fn append_conversation(&self, entry: &ConversationEntry) -> Result<(), PersistenceError> {
        self.append_conversation_at(entry, chrono::Utc::now())
    }

    pub fn append_conversation_at(
        &self,
        entry: &ConversationEntry,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), PersistenceError> {
        validate_conversation_entry(entry)?;
        let date = conversation_local_date(entry)?;
        let path = self.conversation_directory.join(format!("{date}.jsonl"));
        let value = conversation_storage_value(entry, self.conversation_generation()?)?;
        crate::persistence::JsonlStore::new(path).append(&value)?;
        crate::persistence::prune_daily_jsonl_at(
            &self.conversation_directory,
            self.retention_days,
            u64::MAX,
            now,
        )?;
        Ok(())
    }

    pub fn append_conversation_once_at(
        &self,
        entry: &ConversationEntry,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, PersistenceError> {
        self.append_conversation_once_at_with_pruning(entry, now, true)
    }

    pub(crate) fn append_conversation_once_at_without_pruning(
        &self,
        entry: &ConversationEntry,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, PersistenceError> {
        self.append_conversation_once_at_with_pruning(entry, now, false)
    }

    fn append_conversation_once_at_with_pruning(
        &self,
        entry: &ConversationEntry,
        now: chrono::DateTime<chrono::Utc>,
        prune_conversation: bool,
    ) -> Result<bool, PersistenceError> {
        validate_conversation_entry(entry)?;
        let date = conversation_local_date(entry)?;
        let path = self.conversation_directory.join(format!("{date}.jsonl"));
        let value = conversation_storage_value(entry, self.conversation_generation()?)?;
        let appended = crate::persistence::JsonlStore::new(path).append_unique(
            &value,
            |existing: &Value| {
                existing.get("id").and_then(Value::as_str) == Some(entry.id.as_str())
            },
        )?;
        if prune_conversation {
            crate::persistence::prune_daily_jsonl_at(
                &self.conversation_directory,
                self.retention_days,
                u64::MAX,
                now,
            )?;
        }
        Ok(appended)
    }

    pub fn load_summary(&self) -> Result<Option<String>, PersistenceError> {
        let path = self.conversation_directory.join("summary.json");
        let lock_path = path.with_file_name(".summary.lock");
        let _lock = SiblingLock::acquire(&lock_path)?;
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let value: Value = serde_json::from_slice(&bytes)?;
        let object = value.as_object().ok_or_else(|| {
            PersistenceError::Invalid("conversation summary が object ではありません".to_owned())
        })?;
        if object.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
            return Err(PersistenceError::Invalid(
                "conversation summary の schemaVersion が不正です".to_owned(),
            ));
        }
        let summary = object
            .get("text")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| PersistenceError::Invalid("conversation summary が空です".to_owned()))?;
        let summary_generation = object
            .get("conversationGeneration")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if summary_generation != self.conversation_generation()? {
            return Ok(None);
        }
        Ok(Some(summary.to_owned()))
    }

    pub fn save_summary(&self, summary: &str) -> Result<(), PersistenceError> {
        if summary.trim().is_empty() {
            return Err(PersistenceError::Invalid(
                "conversation summary が空です".to_owned(),
            ));
        }
        let path = self.conversation_directory.join("summary.json");
        let lock_path = path.with_file_name(".summary.lock");
        let _lock = SiblingLock::acquire(&lock_path)?;
        let generation = self.conversation_generation()?;
        atomic_write_json(
            &path,
            &serde_json::json!({
                "schemaVersion": 1,
                "text": summary,
                "conversationGeneration": generation,
            }),
        )?;
        Ok(())
    }

    pub fn load_cursor(&self) -> Result<CursorSnapshot, PersistenceError> {
        read_cursor(
            &self.cursor_path,
            &self.pending_quarantine_path,
            &self.pending_delivery_quarantine_path,
            &self.turn_commit_quarantine_path,
            &self.log_path,
        )
    }

    pub fn update_cursor<R>(
        &self,
        update: impl FnOnce(&mut CursorSnapshot) -> Result<R, PersistenceError>,
    ) -> Result<R, PersistenceError> {
        let lock = cursor_lock_path(&self.cursor_path);
        let _guard = SiblingLock::acquire(&lock)?;
        let mut cursor = read_cursor_locked(
            &self.cursor_path,
            &self.pending_quarantine_path,
            &self.pending_delivery_quarantine_path,
            &self.turn_commit_quarantine_path,
            &self.log_path,
        )?;
        let result = update(&mut cursor)?;
        #[cfg(test)]
        failpoints::before_cursor_write(&self.cursor_path)?;
        write_cursor_locked(&self.cursor_path, &cursor)?;
        Ok(result)
    }

    /// 永続ユーザーキューへ入力を一度だけ受理し、採番と横取り世代更新を同じ
    /// cursor トランザクションで行う。
    pub(crate) fn enqueue_user_input(
        &self,
        mut input: PendingUserMessage,
    ) -> Result<PendingUserMessage, PersistenceError> {
        self.update_cursor(|cursor| {
            if let Some(existing) = cursor
                .pending_inputs
                .iter()
                .find(|pending| pending.id() == input.id)
            {
                let PendingInput::UserMessage(existing) = existing;
                return Ok(existing.clone());
            }
            if cursor.cancelled_input_ids.iter().any(|id| id == &input.id) {
                return Err(PersistenceError::Invalid(
                    "取り消された user input を再受理できません".to_owned(),
                ));
            }
            let next = cursor.next_user_seq.checked_add(1).ok_or_else(|| {
                PersistenceError::Invalid("user seq が上限に達しました".to_owned())
            })?;
            cursor.next_user_seq = next;
            cursor.user_epoch = cursor.user_epoch.checked_add(1).ok_or_else(|| {
                PersistenceError::Invalid("user epoch が上限に達しました".to_owned())
            })?;
            input.user_seq = next;
            cursor
                .pending_inputs
                .push(PendingInput::UserMessage(input.clone()));
            Ok(input)
        })
    }

    pub(crate) fn lease_user_inputs(
        &self,
        max_inputs: usize,
    ) -> Result<(UserDispatchLease, Vec<PendingUserMessage>), PersistenceError> {
        self.update_cursor(|cursor| {
            if let Some(lease) = cursor.user_dispatch.clone() {
                let inputs = lease_inputs(cursor, &lease)?;
                return Ok((lease, inputs));
            }
            let mut inputs = cursor
                .pending_inputs
                .iter()
                .filter_map(|pending| match pending {
                    PendingInput::UserMessage(input) if !input.attachment_is_terminal() => {
                        Some(input.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            inputs.sort_by_key(|input| input.user_seq);
            inputs.truncate(max_inputs.max(1));
            if inputs.is_empty() {
                return Ok((
                    UserDispatchLease {
                        dispatch_seq: 0,
                        input_ids: Vec::new(),
                    },
                    inputs,
                ));
            }
            let dispatch_seq = cursor.next_dispatch_seq.checked_add(1).ok_or_else(|| {
                PersistenceError::Invalid("dispatch seq が上限に達しました".to_owned())
            })?;
            cursor.next_dispatch_seq = dispatch_seq;
            let lease = UserDispatchLease {
                dispatch_seq,
                input_ids: inputs.iter().map(|input| input.id.clone()).collect(),
            };
            cursor.user_dispatch = Some(lease.clone());
            Ok((lease, inputs))
        })
    }

    pub(crate) fn extend_user_dispatch(
        &self,
        dispatch_seq: u64,
        input_ids: &[String],
    ) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            let Some(lease) = cursor.user_dispatch.as_mut() else {
                return Err(PersistenceError::Invalid(
                    "user dispatch lease がありません".to_owned(),
                ));
            };
            if lease.dispatch_seq != dispatch_seq {
                return Err(PersistenceError::Invalid(
                    "user dispatch lease が一致しません".to_owned(),
                ));
            }
            for id in input_ids {
                if !lease.input_ids.contains(id) {
                    if !cursor.pending_inputs.iter().any(|input| input.id() == id) {
                        return Err(PersistenceError::Invalid(
                            "追加する user input が cursor にありません".to_owned(),
                        ));
                    }
                    lease.input_ids.push(id.clone());
                }
            }
            Ok(())
        })
    }

    pub fn pending_observations(&self) -> Result<Vec<ObservationRecord>, PersistenceError> {
        Ok(self.load_cursor()?.pending)
    }

    pub fn register_pending_frame_context(
        &self,
        context: PendingFrameContext,
    ) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            if cursor
                .consumed_frame_context_ids
                .iter()
                .any(|id| id == &context.id)
            {
                return Ok(());
            }
            if let Some(existing) = cursor
                .pending_frame_contexts
                .iter()
                .find(|existing| existing.id == context.id)
            {
                if existing != &context {
                    return Err(PersistenceError::Invalid(
                        "処理待ち画面の ID が重複しています".to_owned(),
                    ));
                }
                return Ok(());
            }
            cursor.pending_frame_contexts.push(context);
            if cursor.pending_frame_contexts.len() > 100 {
                cursor
                    .pending_frame_contexts
                    .drain(..cursor.pending_frame_contexts.len() - 100);
            }
            Ok(())
        })
    }

    pub(crate) fn observation_claimed_by_user(
        &self,
        observation: &ObservationRecord,
    ) -> Result<bool, PersistenceError> {
        self.update_cursor(|cursor| {
            let source_frame_ids = observation.source_frame_ids();
            let consumed = cursor.ids.iter().any(|id| id == observation.id())
                || all_source_frames_claimed(source_frame_ids, |source_id| {
                    cursor
                        .consumed_frame_context_ids
                        .iter()
                        .any(|id| id == source_id)
                });
            cursor
                .pending_frame_contexts
                .retain(|context| !source_frame_ids.iter().any(|id| id == &context.id));
            Ok(consumed)
        })
    }

    pub(crate) fn consume_user_observation_context(
        &self,
        input_ids: &[String],
        turn_id: &str,
    ) -> Result<Vec<String>, PersistenceError> {
        self.update_cursor(|cursor| {
            let before = cursor
                .pending
                .iter()
                .map(|observation| observation.id().to_owned())
                .collect::<HashSet<_>>();
            let inputs = cursor
                .pending_inputs
                .iter()
                .filter(|input| input_ids.iter().any(|id| id == input.id()))
                .cloned()
                .collect::<Vec<_>>();
            for input in inputs {
                let PendingInput::UserMessage(input) = input;
                claim_input_context_for_turn(cursor, &input, turn_id, "user-context");
            }
            let after = cursor
                .pending
                .iter()
                .map(|observation| observation.id().to_owned())
                .collect::<HashSet<_>>();
            Ok(before.difference(&after).cloned().collect())
        })
    }

    pub(crate) fn reserve_proactive_commit(
        &self,
        expected_user_epoch: u64,
        turn_id: &str,
        target_ids: &[String],
    ) -> Result<bool, PersistenceError> {
        self.update_cursor(|cursor| {
            if cursor.user_epoch != expected_user_epoch
                || !runnable_inputs(&cursor.pending_inputs).is_empty()
            {
                return Ok(false);
            }
            if cursor.user_dispatch.is_some() {
                return Ok(false);
            }
            if cursor.active_turn_commit.is_some() {
                return Ok(false);
            }
            cursor.active_turn_commit = Some(ActiveTurnCommit {
                turn_id: turn_id.to_owned(),
                kind: TurnCommitKind::Proactive,
                phase: TurnCommitPhase::Reserved,
                target_ids: target_ids.to_vec(),
                dispatch_seq: None,
                expected_user_epoch: Some(expected_user_epoch),
            });
            Ok(true)
        })
    }

    pub(crate) fn mark_turn_commit_persisting(
        &self,
        turn_id: &str,
    ) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            let Some(commit) = cursor.active_turn_commit.as_mut() else {
                return Err(PersistenceError::Invalid(
                    "active turn commit がありません".to_owned(),
                ));
            };
            if commit.turn_id != turn_id {
                return Err(PersistenceError::Invalid(
                    "active turn commit が一致しません".to_owned(),
                ));
            }
            commit.phase = TurnCommitPhase::Persisting;
            Ok(())
        })
    }

    pub(crate) fn finalize_proactive_commit(
        &self,
        expected_user_epoch: u64,
        turn_id: &str,
    ) -> Result<bool, PersistenceError> {
        self.update_cursor(|cursor| {
            if cursor.user_epoch != expected_user_epoch
                || cursor.active_turn_commit.as_ref().is_none_or(|commit| {
                    commit.turn_id != turn_id || commit.kind != TurnCommitKind::Proactive
                })
                || !runnable_inputs(&cursor.pending_inputs).is_empty()
            {
                return Ok(false);
            }
            cursor.active_turn_commit = None;
            Ok(true)
        })
    }

    pub(crate) fn clear_active_turn_commit(&self, turn_id: &str) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            if cursor
                .active_turn_commit
                .as_ref()
                .is_some_and(|commit| commit.turn_id == turn_id)
            {
                cursor.active_turn_commit = None;
            }
            Ok(())
        })
    }

    pub(crate) fn finalize_user_turn_commit(
        &self,
        turn_id: &str,
        dispatch_seq: u64,
        input_ids: &[String],
    ) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            let Some(commit) = cursor.active_turn_commit.as_ref() else {
                return Err(PersistenceError::Invalid(
                    "active user turn commit がありません".to_owned(),
                ));
            };
            if commit.turn_id != turn_id
                || commit.kind != TurnCommitKind::User
                || commit.dispatch_seq != Some(dispatch_seq)
                || commit.target_ids != input_ids
            {
                return Err(PersistenceError::Invalid(
                    "active user turn commit が一致しません".to_owned(),
                ));
            }
            if let Some(lease) = cursor.user_dispatch.as_ref() {
                if lease.dispatch_seq != dispatch_seq || lease.input_ids != input_ids {
                    return Err(PersistenceError::Invalid(
                        "user dispatch lease が active user turn commit と一致しません".to_owned(),
                    ));
                }
            }
            cursor
                .pending_inputs
                .retain(|input| !input_ids.iter().any(|id| input.id() == id));
            cursor.user_dispatch = None;
            cursor.active_turn_commit = None;
            Ok(())
        })
    }

    pub(crate) fn prepare_turn_commit_recovery(
        &self,
    ) -> Result<Option<ActiveTurnCommit>, PersistenceError> {
        self.update_cursor(|cursor| {
            let Some(commit) = cursor.active_turn_commit.clone() else {
                return Ok(None);
            };
            if commit.phase == TurnCommitPhase::Reserved {
                if commit.kind == TurnCommitKind::User {
                    if let Some(lease) = cursor.user_dispatch.as_ref() {
                        if Some(lease.dispatch_seq) == commit.dispatch_seq {
                            cursor.user_dispatch = None;
                        }
                    }
                }
                cursor.active_turn_commit = None;
                return Ok(None);
            }
            Ok(Some(commit))
        })
    }

    pub(crate) fn release_turn_commit_recovery(
        &self,
        commit: &ActiveTurnCommit,
        reason: &str,
    ) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            if cursor.active_turn_commit.as_ref() != Some(commit) {
                return Ok(());
            }
            if commit.kind == TurnCommitKind::User {
                if let Some(lease) = cursor.user_dispatch.as_ref() {
                    if Some(lease.dispatch_seq) == commit.dispatch_seq {
                        cursor.user_dispatch = None;
                    }
                }
                discard_incomplete_user_preparation(&mut cursor.pending_inputs, &commit.target_ids);
            }
            cursor
                .turn_commit_recovery_attempts
                .push(TurnCommitRecoveryAttempt {
                    turn_id: commit.turn_id.clone(),
                    kind: commit.kind.clone(),
                    target_ids: commit.target_ids.clone(),
                    reason: reason.to_owned(),
                    recorded_at: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                });
            if cursor.turn_commit_recovery_attempts.len() > MAX_CURSOR_IDS {
                cursor
                    .turn_commit_recovery_attempts
                    .drain(..cursor.turn_commit_recovery_attempts.len() - MAX_CURSOR_IDS);
            }
            cursor.active_turn_commit = None;
            Ok(())
        })
    }

    pub(crate) fn quarantine_turn_commit_recovery(
        &self,
        commit: &ActiveTurnCommit,
        reason: &str,
    ) -> Result<(), PersistenceError> {
        let marker = serde_json::to_value(commit)?;
        let (quarantine_id, record) = turn_commit_quarantine_record(&marker, reason)?;
        crate::persistence::JsonlStore::new(self.turn_commit_quarantine_path.clone())
            .append_idempotent(&record, |existing: &Value| {
                existing.get("quarantineId").and_then(Value::as_str) == Some(quarantine_id.as_str())
            })?;
        self.release_turn_commit_recovery(commit, reason)
    }

    pub(crate) fn reserve_user_commit(
        &self,
        dispatch_seq: u64,
        input_ids: &[String],
        operation_generation: Option<u64>,
        turn_id: &str,
    ) -> Result<bool, PersistenceError> {
        self.update_cursor(|cursor| {
            if operation_generation
                .is_some_and(|generation| cursor.user_operation_generation != generation)
            {
                return Ok(false);
            }
            let Some(lease) = cursor.user_dispatch.as_ref() else {
                return Ok(false);
            };
            if lease.dispatch_seq != dispatch_seq || lease.input_ids != input_ids {
                return Ok(false);
            }
            if cursor.active_turn_commit.is_some() {
                return Ok(false);
            }
            cursor.active_turn_commit = Some(ActiveTurnCommit {
                turn_id: turn_id.to_owned(),
                kind: TurnCommitKind::User,
                phase: TurnCommitPhase::Reserved,
                target_ids: input_ids.to_vec(),
                dispatch_seq: Some(dispatch_seq),
                expected_user_epoch: None,
            });
            Ok(true)
        })
    }

    pub(crate) fn cancel_user_input_after_termination(
        &self,
        input_id: &str,
    ) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            cursor.pending_inputs.retain(|input| input.id() != input_id);
            if !cursor.cancelled_input_ids.iter().any(|id| id == input_id) {
                cursor.cancelled_input_ids.push(input_id.to_owned());
            }
            if let Some(lease) = cursor.user_dispatch.as_mut() {
                lease.input_ids.retain(|id| id != input_id);
                if lease.input_ids.is_empty() {
                    cursor.user_dispatch = None;
                }
            }
            if cursor.active_turn_commit.as_ref().is_some_and(|commit| {
                commit.kind == TurnCommitKind::User
                    && commit.target_ids.iter().any(|id| id == input_id)
            }) {
                cursor.active_turn_commit = None;
            }
            Ok(())
        })
    }
}

fn lease_inputs(
    cursor: &CursorSnapshot,
    lease: &UserDispatchLease,
) -> Result<Vec<PendingUserMessage>, PersistenceError> {
    let mut inputs = Vec::with_capacity(lease.input_ids.len());
    for id in &lease.input_ids {
        let Some(PendingInput::UserMessage(input)) =
            cursor.pending_inputs.iter().find(|input| input.id() == id)
        else {
            return Err(PersistenceError::Invalid(
                "user dispatch lease の入力がありません".to_owned(),
            ));
        };
        if input.attachment_is_terminal() {
            return Err(PersistenceError::Invalid(
                "terminal input が dispatch lease にあります".to_owned(),
            ));
        }
        inputs.push(input.clone());
    }
    Ok(inputs)
}

fn runnable_inputs(inputs: &[PendingInput]) -> Vec<&PendingInput> {
    inputs
        .iter()
        .filter(|input| match input {
            PendingInput::UserMessage(input) => !input.attachment_is_terminal(),
        })
        .collect()
}

fn claim_input_context_for_turn(
    cursor: &mut CursorSnapshot,
    input: &PendingUserMessage,
    turn_id: &str,
    reason: &str,
) {
    let mut observation_ids = input
        .observations
        .iter()
        .map(|observation| observation.id().to_owned())
        .collect::<Vec<_>>();
    let frame_ids = input
        .pending_frames
        .iter()
        .map(|frame| frame.id.clone())
        .collect::<Vec<_>>();
    let consumed_frame_ids = &cursor.consumed_frame_context_ids;
    for observation in &cursor.pending {
        if all_source_frames_claimed(observation.source_frame_ids(), |source_id| {
            frame_ids.iter().any(|id| id == source_id)
                || consumed_frame_ids.iter().any(|id| id == source_id)
        }) {
            observation_ids.push(observation.id().to_owned());
        }
    }
    observation_ids.sort();
    observation_ids.dedup();
    cursor
        .pending
        .retain(|observation| !observation_ids.iter().any(|id| id == observation.id()));
    for id in observation_ids {
        complete_cursor_observation(cursor, &id, turn_id, reason);
    }
    cursor
        .pending_frame_contexts
        .retain(|context| !frame_ids.contains(&context.id));
    for id in frame_ids {
        if !cursor.consumed_frame_context_ids.contains(&id) {
            cursor.consumed_frame_context_ids.push(id);
        }
    }
    if cursor.consumed_frame_context_ids.len() > MAX_CURSOR_IDS {
        cursor
            .consumed_frame_context_ids
            .drain(..cursor.consumed_frame_context_ids.len() - MAX_CURSOR_IDS);
    }
}

fn all_source_frames_claimed(
    source_frame_ids: &[String],
    mut is_claimed: impl FnMut(&str) -> bool,
) -> bool {
    !source_frame_ids.is_empty()
        && source_frame_ids
            .iter()
            .all(|source_id| is_claimed(source_id))
}

fn complete_cursor_observation(
    cursor: &mut CursorSnapshot,
    observation_id: &str,
    turn_id: &str,
    reason: &str,
) {
    cursor
        .pending
        .retain(|observation| observation.id() != observation_id);
    if !cursor.ids.iter().any(|id| id == observation_id) {
        cursor.ids.push(observation_id.to_owned());
    }
    if cursor.ids.len() > MAX_CURSOR_IDS {
        cursor.ids.drain(..cursor.ids.len() - MAX_CURSOR_IDS);
    }
    cursor
        .observation_attempts
        .retain(|attempt| attempt.observation_id != observation_id);
    if !cursor
        .observation_consumptions
        .iter()
        .any(|consumption| consumption.observation_id == observation_id)
    {
        cursor
            .observation_consumptions
            .push(ObservationConsumption {
                observation_id: observation_id.to_owned(),
                turn_id: turn_id.to_owned(),
                reason: reason.to_owned(),
            });
        if cursor.observation_consumptions.len() > MAX_CURSOR_IDS {
            cursor
                .observation_consumptions
                .drain(..cursor.observation_consumptions.len() - MAX_CURSOR_IDS);
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorDocument<'a> {
    schema_version: u64,
    user_operation_generation: u64,
    user_epoch: u64,
    next_user_seq: u64,
    next_dispatch_seq: u64,
    user_dispatch: &'a Option<UserDispatchLease>,
    active_turn_commit: &'a Option<ActiveTurnCommit>,
    ids: &'a [String],
    pending: &'a [ObservationRecord],
    failed: &'a [String],
    observation_attempts: &'a [ObservationAttempt],
    cancelled_input_ids: &'a [String],
    pending_inputs: &'a [PendingInput],
    pending_deliveries: &'a [PendingDelivery],
    pending_frame_contexts: &'a [PendingFrameContext],
    consumed_frame_context_ids: &'a [String],
    observation_consumptions: &'a [ObservationConsumption],
    turn_commit_recovery_attempts: &'a [TurnCommitRecoveryAttempt],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawCursorDocument {
    #[serde(default)]
    schema_version: Option<u64>,
    user_operation_generation: u64,
    #[serde(default)]
    user_epoch: u64,
    #[serde(default)]
    next_user_seq: u64,
    #[serde(default)]
    next_dispatch_seq: u64,
    #[serde(default)]
    user_dispatch: Option<UserDispatchLease>,
    #[serde(default)]
    active_turn_commit: Option<Value>,
    ids: Vec<String>,
    pending: Vec<Value>,
    failed: Vec<String>,
    #[serde(default)]
    observation_attempts: Vec<ObservationAttempt>,
    cancelled_input_ids: Vec<String>,
    pending_inputs: Vec<PendingInput>,
    pending_deliveries: Vec<PendingDelivery>,
    #[serde(default)]
    pending_frame_contexts: Vec<PendingFrameContext>,
    #[serde(default)]
    consumed_frame_context_ids: Vec<String>,
    #[serde(default)]
    observation_consumptions: Vec<ObservationConsumption>,
    #[serde(default)]
    turn_commit_recovery_attempts: Vec<TurnCommitRecoveryAttempt>,
}

fn read_cursor(
    path: &Path,
    quarantine_path: &Path,
    delivery_quarantine_path: &Path,
    turn_commit_quarantine_path: &Path,
    log_path: &Path,
) -> Result<CursorSnapshot, PersistenceError> {
    let lock = cursor_lock_path(path);
    let _guard = SiblingLock::acquire(&lock)?;
    read_cursor_locked(
        path,
        quarantine_path,
        delivery_quarantine_path,
        turn_commit_quarantine_path,
        log_path,
    )
}

fn read_cursor_locked(
    path: &Path,
    quarantine_path: &Path,
    delivery_quarantine_path: &Path,
    turn_commit_quarantine_path: &Path,
    log_path: &Path,
) -> Result<CursorSnapshot, PersistenceError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CursorSnapshot::default())
        }
        Err(error) => return Err(error.into()),
    };
    let raw: RawCursorDocument = serde_json::from_slice(&bytes)?;
    let RawCursorDocument {
        schema_version,
        user_operation_generation,
        mut user_epoch,
        next_user_seq: raw_next_user_seq,
        next_dispatch_seq,
        user_dispatch,
        active_turn_commit: raw_active_turn_commit,
        ids: raw_ids,
        pending: raw_pending,
        failed: raw_failed,
        observation_attempts,
        cancelled_input_ids: raw_cancelled_input_ids,
        pending_inputs,
        pending_deliveries: raw_pending_deliveries,
        pending_frame_contexts,
        consumed_frame_context_ids: raw_consumed_frame_context_ids,
        observation_consumptions,
        mut turn_commit_recovery_attempts,
    } = raw;
    let legacy_cursor = schema_version.unwrap_or(0) < CURSOR_SCHEMA_VERSION;
    let mut user_dispatch = user_dispatch;
    let mut invalid_active_turn_commit: Option<(Value, Option<ActiveTurnCommit>, String)> = None;
    let mut pending_inputs = pending_inputs;
    let mut next_user_seq = raw_next_user_seq;
    let migrated_user_sequences = if legacy_cursor {
        migrate_legacy_user_sequences(&mut pending_inputs, &mut next_user_seq, &mut user_epoch)?
    } else {
        false
    };
    let mut active_turn_commit = raw_active_turn_commit.as_ref().and_then(|value| {
        match serde_json::from_value::<ActiveTurnCommit>(value.clone()) {
            Ok(commit) if validate_active_turn_commit_shape(&commit).is_ok() => Some(commit),
            Ok(commit) => {
                if commit.kind == TurnCommitKind::User {
                    let target_ids = related_user_input_ids(value, &user_dispatch);
                    discard_incomplete_user_preparation(&mut pending_inputs, &target_ids);
                    user_dispatch = None;
                }
                invalid_active_turn_commit = Some((
                    value.clone(),
                    Some(commit),
                    "invalid-active-turn-commit".to_owned(),
                ));
                None
            }
            Err(_) => {
                if raw_marker_requires_user_cleanup(value, &user_dispatch) {
                    let target_ids = related_user_input_ids(value, &user_dispatch);
                    discard_incomplete_user_preparation(&mut pending_inputs, &target_ids);
                    user_dispatch = None;
                }
                invalid_active_turn_commit =
                    Some((value.clone(), None, "invalid-active-turn-commit".to_owned()));
                None
            }
        }
    });
    let mut pending = Vec::new();
    let mut quarantined = Vec::new();
    for value in raw_pending {
        match parse_observation(value.clone(), DEFAULT_OBSERVATION_LIMITS) {
            Ok(observation) => pending.push(observation),
            Err(_) => quarantined.push(value),
        }
    }
    let mut pending_deliveries = Vec::new();
    let mut quarantined_deliveries = Vec::new();
    for delivery in raw_pending_deliveries {
        if delivery.validate_payload_size().is_err() {
            quarantined_deliveries.push(delivery);
        } else {
            pending_deliveries.push(delivery);
        }
    }
    let ids = retain_ids(raw_ids);
    let failed = retain_ids(raw_failed);
    let cancelled_input_ids = raw_cancelled_input_ids;
    let consumed_frame_context_ids = retain_ids(raw_consumed_frame_context_ids);
    validate_pending_inputs(&pending_inputs, next_user_seq)?;
    validate_user_dispatch(&user_dispatch, next_dispatch_seq, &pending_inputs)?;
    if let Some(commit) = active_turn_commit.as_ref() {
        if let Err(error) =
            validate_active_turn_commit(Some(commit), &user_dispatch, &pending_inputs)
        {
            invalid_active_turn_commit = Some((
                raw_active_turn_commit
                    .clone()
                    .expect("valid active turn commit has a raw marker"),
                active_turn_commit.clone(),
                error.to_string(),
            ));
            if commit.kind == TurnCommitKind::User {
                let target_ids = related_user_input_ids(
                    raw_active_turn_commit
                        .as_ref()
                        .expect("valid active turn commit has a raw marker"),
                    &user_dispatch,
                );
                discard_incomplete_user_preparation(&mut pending_inputs, &target_ids);
                user_dispatch = None;
            }
            active_turn_commit = None;
        }
    }
    validate_observation_consumptions(&observation_consumptions)?;
    validate_observation_attempts(&observation_attempts)?;
    validate_pending_deliveries(&pending_deliveries)?;
    let has_quarantined_observations = !quarantined.is_empty();
    let has_quarantined_deliveries = !quarantined_deliveries.is_empty();
    let has_invalid_active_turn_commit = invalid_active_turn_commit.is_some();
    let needs_cursor_rewrite =
        legacy_cursor || migrated_user_sequences || has_invalid_active_turn_commit;
    if has_quarantined_observations || has_quarantined_deliveries || needs_cursor_rewrite {
        for record in quarantined {
            crate::persistence::JsonlStore::new(quarantine_path.to_owned()).append(&serde_json::json!({
                "schemaVersion": 1,
                "quarantinedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "reason": "invalid-pending-observation",
                "record": record,
            }))?;
        }
        for delivery in quarantined_deliveries {
            let (quarantine_id, record) = delivery_quarantine_record(&delivery)?;
            crate::persistence::JsonlStore::new(delivery_quarantine_path.to_owned())
                .append_idempotent(&record, |existing: &Value| {
                    existing.get("quarantineId").and_then(Value::as_str)
                        == Some(quarantine_id.as_str())
                })?;
        }
        if let Some((marker, commit, reason)) = invalid_active_turn_commit {
            let (quarantine_id, record) = turn_commit_quarantine_record(&marker, &reason)?;
            crate::persistence::JsonlStore::new(turn_commit_quarantine_path.to_owned())
                .append_idempotent(&record, |existing: &Value| {
                    existing.get("quarantineId").and_then(Value::as_str)
                        == Some(quarantine_id.as_str())
                })?;
            if let Some(commit) = commit {
                record_turn_commit_recovery_attempt(
                    &mut turn_commit_recovery_attempts,
                    &commit,
                    &reason,
                );
            }
        }
        #[cfg(test)]
        if has_quarantined_deliveries {
            failpoints::after_delivery_quarantine(path)?;
        }
        // cursor を先に書き戻すと隔離先の書き込み失敗時に payload を失うため、
        // 隔離が durable になった後で有効な record だけを正本へ残す。
        atomic_write_json(
            path,
            &CursorDocument {
                schema_version: CURSOR_SCHEMA_VERSION,
                user_operation_generation,
                user_epoch,
                next_user_seq,
                next_dispatch_seq,
                user_dispatch: &user_dispatch,
                active_turn_commit: &active_turn_commit,
                ids: &ids,
                pending: &pending,
                failed: &failed,
                observation_attempts: &observation_attempts,
                cancelled_input_ids: &cancelled_input_ids,
                pending_inputs: &pending_inputs,
                pending_deliveries: &pending_deliveries,
                pending_frame_contexts: &pending_frame_contexts,
                consumed_frame_context_ids: &consumed_frame_context_ids,
                observation_consumptions: &observation_consumptions,
                turn_commit_recovery_attempts: &turn_commit_recovery_attempts,
            },
        )?;
        if has_invalid_active_turn_commit {
            if let Ok(logger) = FileLogger::new(log_path.to_owned()) {
                let _ = logger.write(
                    "WARN",
                    "active TurnCommit markerをquarantineしました: error-type=invalid-marker",
                );
            }
        }
        if let Ok(logger) = FileLogger::new(log_path.to_owned()) {
            if has_quarantined_observations {
                let _ = logger.write("WARN", "pending observationをquarantineしました。");
            }
            if has_quarantined_deliveries {
                let _ = logger.write(
                    "WARN",
                    "pending deliveryをquarantineしました。 event=pending-delivery-quarantined error-type=payload-too-large",
                );
            }
        }
    }
    Ok(CursorSnapshot {
        user_operation_generation,
        user_epoch,
        next_user_seq,
        next_dispatch_seq,
        user_dispatch,
        active_turn_commit,
        ids,
        pending,
        failed,
        observation_attempts,
        cancelled_input_ids,
        pending_inputs,
        pending_deliveries,
        pending_frame_contexts,
        consumed_frame_context_ids,
        observation_consumptions,
        turn_commit_recovery_attempts,
    })
}

fn write_cursor_locked(path: &Path, cursor: &CursorSnapshot) -> Result<(), PersistenceError> {
    let ids = retain_ids(cursor.ids.clone());
    let failed = retain_ids(cursor.failed.clone());
    let observation_attempts = cursor.observation_attempts.clone();
    let consumed_frame_context_ids = retain_ids(cursor.consumed_frame_context_ids.clone());
    let cancelled_input_ids = cursor.cancelled_input_ids.clone();
    validate_pending_inputs(&cursor.pending_inputs, cursor.next_user_seq)?;
    validate_user_dispatch(
        &cursor.user_dispatch,
        cursor.next_dispatch_seq,
        &cursor.pending_inputs,
    )?;
    validate_active_turn_commit(
        cursor.active_turn_commit.as_ref(),
        &cursor.user_dispatch,
        &cursor.pending_inputs,
    )?;
    validate_observation_consumptions(&cursor.observation_consumptions)?;
    validate_observation_attempts(&observation_attempts)?;
    validate_pending_deliveries(&cursor.pending_deliveries)?;
    atomic_write_json(
        path,
        &CursorDocument {
            schema_version: CURSOR_SCHEMA_VERSION,
            user_operation_generation: cursor.user_operation_generation,
            user_epoch: cursor.user_epoch,
            next_user_seq: cursor.next_user_seq.max(
                cursor
                    .pending_inputs
                    .iter()
                    .map(|pending| match pending {
                        PendingInput::UserMessage(input) => input.user_seq,
                    })
                    .max()
                    .unwrap_or(0),
            ),
            next_dispatch_seq: cursor.next_dispatch_seq,
            user_dispatch: &cursor.user_dispatch,
            active_turn_commit: &cursor.active_turn_commit,
            ids: &ids,
            pending: &cursor.pending,
            failed: &failed,
            observation_attempts: &observation_attempts,
            cancelled_input_ids: &cancelled_input_ids,
            pending_inputs: &cursor.pending_inputs,
            pending_deliveries: &cursor.pending_deliveries,
            pending_frame_contexts: &cursor.pending_frame_contexts,
            consumed_frame_context_ids: &consumed_frame_context_ids,
            observation_consumptions: &cursor.observation_consumptions,
            turn_commit_recovery_attempts: &cursor.turn_commit_recovery_attempts,
        },
    )?;
    Ok(())
}

fn load_recent_conversation(
    directory: &Path,
    limit: usize,
    generation: u64,
) -> Result<Vec<ConversationEntry>, PersistenceError> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let paths = daily_conversation_paths(directory)?;
    let mut entries = Vec::new();
    for path in paths {
        let lock_path = path.with_file_name(format!(
            ".{}.lock",
            path.file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    PersistenceError::Invalid("conversation のファイル名が不正です".to_owned())
                })?
        ));
        let _guard = SiblingLock::acquire(&lock_path)?;
        let file = File::open(path)?;
        for line in BufReader::new(file).lines() {
            let line = line?;
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(entry) = conversation_entry_from_storage_value(value, generation) {
                entries.push(entry);
            }
        }
    }
    if entries.len() > limit {
        Ok(entries.split_off(entries.len() - limit))
    } else {
        Ok(entries)
    }
}

pub(crate) fn conversation_entry_from_storage_value(
    mut value: Value,
    generation: u64,
) -> Option<ConversationEntry> {
    let stored_generation = value
        .get("conversationGeneration")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if stored_generation != generation {
        return None;
    }
    value.as_object_mut()?.remove("conversationGeneration");
    let entry = serde_json::from_value::<ConversationEntry>(value).ok()?;
    validate_conversation_entry(&entry).ok()?;
    Some(entry)
}

fn conversation_storage_value(
    entry: &ConversationEntry,
    generation: u64,
) -> Result<Value, PersistenceError> {
    let mut value = serde_json::to_value(entry)?;
    if generation != 0 {
        value
            .as_object_mut()
            .ok_or_else(|| {
                PersistenceError::Invalid("conversation entry が object ではありません".to_owned())
            })?
            .insert("conversationGeneration".to_owned(), Value::from(generation));
    }
    Ok(value)
}

fn daily_conversation_paths(directory: &Path) -> Result<Vec<PathBuf>, PersistenceError> {
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
                && entry
                    .path()
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| {
                        value.len() == 16
                            && value.ends_with(".jsonl")
                            && value[..10]
                                .chars()
                                .all(|character| character.is_ascii_digit() || character == '-')
                    })
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn validate_pending_inputs(
    inputs: &[PendingInput],
    next_user_seq: u64,
) -> Result<(), PersistenceError> {
    let mut ids = std::collections::HashSet::new();
    let mut seqs = std::collections::HashSet::new();
    let mut previous_seq = 0;
    for input in inputs {
        let PendingInput::UserMessage(input) = input;
        if input.id.is_empty()
            || !ids.insert(input.id.as_str())
            || input.user_seq == 0
            || input.user_seq > next_user_seq
            || !seqs.insert(input.user_seq)
            || chrono::DateTime::parse_from_rfc3339(&input.created_at).is_err()
            || (input.message.is_empty()
                && input.attachment_path.is_none()
                && input.attachment_text.is_none())
            || input
                .attachment_failure
                .as_ref()
                .is_some_and(|failure| failure.attempts == 0)
            || input.prepared_response.as_ref().is_some_and(|response| {
                response.id.is_empty()
                    || response.message.is_empty()
                    || response
                        .session_summary
                        .as_deref()
                        .is_some_and(|summary| summary.trim().is_empty())
                    || chrono::DateTime::parse_from_rfc3339(&response.created_at).is_err()
            })
        {
            return Err(PersistenceError::Invalid(
                "cursor の pendingInputs が不正です".to_owned(),
            ));
        }
        if input.user_seq <= previous_seq {
            return Err(PersistenceError::Invalid(
                "cursor の pendingInputs の順序が不正です".to_owned(),
            ));
        }
        previous_seq = input.user_seq;
    }
    Ok(())
}

fn migrate_legacy_user_sequences(
    inputs: &mut [PendingInput],
    next_user_seq: &mut u64,
    user_epoch: &mut u64,
) -> Result<bool, PersistenceError> {
    if inputs.is_empty() {
        return Ok(false);
    }
    for input in inputs {
        let PendingInput::UserMessage(input) = input;
        *next_user_seq = next_user_seq.checked_add(1).ok_or_else(|| {
            PersistenceError::Invalid("旧 cursor の user seq を移行できません".to_owned())
        })?;
        input.user_seq = *next_user_seq;
        *user_epoch = user_epoch.checked_add(1).ok_or_else(|| {
            PersistenceError::Invalid("旧 cursor の user epoch を移行できません".to_owned())
        })?;
    }
    Ok(true)
}

fn validate_observation_attempts(attempts: &[ObservationAttempt]) -> Result<(), PersistenceError> {
    let mut ids = HashSet::new();
    if attempts.iter().any(|attempt| {
        attempt.observation_id.is_empty()
            || attempt.attempts == 0
            || !ids.insert(attempt.observation_id.as_str())
    }) {
        return Err(PersistenceError::Invalid(
            "cursor の observationAttempts が不正です".to_owned(),
        ));
    }
    Ok(())
}

fn validate_user_dispatch(
    lease: &Option<UserDispatchLease>,
    next_dispatch_seq: u64,
    pending_inputs: &[PendingInput],
) -> Result<(), PersistenceError> {
    let Some(lease) = lease else {
        return Ok(());
    };
    if lease.dispatch_seq == 0
        || lease.dispatch_seq > next_dispatch_seq
        || lease.input_ids.is_empty()
    {
        return Err(PersistenceError::Invalid(
            "cursor の user dispatch lease が不正です".to_owned(),
        ));
    }
    let mut ids = HashSet::new();
    let mut last_seq = 0;
    for id in &lease.input_ids {
        if !ids.insert(id.as_str()) {
            return Err(PersistenceError::Invalid(
                "user dispatch lease の ID が重複しています".to_owned(),
            ));
        }
        let Some(PendingInput::UserMessage(input)) =
            pending_inputs.iter().find(|pending| pending.id() == id)
        else {
            return Err(PersistenceError::Invalid(
                "user dispatch lease の入力がありません".to_owned(),
            ));
        };
        if input.attachment_is_terminal() || input.user_seq <= last_seq {
            return Err(PersistenceError::Invalid(
                "user dispatch lease の順序が不正です".to_owned(),
            ));
        }
        last_seq = input.user_seq;
    }
    let runnable_prefix = pending_inputs
        .iter()
        .filter_map(|pending| match pending {
            PendingInput::UserMessage(input) if !input.attachment_is_terminal() => {
                Some(input.id.as_str())
            }
            _ => None,
        })
        .take(lease.input_ids.len())
        .collect::<Vec<_>>();
    if runnable_prefix
        != lease
            .input_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        return Err(PersistenceError::Invalid(
            "user dispatch lease が user queue の先頭 prefix ではありません".to_owned(),
        ));
    }
    Ok(())
}

fn validate_active_turn_commit(
    commit: Option<&ActiveTurnCommit>,
    user_dispatch: &Option<UserDispatchLease>,
    pending_inputs: &[PendingInput],
) -> Result<(), PersistenceError> {
    let Some(commit) = commit else {
        return Ok(());
    };
    validate_active_turn_commit_shape(commit)?;
    if commit.kind != TurnCommitKind::User {
        return Ok(());
    }
    let target_inputs = commit
        .target_ids
        .iter()
        .map(|id| {
            pending_inputs.iter().find_map(|pending| match pending {
                PendingInput::UserMessage(input) if &input.id == id => Some(input),
                _ => None,
            })
        })
        .collect::<Vec<_>>();
    let missing_inputs = target_inputs.iter().any(Option::is_none);
    if missing_inputs {
        if commit.phase == TurnCommitPhase::Persisting && user_dispatch.is_none() {
            return Ok(());
        }
        return Err(PersistenceError::Invalid(
            "active TurnCommit の対象 user input がありません".to_owned(),
        ));
    }
    let target_inputs = target_inputs.into_iter().flatten().collect::<Vec<_>>();
    let has_prepared = target_inputs
        .iter()
        .any(|input| input.prepared_response.is_some());
    if target_inputs
        .iter()
        .any(|input| input.response_commit_started && input.prepared_response.is_none())
    {
        return Err(PersistenceError::Invalid(
            "active TurnCommit の response commit 状態が不正です".to_owned(),
        ));
    }
    if has_prepared && !has_complete_prepared_response(pending_inputs, commit) {
        return Err(PersistenceError::Invalid(
            "active TurnCommit の prepared response が不一致です".to_owned(),
        ));
    }
    let lease_matches = user_dispatch.as_ref().is_some_and(|lease| {
        Some(lease.dispatch_seq) == commit.dispatch_seq && lease.input_ids == commit.target_ids
    });
    if user_dispatch.is_some() && !lease_matches {
        return Err(PersistenceError::Invalid(
            "active TurnCommit と user dispatch lease が一致しません".to_owned(),
        ));
    }
    if user_dispatch.is_none()
        && (commit.phase != TurnCommitPhase::Persisting
            || target_inputs
                .iter()
                .any(|input| !input.response_commit_started))
    {
        return Err(PersistenceError::Invalid(
            "active TurnCommit の user dispatch lease がありません".to_owned(),
        ));
    }
    Ok(())
}

fn validate_active_turn_commit_shape(commit: &ActiveTurnCommit) -> Result<(), PersistenceError> {
    let mut target_ids = HashSet::new();
    if commit.turn_id.is_empty()
        || commit.target_ids.is_empty()
        || commit
            .target_ids
            .iter()
            .any(|id| id.is_empty() || !target_ids.insert(id.as_str()))
        || matches!(commit.kind, TurnCommitKind::User)
            && (commit.dispatch_seq.is_none_or(|seq| seq == 0)
                || commit.expected_user_epoch.is_some())
        || matches!(commit.kind, TurnCommitKind::Proactive)
            && (commit.expected_user_epoch.is_none() || commit.dispatch_seq.is_some())
    {
        return Err(PersistenceError::Invalid(
            "cursor の active turn commit が不正です".to_owned(),
        ));
    }
    Ok(())
}

fn discard_incomplete_user_preparation(pending_inputs: &mut [PendingInput], target_ids: &[String]) {
    for pending in pending_inputs {
        if target_ids.iter().any(|id| id == pending.id()) {
            let PendingInput::UserMessage(input) = pending;
            input.prepared_response = None;
            input.response_commit_started = false;
        }
    }
}

fn has_complete_prepared_response(
    pending_inputs: &[PendingInput],
    commit: &ActiveTurnCommit,
) -> bool {
    let target_inputs = commit
        .target_ids
        .iter()
        .filter_map(|id| {
            pending_inputs.iter().find_map(|pending| match pending {
                PendingInput::UserMessage(input) if &input.id == id => Some(input),
                _ => None,
            })
        })
        .collect::<Vec<_>>();
    if target_inputs.len() != commit.target_ids.len() {
        return false;
    }
    let Some(expected) = target_inputs
        .first()
        .and_then(|input| input.prepared_response.as_ref())
    else {
        return false;
    };
    target_inputs
        .iter()
        .all(|input| input.prepared_response.as_ref() == Some(expected))
}

fn raw_marker_requires_user_cleanup(
    marker: &Value,
    user_dispatch: &Option<UserDispatchLease>,
) -> bool {
    match marker.get("kind").and_then(Value::as_str) {
        Some("user") => true,
        Some("proactive") => false,
        Some(_) | None => user_dispatch.is_some(),
    }
}

fn related_user_input_ids(
    marker: &Value,
    user_dispatch: &Option<UserDispatchLease>,
) -> Vec<String> {
    let mut ids = marker
        .get("targetIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if raw_marker_requires_user_cleanup(marker, user_dispatch) {
        if let Some(lease) = user_dispatch {
            ids.extend(lease.input_ids.iter().cloned());
        }
    }
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    ids
}

fn record_turn_commit_recovery_attempt(
    attempts: &mut Vec<TurnCommitRecoveryAttempt>,
    commit: &ActiveTurnCommit,
    reason: &str,
) {
    attempts.push(TurnCommitRecoveryAttempt {
        turn_id: commit.turn_id.clone(),
        kind: commit.kind.clone(),
        target_ids: commit.target_ids.clone(),
        reason: reason.to_owned(),
        recorded_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    if attempts.len() > MAX_CURSOR_IDS {
        attempts.drain(..attempts.len() - MAX_CURSOR_IDS);
    }
}

fn turn_commit_quarantine_record(
    marker: &Value,
    reason: &str,
) -> Result<(String, Value), PersistenceError> {
    let digest = Sha256::digest(serde_json::to_vec(marker)?);
    let quarantine_id = format!("{:x}", digest);
    Ok((
        quarantine_id.clone(),
        serde_json::json!({
            "schemaVersion": 1,
            "quarantineId": quarantine_id,
            "quarantinedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "reason": reason,
            "record": marker,
        }),
    ))
}

fn validate_observation_consumptions(
    consumptions: &[ObservationConsumption],
) -> Result<(), PersistenceError> {
    let mut ids = HashSet::new();
    if consumptions.iter().any(|consumption| {
        consumption.observation_id.is_empty()
            || consumption.turn_id.is_empty()
            || consumption.reason.is_empty()
            || !ids.insert(consumption.observation_id.as_str())
    }) {
        return Err(PersistenceError::Invalid(
            "cursor の observationConsumptions が不正です".to_owned(),
        ));
    }
    Ok(())
}

fn validate_pending_deliveries(deliveries: &[PendingDelivery]) -> Result<(), PersistenceError> {
    let mut remark_ids = std::collections::HashSet::new();
    for delivery in deliveries {
        delivery.validate_payload_size()?;
        let mut observation_ids = std::collections::HashSet::new();
        if delivery.remark_id.is_empty()
            || chrono::DateTime::parse_from_rfc3339(&delivery.created_at).is_err()
            || delivery.message.is_empty()
            || !matches!(
                delivery.notification_priority.as_str(),
                "none" | "info" | "warning" | "critical"
            )
            || delivery.observation_ids.is_empty()
            || !remark_ids.insert(delivery.remark_id.as_str())
            || delivery
                .observation_ids
                .iter()
                .any(|id| id.is_empty() || !observation_ids.insert(id.as_str()))
        {
            return Err(PersistenceError::Invalid(
                "cursor の pendingDeliveries が不正です".to_owned(),
            ));
        }
    }
    Ok(())
}

fn conversation_local_date(entry: &ConversationEntry) -> Result<String, PersistenceError> {
    Ok(chrono::DateTime::parse_from_rfc3339(&entry.created_at)
        .map_err(|_| {
            PersistenceError::Invalid("conversation entry の createdAt が不正です".to_owned())
        })?
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string())
}

fn validate_conversation_entry(entry: &ConversationEntry) -> Result<(), PersistenceError> {
    if entry.schema_version != 1
        || entry.id.is_empty()
        || entry.created_at.is_empty()
        || !matches!(
            entry.notification_priority.as_str(),
            "none" | "info" | "warning" | "critical"
        )
    {
        return Err(PersistenceError::Invalid(
            "conversation entry の形式が不正です".to_owned(),
        ));
    }
    Ok(())
}

fn cursor_lock_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.lock",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("cursor")
    ))
}

fn retain_ids(mut ids: Vec<String>) -> Vec<String> {
    if ids.len() > MAX_CURSOR_IDS {
        ids.drain(..ids.len() - MAX_CURSOR_IDS);
    }
    ids
}

