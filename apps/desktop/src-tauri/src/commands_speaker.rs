use crate::commands::{authorize_main_or_details, IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::HelperResolverPort;
use coosenpai_core::speaker_name_proposals::{
    load_speaker_name_proposals, update_speaker_name_proposals,
    verify_stored_speaker_name_proposal, SpeakerNameApplyOperation,
    SpeakerNameProposalResultRecord, SpeakerNameProposalResultRecordStatus,
    SpeakerNameProposalStatus, SpeakerNameProposalUndoCardStatus, StoredSpeakerNameProposal,
};
use coosenpai_core::speaker_names::{load_speaker_names, save_speaker_name, validate_speaker_name};
use coosenpai_core::state::{ConversationEntry, ConversationMessageKind, ConversationRole};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tauri::{State, WebviewWindow};
use tokio::sync::{mpsc, oneshot};

type SpeakerQueueJob = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// 話者台帳と表示名索引に触れる処理を、desktop process 内で一つの FIFO に通す。
#[derive(Clone)]
pub(crate) struct SpeakerManagementQueue {
    sender: mpsc::UnboundedSender<SpeakerQueueJob>,
}

impl SpeakerManagementQueue {
    pub(crate) fn channel() -> (Self, impl Future<Output = ()> + Send + 'static) {
        let (sender, mut receiver) = mpsc::unbounded_channel::<SpeakerQueueJob>();
        let run = async move {
            while let Some(job) = receiver.recv().await {
                job.await;
            }
        };
        (Self { sender }, run)
    }

    pub(crate) async fn enqueue<T, F, Fut>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(Box::pin(async move {
                let _ = reply.send(operation().await);
            }))
            .map_err(|_| "話者操作キューが終了しました".to_owned())?;
        result
            .await
            .map_err(|_| "話者操作キューの応答がありません".to_owned())?
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerManagementPayload {
    operation: String,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    speaker_id: Option<String>,
}

pub(crate) fn valid_speaker_id(value: &str) -> bool {
    coosenpai_core::speaker_id::is_valid_speaker_id(value)
}

fn management_arguments(
    payload: SpeakerManagementPayload,
    locale: Locale,
) -> Result<Vec<String>, String> {
    let invalid = || text(TextKey::CommandInvalidInput, locale).to_owned();
    let mut arguments = vec!["--speaker-management".to_owned()];
    match payload.operation.as_str() {
        "merge" => {
            let (Some(source_id), Some(target_id)) = (payload.source_id, payload.target_id) else {
                return Err(invalid());
            };
            if !valid_speaker_id(&source_id)
                || !valid_speaker_id(&target_id)
                || payload.speaker_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend([
                "merge".to_owned(),
                "--speaker-from".to_owned(),
                source_id,
                "--speaker-to".to_owned(),
                target_id,
            ]);
        }
        "undoMerge" => {
            let Some(source_id) = payload.source_id else {
                return Err(invalid());
            };
            if !valid_speaker_id(&source_id)
                || payload.target_id.is_some()
                || payload.speaker_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend([
                "undo-merge".to_owned(),
                "--speaker-from".to_owned(),
                source_id,
            ]);
        }
        "reregister" => {
            let Some(speaker_id) = payload.speaker_id else {
                return Err(invalid());
            };
            if !valid_speaker_id(&speaker_id)
                || payload.source_id.is_some()
                || payload.target_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend([
                "reregister".to_owned(),
                "--speaker-id".to_owned(),
                speaker_id,
            ]);
        }
        "delete" => {
            let Some(speaker_id) = payload.speaker_id else {
                return Err(invalid());
            };
            if !valid_speaker_id(&speaker_id)
                || payload.source_id.is_some()
                || payload.target_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend(["delete".to_owned(), "--speaker-id".to_owned(), speaker_id]);
        }
        "deleteAll" => {
            if payload.source_id.is_some()
                || payload.target_id.is_some()
                || payload.speaker_id.is_some()
            {
                return Err(invalid());
            }
            arguments.push("delete-all".to_owned());
        }
        _ => return Err(invalid()),
    }
    Ok(arguments)
}

fn hearing_helper(state: &DesktopState) -> Option<std::path::PathBuf> {
    let executable_directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_owned))?;
    crate::platform::MacHelperResolver
        .resolve_hearing_helper(&executable_directory, &state.paths.root)
}

async fn spawn_speaker_helper(
    state: &DesktopState,
    arguments: Vec<String>,
) -> Result<std::process::Output, String> {
    let Some(helper) = hearing_helper(state) else {
        return Err("話者管理 helper が見つかりません".to_owned());
    };
    let ledger = state.paths.speakers.join("registry.enc");
    tokio::process::Command::new(helper)
        .args(arguments)
        .arg("--speaker-ledger")
        .arg(ledger)
        .output()
        .await
        .map_err(|error| format!("話者管理 helper を起動できません: {error}"))
}

fn helper_failure_message(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|value| {
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            (!stderr.is_empty()).then_some(stderr)
        })
        .unwrap_or_else(|| "話者管理 helper が失敗しました".to_owned())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerSummary {
    pub(crate) id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MergedSpeakers {
    source_id: String,
    target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_name: Option<String>,
}

// 統合・取り消し・表示名の UI が提示する候補。声紋・鍵・registry UUID はフロントへ出さない。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerDirectory {
    pub(crate) speakers: Vec<SpeakerSummary>,
    pub(crate) merged: Vec<MergedSpeakers>,
}

impl SpeakerDirectory {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self
            .speakers
            .iter()
            .any(|speaker| !valid_speaker_id(&speaker.id))
            || self.merged.iter().any(|pair| {
                !valid_speaker_id(&pair.source_id)
                    || !valid_speaker_id(&pair.target_id)
                    || pair.source_id == pair.target_id
            })
        {
            return Err("話者一覧の話者 ID が不正です".to_owned());
        }
        Ok(())
    }

    pub(crate) fn has_speaker(&self, id: &str) -> bool {
        self.speakers.iter().any(|speaker| speaker.id == id)
    }

    pub(crate) fn has_merged_source(&self, id: &str) -> bool {
        self.merged.iter().any(|pair| pair.source_id == id)
    }
}

struct HelperDirectory {
    registry_id: Option<String>,
    speakers: Vec<String>,
    aliases: BTreeMap<String, String>,
}

fn parse_helper_directory(stdout: &[u8]) -> Result<HelperDirectory, String> {
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("event").and_then(serde_json::Value::as_str) {
            Some("speaker-directory") => {
                let registry_id = match value.get("registryId") {
                    Some(serde_json::Value::Null) | None => None,
                    Some(serde_json::Value::String(id)) if uuid::Uuid::parse_str(id).is_ok() => {
                        Some(id.clone())
                    }
                    _ => return Err("話者一覧の registryId が不正です".to_owned()),
                };
                let speakers: Vec<String> =
                    serde_json::from_value(value.get("speakers").cloned().unwrap_or_default())
                        .map_err(|error| format!("話者一覧が不正です: {error}"))?;
                let aliases: BTreeMap<String, String> =
                    serde_json::from_value(value.get("aliases").cloned().unwrap_or_default())
                        .map_err(|error| format!("話者の別名対応が不正です: {error}"))?;
                if speakers.iter().any(|id| !valid_speaker_id(id))
                    || aliases.iter().any(|(source, target)| {
                        !valid_speaker_id(source) || !valid_speaker_id(target) || source == target
                    })
                {
                    return Err("話者一覧の話者 ID が不正です".to_owned());
                }
                return Ok(HelperDirectory {
                    registry_id,
                    speakers,
                    aliases,
                });
            }
            Some("error") => {
                let message = value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("話者管理 helper が失敗しました");
                return Err(message.to_owned());
            }
            _ => {}
        }
    }
    Err("話者管理 helper から話者一覧を取得できませんでした".to_owned())
}

fn build_directory(raw: HelperDirectory, names: BTreeMap<String, String>) -> SpeakerDirectory {
    SpeakerDirectory {
        speakers: raw
            .speakers
            .iter()
            .map(|id| SpeakerSummary {
                id: id.clone(),
                name: names.get(id).cloned(),
            })
            .collect(),
        merged: raw
            .aliases
            .iter()
            .map(|(source, target)| MergedSpeakers {
                source_id: source.clone(),
                target_id: target.clone(),
                source_name: names.get(source).cloned(),
                target_name: names.get(target).cloned(),
            })
            .collect(),
    }
}

async fn load_directory(state: &DesktopState) -> Result<SpeakerDirectory, String> {
    let raw = load_helper_directory(state).await?;
    let names = match &raw.registry_id {
        Some(registry_id) => {
            load_speaker_names(&state.paths, registry_id).map_err(|error| error.to_string())?
        }
        None => BTreeMap::new(),
    };
    Ok(build_directory(raw, names))
}

async fn load_helper_directory(state: &DesktopState) -> Result<HelperDirectory, String> {
    let output = spawn_speaker_helper(
        state,
        vec!["--speaker-management".to_owned(), "list".to_owned()],
    )
    .await?;
    if !output.status.success() {
        return Err(helper_failure_message(&output));
    }
    parse_helper_directory(&output.stdout)
}

#[tauri::command]
pub async fn speaker_directory(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<SpeakerDirectory> {
    authorize_main_or_details(&window)?;
    let queue = state.speaker_queue.clone();
    let queued_state = state.inner().clone();
    Ok(
        match queue
            .enqueue(move || async move { load_directory(&queued_state).await })
            .await
        {
            Ok(directory) => IpcResult::success(directory),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerRenamePayload {
    speaker_id: String,
    name: Option<String>,
}

#[tauri::command]
pub async fn speaker_rename(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeakerRenamePayload,
) -> TauriIpcResult<SpeakerDirectory> {
    authorize_main_or_details(&window)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    if !valid_speaker_id(&payload.speaker_id) {
        return Ok(IpcResult::failure(
            text(TextKey::CommandInvalidInput, locale).to_owned(),
        ));
    }
    let queue = state.speaker_queue.clone();
    let queued_state = state.inner().clone();
    Ok(
        match queue
            .enqueue(move || async move {
                let directory = rename_speaker(&queued_state, payload).await?;
                queued_state
                    .publish_event(
                        crate::snapshot_presenter::SnapshotEvent::SpeakerDirectoryChanged,
                    )
                    .await;
                Ok(directory)
            })
            .await
        {
            Ok(directory) => IpcResult::success(directory),
            Err(message) => IpcResult::failure(message),
        },
    )
}

// 表示名の検証と話者の実在確認をしてから保存し、保存後の候補一覧を返す。
async fn rename_speaker(
    state: &DesktopState,
    payload: SpeakerRenamePayload,
) -> Result<SpeakerDirectory, String> {
    let name = payload
        .name
        .as_deref()
        .map(validate_speaker_name)
        .transpose()?;
    let output = spawn_speaker_helper(
        state,
        vec!["--speaker-management".to_owned(), "list".to_owned()],
    )
    .await?;
    if !output.status.success() {
        return Err(helper_failure_message(&output));
    }
    let raw = parse_helper_directory(&output.stdout)?;
    let registry_id = raw
        .registry_id
        .clone()
        .ok_or("話者台帳がありません".to_owned())?;
    if !raw.speakers.iter().any(|id| id == &payload.speaker_id) {
        return Err("名前を付ける話者 ID が見つかりません".to_owned());
    }
    save_speaker_name(
        &state.paths,
        &registry_id,
        &payload.speaker_id,
        name.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let names =
        load_speaker_names(&state.paths, &registry_id).map_err(|error| error.to_string())?;
    Ok(build_directory(raw, names))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProposalActionKind {
    Register,
    Edit,
    EditSubmit,
    EditCancel,
    Reject,
    Undo,
}

struct ParsedProposalAction {
    kind: ProposalActionKind,
    proposal_id: String,
    version: u64,
}

enum ProposalActionEffect {
    Edit(StoredSpeakerNameProposal),
    EditCancelled(StoredSpeakerNameProposal),
    Edited { old_bubble_id: String },
    Applied { old_bubble_id: String },
    Expired { old_bubble_id: String },
    Rejected { old_bubble_id: String },
    Undone { old_bubble_id: String },
}

fn parse_proposal_action(action: &str) -> Option<ParsedProposalAction> {
    let mut parts = action.split(':');
    if parts.next()? != "speaker-name" {
        return None;
    }
    let kind = match parts.next()? {
        "register" => ProposalActionKind::Register,
        "edit" => ProposalActionKind::Edit,
        "edit-submit" => ProposalActionKind::EditSubmit,
        "edit-cancel" => ProposalActionKind::EditCancel,
        "reject" => ProposalActionKind::Reject,
        "undo" => ProposalActionKind::Undo,
        _ => return None,
    };
    let proposal_id = parts.next()?.to_owned();
    let version = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() || uuid::Uuid::parse_str(&proposal_id).is_err() || version == 0 {
        return None;
    }
    Some(ParsedProposalAction {
        kind,
        proposal_id,
        version,
    })
}

fn current_user_messages(state: &DesktopState) -> Result<Vec<(String, String)>, String> {
    let config = state.runtime_config();
    let storage = coosenpai_core::companion_storage::CompanionStorage::from_paths(
        &state.paths,
        config.retention.conversation_days,
    );
    storage
        .load_conversation()
        .map(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.role == ConversationRole::User)
                .map(|entry| (entry.id, entry.message))
                .collect()
        })
        .map_err(|_| "話者名登録の依頼元を確認できません".to_owned())
}

#[derive(Debug, Clone)]
struct SpeakerNameProposalBubbleDelivery {
    proposal_id: String,
    version: u64,
    undo_result: bool,
    record: crate::bubbles::BubbleRecord,
    replaced_ids: Vec<String>,
}

struct ReconciledSpeakerNameProposalResults {
    conversation_record_appended: bool,
    display: Vec<SpeakerNameProposalBubbleDelivery>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultCardDeliveryStart {
    Deliver,
    ConfirmOnly,
    AlreadyDelivered,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultCardDeliveryCommit {
    Delivered,
    AlreadyDelivered,
    Expired,
    Stale,
}

fn result_record_state_mut(
    proposal: &mut StoredSpeakerNameProposal,
    undo: bool,
) -> &mut Option<SpeakerNameProposalResultRecord> {
    if undo {
        &mut proposal.undo_completion_record
    } else {
        &mut proposal.completion_record
    }
}

fn result_record_state(
    proposal: &StoredSpeakerNameProposal,
    undo: bool,
) -> Option<&SpeakerNameProposalResultRecord> {
    if undo {
        proposal.undo_completion_record.as_ref()
    } else {
        proposal.completion_record.as_ref()
    }
}

fn result_record_operation_id(proposal: &StoredSpeakerNameProposal, undo: bool) -> Option<&str> {
    let operation = proposal.operation.as_ref()?;
    if undo {
        operation.undo_id.as_deref()
    } else {
        Some(operation.id.as_str())
    }
}

fn persist_result_record_state(
    paths: &coosenpai_core::config::ConfigPaths,
    proposal: &StoredSpeakerNameProposal,
    undo: bool,
) -> Result<(), String> {
    let operation_id = result_record_operation_id(proposal, undo)
        .ok_or_else(|| "話者名登録の操作記録がありません".to_owned())?
        .to_owned();
    let record = result_record_state(proposal, undo)
        .cloned()
        .ok_or_else(|| "話者名登録の完了記録状態がありません".to_owned())?;
    update_speaker_name_proposals(paths, |store| {
        let existing = store
            .proposals
            .iter_mut()
            .find(|item| item.id == proposal.id && item.version == proposal.version)
            .ok_or_else(|| {
                coosenpai_core::persistence::PersistenceError::Invalid(
                    "話者名登録の提案が更新されました".to_owned(),
                )
            })?;
        if result_record_operation_id(existing, undo) != Some(operation_id.as_str()) {
            return Err(coosenpai_core::persistence::PersistenceError::Invalid(
                "話者名登録の操作が競合しました".to_owned(),
            ));
        }
        *result_record_state_mut(existing, undo) = Some(record);
        existing.undo_card_status = proposal.undo_card_status;
        existing.confirmation_bubble_id = proposal.confirmation_bubble_id.clone();
        existing.pending_bubble_dismissal_id = proposal.pending_bubble_dismissal_id.clone();
        Ok(())
    })
    .map_err(|_| "話者名登録の完了記録状態を保存できません".to_owned())
}

struct ProposalResultEntryWrite<'a> {
    paths: &'a coosenpai_core::config::ConfigPaths,
    config: &'a coosenpai_core::config::Config,
    proposal: &'a StoredSpeakerNameProposal,
    operation_id: &'a str,
    undo: bool,
    conversation_generation: u64,
    entry_created_at: &'a str,
    now: chrono::DateTime<chrono::Utc>,
}

fn write_proposal_result_entry(write: ProposalResultEntryWrite<'_>) -> Result<bool, String> {
    let ProposalResultEntryWrite {
        paths,
        config,
        proposal,
        operation_id,
        undo,
        conversation_generation,
        entry_created_at,
        now,
    } = write;
    let Some(operation) = proposal.operation.as_ref() else {
        return Err("話者名登録の操作記録がありません".to_owned());
    };
    let created_at = chrono::DateTime::parse_from_rfc3339(entry_created_at)
        .map_err(|_| "話者名登録の完了記録時刻が不正です".to_owned())?
        .with_timezone(&chrono::Utc);
    let locale = Locale::from_config(&config.ui.language);
    let message = crate::speaker_name_proposal_presenter::conversation_result_message(
        operation, locale, undo,
    );
    let entry = ConversationEntry {
        schema_version: 1,
        id: operation_id.to_owned(),
        created_at: created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        role: ConversationRole::Companion,
        message,
        message_kind: Some(ConversationMessageKind::Chat),
        attachment_path: None,
        attachment_text: None,
        tutorial_response_key: None,
        screen_context: None,
        caused_by_ids: proposal.source_user_message_ids.clone(),
        notification_priority: "none".to_owned(),
    };
    let storage = coosenpai_core::companion_storage::CompanionStorage::from_paths(
        paths,
        config.retention.conversation_days,
    );
    storage
        .append_conversation_once_at_generation(&entry, conversation_generation, now)
        .map_err(|_| "話者名登録の結果を会話へ保存できません".to_owned())
}

fn result_entry_was_pruned(
    entry_created_at: Option<&str>,
    retention_days: u64,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(entry_created_at) = entry_created_at else {
        return false;
    };
    let Ok(entry_created_at) = chrono::DateTime::parse_from_rfc3339(entry_created_at) else {
        return true;
    };
    let days = i64::try_from(retention_days).unwrap_or(i64::MAX);
    let cutoff = now.with_timezone(&chrono::Local).date_naive() - chrono::Duration::days(days);
    entry_created_at.with_timezone(&chrono::Local).date_naive() < cutoff
}

fn completion_record(
    status: SpeakerNameProposalResultRecordStatus,
    conversation_generation: Option<u64>,
    entry_created_at: Option<String>,
    recorded_at: Option<String>,
) -> SpeakerNameProposalResultRecord {
    SpeakerNameProposalResultRecord {
        status,
        conversation_generation,
        entry_created_at,
        recorded_at,
    }
}

fn initialize_applied_result_state(
    proposal: &mut StoredSpeakerNameProposal,
    conversation_generation: Option<u64>,
    now: chrono::DateTime<chrono::Utc>,
) {
    let created_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    proposal.completion_record = Some(completion_record(
        SpeakerNameProposalResultRecordStatus::Pending,
        conversation_generation,
        Some(created_at),
        None,
    ));
    proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Pending;
    proposal.confirmation_bubble_id =
        Some(crate::speaker_name_proposal_presenter::result_id(proposal));
}

fn reconcile_speaker_name_proposal_results(
    paths: &coosenpai_core::config::ConfigPaths,
    config: &coosenpai_core::config::Config,
    proposals: &mut [StoredSpeakerNameProposal],
    conversation_generation: Option<u64>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ReconciledSpeakerNameProposalResults, String> {
    let operation_ids = proposals
        .iter()
        .filter(|proposal| {
            matches!(
                proposal.status,
                SpeakerNameProposalStatus::Applied | SpeakerNameProposalStatus::Undone
            )
        })
        .filter_map(|proposal| {
            let operation = proposal.operation.as_ref()?;
            let needs_applied_lookup = proposal.status == SpeakerNameProposalStatus::Applied
                && proposal.completion_record.as_ref().is_none_or(|record| {
                    record.status == SpeakerNameProposalResultRecordStatus::Pending
                });
            let needs_undo_lookup = proposal.status == SpeakerNameProposalStatus::Undone
                && proposal
                    .undo_completion_record
                    .as_ref()
                    .is_none_or(|record| {
                        record.status == SpeakerNameProposalResultRecordStatus::Pending
                    });
            if !needs_applied_lookup && !needs_undo_lookup {
                return None;
            }
            if needs_undo_lookup {
                operation.undo_id.clone()
            } else {
                Some(operation.id.clone())
            }
        })
        .collect::<std::collections::HashSet<_>>();
    let storage = coosenpai_core::companion_storage::CompanionStorage::from_paths(
        paths,
        config.retention.conversation_days,
    );
    let existing_entries = storage
        .existing_conversation_entry_generations(&operation_ids)
        .map_err(|_| "話者名登録の結果を会話から確認できません".to_owned())?;
    let mut conversation_record_appended = false;
    let mut display = Vec::new();

    for proposal in proposals {
        let Some(operation_id) = result_record_operation_id(
            proposal,
            proposal.status == SpeakerNameProposalStatus::Undone,
        )
        .map(str::to_owned) else {
            continue;
        };
        let undo = proposal.status == SpeakerNameProposalStatus::Undone;
        if !matches!(
            proposal.status,
            SpeakerNameProposalStatus::Applied | SpeakerNameProposalStatus::Undone
        ) {
            continue;
        }

        let mut state_changed = false;
        if result_record_state(proposal, undo).is_none() {
            let existing = existing_entries.get(&operation_id);
            let legacy_record = if let Some((generation, created_at)) = existing {
                completion_record(
                    SpeakerNameProposalResultRecordStatus::Recorded,
                    Some(*generation),
                    Some(created_at.clone()),
                    Some(created_at.clone()),
                )
            } else if undo
                || result_entry_was_pruned(
                    Some(&proposal.created_at),
                    config.retention.conversation_days,
                    now,
                )
            {
                completion_record(
                    SpeakerNameProposalResultRecordStatus::LegacyRecorded,
                    None,
                    None,
                    None,
                )
            } else {
                completion_record(
                    SpeakerNameProposalResultRecordStatus::Pending,
                    conversation_generation,
                    Some(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                    None,
                )
            };
            *result_record_state_mut(proposal, undo) = Some(legacy_record);
            state_changed = true;

            if !undo
                && proposal.undo_card_status == SpeakerNameProposalUndoCardStatus::LegacyUnknown
            {
                proposal.undo_card_status = match proposal
                    .completion_record
                    .as_ref()
                    .map(|record| record.status)
                {
                    Some(SpeakerNameProposalResultRecordStatus::Pending)
                        if proposal.confirmation_bubble_id.is_some() =>
                    {
                        SpeakerNameProposalUndoCardStatus::Pending
                    }
                    _ if proposal.confirmation_bubble_id.is_none() => {
                        SpeakerNameProposalUndoCardStatus::Dismissed
                    }
                    _ => SpeakerNameProposalUndoCardStatus::Delivered,
                };
            }
        }

        if result_record_state(proposal, undo)
            .is_some_and(|record| record.status == SpeakerNameProposalResultRecordStatus::Pending)
        {
            let mut record_state = result_record_state(proposal, undo)
                .expect("pending result record exists")
                .clone();
            if record_state.conversation_generation.is_none() {
                record_state.conversation_generation = conversation_generation;
                if record_state.entry_created_at.is_none() {
                    record_state.entry_created_at =
                        Some(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
                }
                *result_record_state_mut(proposal, undo) = Some(record_state.clone());
                persist_result_record_state(paths, proposal, undo)?;
            }
            let existing = existing_entries.get(&operation_id);
            let next_state = if let Some((generation, created_at)) = existing {
                Some(completion_record(
                    SpeakerNameProposalResultRecordStatus::Recorded,
                    Some(*generation),
                    Some(created_at.clone()),
                    Some(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                ))
            } else if result_entry_was_pruned(
                record_state.entry_created_at.as_deref(),
                config.retention.conversation_days,
                now,
            ) {
                Some(completion_record(
                    SpeakerNameProposalResultRecordStatus::Pruned,
                    record_state.conversation_generation,
                    record_state.entry_created_at,
                    None,
                ))
            } else if let (Some(generation), Some(created_at)) = (
                record_state.conversation_generation,
                record_state.entry_created_at.as_deref(),
            ) {
                let appended = write_proposal_result_entry(ProposalResultEntryWrite {
                    paths,
                    config,
                    proposal,
                    operation_id: &operation_id,
                    undo,
                    conversation_generation: generation,
                    entry_created_at: created_at,
                    now,
                })?;
                conversation_record_appended |= appended;
                Some(completion_record(
                    SpeakerNameProposalResultRecordStatus::Recorded,
                    Some(generation),
                    Some(created_at.to_owned()),
                    Some(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                ))
            } else {
                None
            };
            if let Some(next_state) = next_state {
                *result_record_state_mut(proposal, undo) = Some(next_state);
                state_changed = true;
            }
        }

        if state_changed {
            persist_result_record_state(paths, proposal, undo)?;
        }

        if !undo {
            if proposal.can_undo_at(&now) {
                if proposal.undo_card_status == SpeakerNameProposalUndoCardStatus::LegacyUnknown {
                    proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Delivered;
                    persist_result_record_state(paths, proposal, false)?;
                }
                if matches!(
                    proposal.undo_card_status,
                    SpeakerNameProposalUndoCardStatus::Pending
                        | SpeakerNameProposalUndoCardStatus::Delivering
                ) {
                    let result_id = crate::speaker_name_proposal_presenter::result_id(proposal);
                    if proposal.confirmation_bubble_id.as_deref() != Some(result_id.as_str()) {
                        proposal.confirmation_bubble_id = Some(result_id);
                        update_speaker_name_proposals(paths, |store| {
                            if let Some(existing) = store.proposals.iter_mut().find(|item| {
                                item.id == proposal.id && item.version == proposal.version
                            }) {
                                existing.confirmation_bubble_id =
                                    proposal.confirmation_bubble_id.clone();
                            }
                            Ok(())
                        })
                        .map_err(|_| "話者名登録のカード状態を保存できません".to_owned())?;
                    }
                    if let Some(generation) = conversation_generation {
                        if let Some(record) = crate::speaker_name_proposal_presenter::result_record(
                            proposal, config, generation, &now,
                        ) {
                            display.push(SpeakerNameProposalBubbleDelivery {
                                proposal_id: proposal.id.clone(),
                                version: proposal.version,
                                undo_result: true,
                                record,
                                replaced_ids: Vec::new(),
                            });
                        }
                    }
                }
            } else if proposal.undo_card_status != SpeakerNameProposalUndoCardStatus::Expired {
                proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Expired;
                proposal.pending_bubble_dismissal_id = proposal.confirmation_bubble_id.clone();
                persist_result_record_state(paths, proposal, false)?;
            }
        } else if proposal.undo_card_status != SpeakerNameProposalUndoCardStatus::Resolved {
            proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Resolved;
            proposal.confirmation_bubble_id = None;
            update_speaker_name_proposals(paths, |store| {
                if let Some(existing) = store
                    .proposals
                    .iter_mut()
                    .find(|item| item.id == proposal.id && item.version == proposal.version)
                {
                    existing.undo_card_status = proposal.undo_card_status;
                    existing.confirmation_bubble_id = None;
                }
                Ok(())
            })
            .map_err(|_| "話者名登録のカード状態を保存できません".to_owned())?;
        }
    }

    Ok(ReconciledSpeakerNameProposalResults {
        conversation_record_appended,
        display,
    })
}

fn validate_edit_name(
    name: &str,
    current_name: Option<&str>,
    locale: Locale,
) -> Result<(), String> {
    if current_name != Some(name) {
        return Ok(());
    }
    Err(match locale {
        Locale::Ja => "現在の表示名と同じため、変更はありません。".to_owned(),
        Locale::En => "The name matches the current display name; nothing changed.".to_owned(),
    })
}

fn localize_proposal_action_error(error: String, locale: Locale) -> String {
    if locale == Locale::Ja {
        return error;
    }
    match error.as_str() {
        "この話者名登録の確認は期限切れです" => {
            return "This speaker-name confirmation has expired.".to_owned();
        }
        "話者名登録の取り消し期限が切れています" => {
            return "The speaker-name undo period has expired.".to_owned();
        }
        "話者名登録の操作が競合しました" | "話者名登録の取り消しが競合しました" =>
        {
            return "The speaker-name action conflicted with a newer change.".to_owned();
        }
        "話者名登録の根拠または対象が変わりました" => {
            return "The speaker-name evidence or target has changed.".to_owned();
        }
        _ => {}
    }
    if error.chars().any(is_japanese_error_character) {
        "The speaker-name action could not be completed.".to_owned()
    } else {
        error
    }
}

fn is_japanese_error_character(character: char) -> bool {
    matches!(
        character,
        '\u{3000}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}'
    )
}

fn mark_proposal_conflict(
    paths: &coosenpai_core::config::ConfigPaths,
    id: &str,
    version: u64,
) -> Result<(), String> {
    update_speaker_name_proposals(paths, |store| {
        if let Some(proposal) = store
            .proposals
            .iter_mut()
            .find(|proposal| proposal.id == id && proposal.version == version)
        {
            proposal.status = SpeakerNameProposalStatus::Conflict;
        }
        Ok(())
    })
    .map_err(|_| "話者名登録の状態を保存できません".to_owned())
}

fn proposal_for_action(
    paths: &coosenpai_core::config::ConfigPaths,
    action: &ParsedProposalAction,
    bubble_id: &str,
) -> Result<StoredSpeakerNameProposal, String> {
    let store = load_speaker_name_proposals(paths)
        .map_err(|_| "話者名登録の提案を読み込めません".to_owned())?;
    store
        .proposals
        .into_iter()
        .find(|proposal| {
            proposal.id == action.proposal_id
                && proposal.version == action.version
                && proposal.confirmation_bubble_id.as_deref() == Some(bubble_id)
        })
        .ok_or_else(|| "この話者名登録の確認は期限切れです".to_owned())
}

fn validate_proposal_for_action(
    state: &DesktopState,
    proposal: &StoredSpeakerNameProposal,
    directory: &HelperDirectory,
) -> Result<
    (
        String,
        coosenpai_core::speaker_id::PromptSpeakerIdResolver,
        BTreeMap<String, String>,
    ),
    String,
> {
    let (registry_id, names) = validate_proposal_registry_target(state, proposal, directory)?;
    let resolver = coosenpai_core::speaker_id::PromptSpeakerIdResolver::new(
        &registry_id,
        directory.aliases.clone(),
    );
    if !verify_stored_speaker_name_proposal(
        &state.paths,
        proposal,
        &current_user_messages(state)?,
        &resolver,
        &registry_id,
        chrono::Utc::now(),
    )
    .map_err(|_| "話者名登録の根拠を再確認できません".to_owned())?
    {
        return Err("話者名登録の根拠または対象が変わりました".to_owned());
    }
    Ok((registry_id, resolver, names))
}

fn validate_proposal_registry_target(
    state: &DesktopState,
    proposal: &StoredSpeakerNameProposal,
    directory: &HelperDirectory,
) -> Result<(String, BTreeMap<String, String>), String> {
    let registry_id = directory
        .registry_id
        .as_deref()
        .ok_or_else(|| "話者台帳がありません".to_owned())?;
    let names = load_speaker_names(&state.paths, registry_id)
        .map_err(|_| "話者の現在の表示名を確認できません".to_owned())?;
    if proposal.registry_id != registry_id
        || !directory
            .speakers
            .iter()
            .any(|id| id == &proposal.speaker_id)
    {
        return Err("話者台帳または話者が変わりました".to_owned());
    }
    Ok((registry_id.to_owned(), names))
}

async fn process_proposal_action(
    state: &DesktopState,
    bubble_id: String,
    parsed: ParsedProposalAction,
    value: Option<String>,
) -> Result<ProposalActionEffect, String> {
    let mut proposal = proposal_for_action(&state.paths, &parsed, &bubble_id)?;
    if parsed.kind == ProposalActionKind::Undo
        && proposal.status == SpeakerNameProposalStatus::Applied
        && proposal.undo_card_status == SpeakerNameProposalUndoCardStatus::Delivering
    {
        if !confirm_delivering_speaker_name_result_card(
            &state.ui,
            &state.paths,
            &proposal,
            &bubble_id,
        )
        .await?
        {
            return Err("この話者名の取り消しは利用できません".to_owned());
        }
        proposal = proposal_for_action(&state.paths, &parsed, &bubble_id)?;
    }
    let old_bubble_id = bubble_id;
    match parsed.kind {
        ProposalActionKind::Edit | ProposalActionKind::EditCancel => {
            if proposal.status != SpeakerNameProposalStatus::Proposed || value.is_some() {
                return Err("この話者名登録の確認は利用できません".to_owned());
            }
            let directory = load_helper_directory(state).await?;
            if proposal.registry_id != directory.registry_id.as_deref().unwrap_or_default()
                || validate_proposal_for_action(state, &proposal, &directory).is_err()
            {
                mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                return Err("話者名登録の根拠または対象が変わりました".to_owned());
            }
            if parsed.kind == ProposalActionKind::Edit {
                Ok(ProposalActionEffect::Edit(proposal))
            } else {
                Ok(ProposalActionEffect::EditCancelled(proposal))
            }
        }
        ProposalActionKind::EditSubmit => {
            if proposal.status != SpeakerNameProposalStatus::Proposed {
                return Err("この話者名登録の確認は利用できません".to_owned());
            }
            let name = value
                .as_deref()
                .ok_or_else(|| "登録する名前を入力してください".to_owned())
                .and_then(validate_speaker_name)?;
            if name == proposal.name {
                return Err("登録予定の名前を変更してください".to_owned());
            }
            validate_edit_name(
                &name,
                proposal.current_name.as_deref(),
                Locale::from_config(&state.runtime_config().ui.language),
            )?;
            let directory = load_helper_directory(state).await?;
            if proposal.registry_id != directory.registry_id.as_deref().unwrap_or_default()
                || validate_proposal_for_action(state, &proposal, &directory).is_err()
                || load_speaker_names(&state.paths, &proposal.registry_id)
                    .map_err(|_| "話者の現在の表示名を確認できません".to_owned())?
                    .get(&proposal.speaker_id)
                    .cloned()
                    != proposal.current_name
            {
                mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                return Err("話者名登録の根拠または対象が変わりました".to_owned());
            }
            proposal.name = name;
            proposal.name_edited_by_user = true;
            proposal.version = proposal
                .version
                .checked_add(1)
                .ok_or_else(|| "話者名登録の版数上限に達しました".to_owned())?;
            proposal.status = SpeakerNameProposalStatus::Proposed;
            proposal.operation = None;
            proposal.pending_bubble_dismissal_id = Some(old_bubble_id.clone());
            proposal.confirmation_bubble_id = Some(
                crate::speaker_name_proposal_presenter::confirmation_id(&proposal),
            );
            let updated = proposal.clone();
            update_speaker_name_proposals(&state.paths, |store| {
                let existing = store
                    .proposals
                    .iter_mut()
                    .find(|item| item.id == parsed.proposal_id && item.version == parsed.version)
                    .ok_or_else(|| {
                        coosenpai_core::persistence::PersistenceError::Invalid(
                            "話者名登録の提案が更新されました".to_owned(),
                        )
                    })?;
                if existing.status != SpeakerNameProposalStatus::Proposed
                    || existing.confirmation_bubble_id.as_deref() != Some(old_bubble_id.as_str())
                {
                    return Err(coosenpai_core::persistence::PersistenceError::Invalid(
                        "話者名登録の提案が更新されました".to_owned(),
                    ));
                }
                *existing = updated.clone();
                Ok(())
            })
            .map_err(|_| "話者名登録の提案を更新できません".to_owned())?;
            Ok(ProposalActionEffect::Edited { old_bubble_id })
        }
        ProposalActionKind::Reject => {
            if proposal.status != SpeakerNameProposalStatus::Proposed || value.is_some() {
                return Err("この話者名登録の確認は利用できません".to_owned());
            }
            update_speaker_name_proposals(&state.paths, |store| {
                let existing = store
                    .proposals
                    .iter_mut()
                    .find(|item| item.id == parsed.proposal_id && item.version == parsed.version)
                    .ok_or_else(|| {
                        coosenpai_core::persistence::PersistenceError::Invalid(
                            "話者名登録の提案が更新されました".to_owned(),
                        )
                    })?;
                if existing.confirmation_bubble_id.as_deref() != Some(old_bubble_id.as_str())
                    || existing.status != SpeakerNameProposalStatus::Proposed
                {
                    return Err(coosenpai_core::persistence::PersistenceError::Invalid(
                        "話者名登録の提案が更新されました".to_owned(),
                    ));
                }
                existing.status = SpeakerNameProposalStatus::Rejected;
                existing.confirmation_bubble_id = None;
                existing.pending_bubble_dismissal_id = Some(old_bubble_id.clone());
                if existing.source
                    == coosenpai_core::speaker_name_proposals::SpeakerNameProposalSource::Inferred
                {
                    existing.suppressed_until = Some(
                        (chrono::Utc::now()
                            + coosenpai_core::speaker_name_proposals::INFERENCE_REJECTION_TTL)
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    );
                }
                Ok(())
            })
            .map_err(|_| "話者名登録の提案を却下できません".to_owned())?;
            Ok(ProposalActionEffect::Rejected { old_bubble_id })
        }
        ProposalActionKind::Register => {
            if value.is_some()
                || !matches!(
                    proposal.status,
                    SpeakerNameProposalStatus::Proposed | SpeakerNameProposalStatus::Applying
                )
            {
                return Err("この話者名登録の確認は利用できません".to_owned());
            }
            let directory = load_helper_directory(state).await?;
            if proposal.registry_id != directory.registry_id.as_deref().unwrap_or_default() {
                mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                return Err("話者台帳が変わったため登録できません".to_owned());
            }
            let (registry_id, _, names) =
                match validate_proposal_for_action(state, &proposal, &directory) {
                    Ok(context) => context,
                    Err(error) => {
                        mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                        return Err(error);
                    }
                };
            let current_name = names.get(&proposal.speaker_id).cloned();
            let mut operation = proposal.operation.clone();
            if proposal.status == SpeakerNameProposalStatus::Proposed {
                if current_name != proposal.current_name {
                    mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                    return Err("話者の表示名が変わったため登録できません".to_owned());
                }
                validate_edit_name(
                    &proposal.name,
                    current_name.as_deref(),
                    Locale::from_config(&state.runtime_config().ui.language),
                )?;
                if let Some(existing) = &operation {
                    if existing.after_name != proposal.name || existing.before_name != current_name
                    {
                        mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                        return Err("話者名登録の操作が競合しました".to_owned());
                    }
                } else {
                    operation = Some(SpeakerNameApplyOperation {
                        id: uuid::Uuid::new_v4().to_string(),
                        before_name: current_name.clone(),
                        after_name: proposal.name.clone(),
                        undo_id: None,
                    });
                }
            }
            let operation =
                operation.ok_or_else(|| "話者名登録の操作記録がありません".to_owned())?;
            if current_name.as_deref() == Some(operation.after_name.as_str()) {
                proposal.status = SpeakerNameProposalStatus::Applied;
                proposal.operation = Some(operation.clone());
                proposal.confirmation_bubble_id =
                    Some(crate::speaker_name_proposal_presenter::result_id(&proposal));
            } else if current_name == operation.before_name {
                proposal.status = SpeakerNameProposalStatus::Applying;
                proposal.operation = Some(operation.clone());
                update_speaker_name_proposals(&state.paths, |store| {
                    let existing = store
                        .proposals
                        .iter_mut()
                        .find(|item| {
                            item.id == parsed.proposal_id && item.version == parsed.version
                        })
                        .ok_or_else(|| {
                            coosenpai_core::persistence::PersistenceError::Invalid(
                                "話者名登録の提案が更新されました".to_owned(),
                            )
                        })?;
                    if existing.confirmation_bubble_id.as_deref() != Some(&old_bubble_id)
                        || !matches!(
                            existing.status,
                            SpeakerNameProposalStatus::Proposed
                                | SpeakerNameProposalStatus::Applying
                        )
                    {
                        return Err(coosenpai_core::persistence::PersistenceError::Invalid(
                            "話者名登録の提案が更新されました".to_owned(),
                        ));
                    }
                    existing.status = SpeakerNameProposalStatus::Applying;
                    existing.operation = Some(operation.clone());
                    Ok(())
                })
                .map_err(|_| "話者名登録の操作を開始できません".to_owned())?;
                save_speaker_name(
                    &state.paths,
                    &registry_id,
                    &proposal.speaker_id,
                    Some(&operation.after_name),
                )
                .map_err(|_| "話者名を保存できません".to_owned())?;
                proposal.status = SpeakerNameProposalStatus::Applied;
                proposal.operation = Some(operation.clone());
                proposal.confirmation_bubble_id =
                    Some(crate::speaker_name_proposal_presenter::result_id(&proposal));
            } else {
                mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                return Err("話者の表示名が変わったため登録できません".to_owned());
            }
            let applied_at = chrono::Utc::now();
            let conversation_generation = crate::bubbles::conversation_generation(state).await.ok();
            proposal.pending_bubble_dismissal_id = Some(old_bubble_id.clone());
            initialize_applied_result_state(&mut proposal, conversation_generation, applied_at);
            let finalized = proposal.clone();
            update_speaker_name_proposals(&state.paths, |store| {
                let existing = store
                    .proposals
                    .iter_mut()
                    .find(|item| item.id == parsed.proposal_id && item.version == parsed.version)
                    .ok_or_else(|| {
                        coosenpai_core::persistence::PersistenceError::Invalid(
                            "話者名登録の提案が更新されました".to_owned(),
                        )
                    })?;
                if existing.operation.as_ref().map(|op| op.id.as_str()) != Some(&operation.id)
                    || !matches!(
                        existing.status,
                        SpeakerNameProposalStatus::Applying | SpeakerNameProposalStatus::Applied
                    )
                {
                    return Err(coosenpai_core::persistence::PersistenceError::Invalid(
                        "話者名登録の操作が競合しました".to_owned(),
                    ));
                }
                *existing = finalized;
                Ok(())
            })
            .map_err(|_| "話者名登録の結果を保存できません".to_owned())?;
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::SpeakerDirectoryChanged)
                .await;
            Ok(ProposalActionEffect::Applied { old_bubble_id })
        }
        ProposalActionKind::Undo => {
            if value.is_some()
                || !matches!(
                    proposal.status,
                    SpeakerNameProposalStatus::Applied | SpeakerNameProposalStatus::Undoing
                )
            {
                return Err("この話者名の取り消しは利用できません".to_owned());
            }
            if proposal.status == SpeakerNameProposalStatus::Applied
                && !proposal.can_undo_at(&chrono::Utc::now())
            {
                mark_result_card_expired(
                    &state.paths,
                    &parsed.proposal_id,
                    parsed.version,
                    &old_bubble_id,
                    chrono::Utc::now(),
                )?;
                return Ok(ProposalActionEffect::Expired { old_bubble_id });
            }
            if proposal.status == SpeakerNameProposalStatus::Applied
                && !matches!(
                    proposal.undo_card_status,
                    SpeakerNameProposalUndoCardStatus::LegacyUnknown
                        | SpeakerNameProposalUndoCardStatus::Pending
                        | SpeakerNameProposalUndoCardStatus::Delivered
                )
            {
                return Err("この話者名の取り消しは利用できません".to_owned());
            }
            let directory = load_helper_directory(state).await?;
            if proposal.registry_id != directory.registry_id.as_deref().unwrap_or_default() {
                mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                return Err("話者台帳が変わったため取り消せません".to_owned());
            }
            let (registry_id, names) =
                match validate_proposal_registry_target(state, &proposal, &directory) {
                    Ok(context) => context,
                    Err(error) => {
                        mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                        return Err(error);
                    }
                };
            let mut operation = proposal
                .operation
                .clone()
                .ok_or_else(|| "話者名登録の操作記録がありません".to_owned())?;
            let current_name = names.get(&proposal.speaker_id).cloned();
            let should_restore = current_name.as_deref() == Some(operation.after_name.as_str());
            let already_restored = current_name == operation.before_name;
            if !should_restore && !(already_restored && operation.undo_id.is_some()) {
                mark_proposal_conflict(&state.paths, &proposal.id, proposal.version)?;
                return Err("話者の表示名が変わったため取り消せません".to_owned());
            }
            if should_restore && operation.undo_id.is_none() {
                operation.undo_id = Some(uuid::Uuid::new_v4().to_string());
            }
            let undo_at = chrono::Utc::now();
            let conversation_generation = crate::bubbles::conversation_generation(state).await.ok();
            if proposal.undo_completion_record.is_none() {
                proposal.undo_completion_record = Some(completion_record(
                    SpeakerNameProposalResultRecordStatus::Pending,
                    conversation_generation,
                    Some(undo_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                    None,
                ));
            }
            proposal.operation = Some(operation.clone());
            proposal.status = SpeakerNameProposalStatus::Undoing;
            proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Resolved;
            proposal.pending_bubble_dismissal_id = Some(old_bubble_id.clone());
            proposal.confirmation_bubble_id = None;
            let undoing = proposal.clone();
            update_speaker_name_proposals(&state.paths, |store| {
                let existing = store
                    .proposals
                    .iter_mut()
                    .find(|item| item.id == parsed.proposal_id && item.version == parsed.version)
                    .ok_or_else(|| {
                        coosenpai_core::persistence::PersistenceError::Invalid(
                            "話者名登録の提案が更新されました".to_owned(),
                        )
                    })?;
                let existing_undo_id = existing
                    .operation
                    .as_ref()
                    .and_then(|item| item.undo_id.as_deref());
                if existing.confirmation_bubble_id.as_deref() != Some(old_bubble_id.as_str())
                    || !matches!(
                        existing.status,
                        SpeakerNameProposalStatus::Applied | SpeakerNameProposalStatus::Undoing
                    )
                    || existing_undo_id.is_some()
                        && existing_undo_id != operation.undo_id.as_deref()
                {
                    return Err(coosenpai_core::persistence::PersistenceError::Invalid(
                        "話者名登録の取り消しが競合しました".to_owned(),
                    ));
                }
                *existing = undoing.clone();
                Ok(())
            })
            .map_err(|_| "話者名の取り消しを開始できません".to_owned())?;
            if should_restore {
                save_speaker_name(
                    &state.paths,
                    &registry_id,
                    &proposal.speaker_id,
                    operation.before_name.as_deref(),
                )
                .map_err(|_| "前の話者名に戻せません".to_owned())?;
            }
            proposal.status = SpeakerNameProposalStatus::Undone;
            proposal.confirmation_bubble_id = None;
            let finalized = proposal.clone();
            update_speaker_name_proposals(&state.paths, |store| {
                let existing = store
                    .proposals
                    .iter_mut()
                    .find(|item| item.id == parsed.proposal_id && item.version == parsed.version)
                    .ok_or_else(|| {
                        coosenpai_core::persistence::PersistenceError::Invalid(
                            "話者名登録の提案が更新されました".to_owned(),
                        )
                    })?;
                if existing.status != SpeakerNameProposalStatus::Undoing
                    || existing
                        .operation
                        .as_ref()
                        .and_then(|op| op.undo_id.as_deref())
                        != operation.undo_id.as_deref()
                {
                    return Err(coosenpai_core::persistence::PersistenceError::Invalid(
                        "話者名登録の取り消しが競合しました".to_owned(),
                    ));
                }
                *existing = finalized;
                Ok(())
            })
            .map_err(|_| "話者名の取り消し結果を保存できません".to_owned())?;
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::SpeakerDirectoryChanged)
                .await;
            Ok(ProposalActionEffect::Undone { old_bubble_id })
        }
    }
}

pub(crate) async fn handle_speaker_name_proposal_action(
    state: Arc<DesktopState>,
    bubble_id: &str,
    action: &str,
    value: Option<&str>,
) -> Result<(), String> {
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let parsed = parse_proposal_action(action).ok_or_else(|| {
        localize_proposal_action_error("話者名登録の操作が不正です".to_owned(), locale)
    })?;
    let queue = state.speaker_queue.clone();
    let queued_state = state.clone();
    let queued_bubble_id = bubble_id.to_owned();
    let queued_value = value.map(str::to_owned);
    let proposal_id = parsed.proposal_id.clone();
    let proposal_version = parsed.version;
    let effect = match queue
        .enqueue(move || async move {
            process_proposal_action(&queued_state, queued_bubble_id, parsed, queued_value).await
        })
        .await
    {
        Ok(effect) => effect,
        Err(error) => {
            let _ = recover_speaker_proposal_card_after_action_error(
                state.speaker_queue.clone(),
                state.ui.clone(),
                state.paths.clone(),
                proposal_id,
                proposal_version,
                bubble_id.to_owned(),
            )
            .await;
            return Err(localize_proposal_action_error(error, locale));
        }
    };
    match effect {
        ProposalActionEffect::Edit(proposal) => {
            crate::bubbles::mutate(
                &state,
                crate::bubbles::BubbleMutation::SetInteraction {
                    id: bubble_id.to_owned(),
                    interaction: Some(Box::new(
                        crate::speaker_name_proposal_presenter::edit_interaction(
                            &proposal,
                            &state.runtime_config(),
                        ),
                    )),
                },
            )
            .await;
        }
        ProposalActionEffect::EditCancelled(proposal) => {
            crate::bubbles::mutate(
                &state,
                crate::bubbles::BubbleMutation::SetInteraction {
                    id: bubble_id.to_owned(),
                    interaction: Some(Box::new(
                        crate::speaker_name_proposal_presenter::confirmation_interaction_for_action(
                            &proposal,
                            Locale::from_config(&state.runtime_config().ui.language),
                        ),
                    )),
                },
            )
            .await;
        }
        ProposalActionEffect::Applied { old_bubble_id } => {
            dismiss_speaker_name_proposal_bubble(&state, &old_bubble_id).await;
            state.refresh_conversation().await;
            sync_speaker_name_proposal_bubbles(state.clone()).await;
        }
        ProposalActionEffect::Expired { old_bubble_id } => {
            dismiss_speaker_name_proposal_bubble(&state, &old_bubble_id).await;
            return Err(localize_proposal_action_error(
                "話者名登録の取り消し期限が切れています".to_owned(),
                locale,
            ));
        }
        ProposalActionEffect::Edited { old_bubble_id }
        | ProposalActionEffect::Undone { old_bubble_id } => {
            dismiss_speaker_name_proposal_bubble(&state, &old_bubble_id).await;
            state.refresh_conversation().await;
            sync_speaker_name_proposal_bubbles(state.clone()).await;
        }
        ProposalActionEffect::Rejected { old_bubble_id } => {
            dismiss_speaker_name_proposal_bubble(&state, &old_bubble_id).await;
        }
    }
    Ok(())
}

pub(crate) fn schedule_speaker_name_proposal_bubble_sync(state: Arc<DesktopState>) {
    if !state.paths.speaker_name_proposals.exists() {
        return;
    }
    tokio::spawn(async move {
        sync_speaker_name_proposal_bubbles(state).await;
    });
}

fn mark_result_card_delivering(
    paths: &coosenpai_core::config::ConfigPaths,
    proposal_id: &str,
    version: u64,
    bubble_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ResultCardDeliveryStart, String> {
    update_speaker_name_proposals(paths, |store| {
        let Some(proposal) = store
            .proposals
            .iter_mut()
            .find(|proposal| proposal.id == proposal_id && proposal.version == version)
        else {
            return Ok(ResultCardDeliveryStart::Stale);
        };
        if proposal.status != SpeakerNameProposalStatus::Applied
            || proposal.confirmation_bubble_id.as_deref() != Some(bubble_id)
        {
            if proposal.pending_bubble_dismissal_id.is_none() {
                proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
            }
            return Ok(ResultCardDeliveryStart::Stale);
        }
        if !proposal.can_undo_at(&now) {
            proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Expired;
            proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
            return Ok(ResultCardDeliveryStart::Stale);
        }
        match proposal.undo_card_status {
            SpeakerNameProposalUndoCardStatus::Pending => {
                proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Delivering;
                Ok(ResultCardDeliveryStart::Deliver)
            }
            SpeakerNameProposalUndoCardStatus::Delivering => {
                Ok(ResultCardDeliveryStart::ConfirmOnly)
            }
            SpeakerNameProposalUndoCardStatus::Delivered => {
                Ok(ResultCardDeliveryStart::AlreadyDelivered)
            }
            _ => {
                if proposal.pending_bubble_dismissal_id.is_none() {
                    proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
                }
                Ok(ResultCardDeliveryStart::Stale)
            }
        }
    })
    .map_err(|_| "話者名登録カードの配送状態を保存できません".to_owned())
}

fn mark_result_card_delivered(
    paths: &coosenpai_core::config::ConfigPaths,
    proposal_id: &str,
    version: u64,
    bubble_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ResultCardDeliveryCommit, String> {
    update_speaker_name_proposals(paths, |store| {
        let Some(proposal) = store
            .proposals
            .iter_mut()
            .find(|proposal| proposal.id == proposal_id && proposal.version == version)
        else {
            return Ok(ResultCardDeliveryCommit::Stale);
        };
        if proposal.status != SpeakerNameProposalStatus::Applied
            || proposal.confirmation_bubble_id.as_deref() != Some(bubble_id)
        {
            if proposal.pending_bubble_dismissal_id.is_none() {
                proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
            }
            return Ok(ResultCardDeliveryCommit::Stale);
        }
        if !proposal.can_undo_at(&now) {
            proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Expired;
            proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
            return Ok(ResultCardDeliveryCommit::Expired);
        }
        match proposal.undo_card_status {
            SpeakerNameProposalUndoCardStatus::Pending
            | SpeakerNameProposalUndoCardStatus::Delivering => {
                proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Delivered;
                Ok(ResultCardDeliveryCommit::Delivered)
            }
            SpeakerNameProposalUndoCardStatus::Delivered => {
                Ok(ResultCardDeliveryCommit::AlreadyDelivered)
            }
            _ => {
                if proposal.pending_bubble_dismissal_id.is_none() {
                    proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
                }
                Ok(ResultCardDeliveryCommit::Stale)
            }
        }
    })
    .map_err(|_| "話者名登録カードの配送状態を保存できません".to_owned())
}

async fn confirm_delivering_speaker_name_result_card(
    ui: &crate::ui_root::UiHandle,
    paths: &coosenpai_core::config::ConfigPaths,
    proposal: &StoredSpeakerNameProposal,
    bubble_id: &str,
) -> Result<bool, String> {
    let root_has_card = root_contains_speaker_name_bubble(ui, bubble_id).await?;
    let commit = mark_result_card_delivered(
        paths,
        &proposal.id,
        proposal.version,
        bubble_id,
        chrono::Utc::now(),
    )?;
    match commit {
        ResultCardDeliveryCommit::Delivered | ResultCardDeliveryCommit::AlreadyDelivered => {
            Ok(root_has_card)
        }
        ResultCardDeliveryCommit::Expired => {
            dismiss_speaker_name_proposal_bubble_from_root(ui, paths, bubble_id).await?;
            Ok(false)
        }
        ResultCardDeliveryCommit::Stale => Ok(false),
    }
}

fn mark_result_card_dismissed_at(
    paths: &coosenpai_core::config::ConfigPaths,
    bubble_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, String> {
    update_speaker_name_proposals(paths, |store| {
        if let Some(proposal) = store.proposals.iter_mut().find(|proposal| {
            proposal.status == SpeakerNameProposalStatus::Applied
                && proposal.confirmation_bubble_id.as_deref() == Some(bubble_id)
        }) {
            if !matches!(
                proposal.undo_card_status,
                SpeakerNameProposalUndoCardStatus::Dismissed
                    | SpeakerNameProposalUndoCardStatus::Expired
                    | SpeakerNameProposalUndoCardStatus::Resolved
            ) {
                proposal.undo_card_status = if proposal.can_undo_at(&now) {
                    SpeakerNameProposalUndoCardStatus::Dismissed
                } else {
                    SpeakerNameProposalUndoCardStatus::Expired
                };
            }
            proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
            return Ok(true);
        }
        Ok(false)
    })
    .map_err(|_| "話者名登録カードの解除状態を保存できません".to_owned())
}

fn mark_result_card_expired(
    paths: &coosenpai_core::config::ConfigPaths,
    proposal_id: &str,
    version: u64,
    bubble_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, String> {
    update_speaker_name_proposals(paths, |store| {
        let Some(proposal) = store.proposals.iter_mut().find(|proposal| {
            proposal.id == proposal_id
                && proposal.version == version
                && proposal.status == SpeakerNameProposalStatus::Applied
                && proposal.confirmation_bubble_id.as_deref() == Some(bubble_id)
        }) else {
            return Ok(false);
        };
        if proposal.can_undo_at(&now) {
            return Ok(false);
        }
        proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Expired;
        proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
        Ok(true)
    })
    .map_err(|_| "話者名登録の期限状態を保存できません".to_owned())
}

pub(crate) async fn mark_speaker_name_proposal_card_dismissed(
    state: Arc<DesktopState>,
    bubble_id: String,
) -> Result<(), String> {
    let queue = state.speaker_queue.clone();
    let queued_state = state.clone();
    queue
        .enqueue(move || async move {
            mark_result_card_dismissed_at(&queued_state.paths, &bubble_id, chrono::Utc::now())?;
            dismiss_speaker_name_proposal_bubble_from_root(
                &queued_state.ui,
                &queued_state.paths,
                &bubble_id,
            )
            .await
        })
        .await
}

fn schedule_speaker_name_proposal_expiry(
    state: Arc<DesktopState>,
    proposal_id: String,
    version: u64,
    expires_at: String,
) {
    static SCHEDULED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashSet<(String, u64)>>,
    > = std::sync::OnceLock::new();
    let scheduled = SCHEDULED.get_or_init(Default::default);
    let key = (proposal_id, version);
    if !scheduled
        .lock()
        .is_ok_and(|mut scheduled| scheduled.insert(key.clone()))
    {
        return;
    }
    let delay = chrono::DateTime::parse_from_rfc3339(&expires_at)
        .ok()
        .map(|expires_at| {
            expires_at
                .with_timezone(&chrono::Utc)
                .signed_duration_since(chrono::Utc::now())
                .to_std()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    tokio::spawn(async move {
        let should_sync = tokio::select! {
            () = state.cancellation.cancelled() => false,
            () = tokio::time::sleep(delay) => true,
        };
        if should_sync {
            sync_speaker_name_proposal_bubbles(state).await;
        }
        if let Ok(mut scheduled) = SCHEDULED.get_or_init(Default::default).lock() {
            scheduled.remove(&key);
        }
    });
}

async fn sync_speaker_name_proposal_bubbles(state: Arc<DesktopState>) {
    let register_state = state.clone();
    let prepare_state = state.clone();
    let queue = state.speaker_queue.clone();
    let duration_ms = state.runtime_config().notification.bubble_duration_ms;
    let prepare = move || async move { sync_speaker_name_proposals(&prepare_state).await };
    let register = move |record, duration_ms| {
        let state = register_state.clone();
        async move { crate::bubbles::show_best_effort(state, record, duration_ms).await }
    };
    let _ = run_speaker_name_proposal_bubble_sync(
        queue,
        state.ui.clone(),
        state.paths.clone(),
        duration_ms,
        prepare,
        register,
    )
    .await;
    schedule_delivered_speaker_name_proposal_expiries(state);
}

async fn run_speaker_name_proposal_bubble_sync<Prepare, PrepareFuture, Register, RegisterFuture>(
    queue: SpeakerManagementQueue,
    ui: crate::ui_root::UiHandle,
    paths: coosenpai_core::config::ConfigPaths,
    duration_ms: u64,
    prepare: Prepare,
    register: Register,
) -> Result<(), String>
where
    Prepare: FnOnce() -> PrepareFuture + Send + 'static,
    PrepareFuture:
        Future<Output = Result<Vec<SpeakerNameProposalBubbleDelivery>, String>> + Send + 'static,
    Register: Fn(crate::bubbles::BubbleRecord, u64) -> RegisterFuture + Send + Sync + 'static,
    RegisterFuture: Future<Output = bool> + Send + 'static,
{
    queue
        .enqueue(move || async move {
            reconcile_pending_speaker_name_bubble_dismissals(&ui, &paths).await?;
            let records = prepare().await?;
            for delivery in records {
                for old_id in &delivery.replaced_ids {
                    if old_id != &delivery.record.id {
                        dismiss_speaker_name_proposal_bubble_from_root(&ui, &paths, old_id).await?;
                    }
                }
                let delivery_start = if delivery.undo_result {
                    match mark_result_card_delivering(
                        &paths,
                        &delivery.proposal_id,
                        delivery.version,
                        &delivery.record.id,
                        chrono::Utc::now(),
                    )? {
                        ResultCardDeliveryStart::AlreadyDelivered
                        | ResultCardDeliveryStart::Stale => continue,
                        start => Some(start),
                    }
                } else {
                    None
                };

                let root_has_card =
                    root_contains_speaker_name_bubble(&ui, &delivery.record.id).await?;
                if delivery_start == Some(ResultCardDeliveryStart::ConfirmOnly) {
                    // Delivering is an ambiguous crash boundary. Commit it without retrying so
                    // an accepted card cannot reappear after Root has restarted empty.
                    match mark_result_card_delivered(
                        &paths,
                        &delivery.proposal_id,
                        delivery.version,
                        &delivery.record.id,
                        chrono::Utc::now(),
                    )? {
                        ResultCardDeliveryCommit::Expired => {
                            dismiss_speaker_name_proposal_bubble_from_root(
                                &ui,
                                &paths,
                                &delivery.record.id,
                            )
                            .await?;
                        }
                        ResultCardDeliveryCommit::Delivered
                        | ResultCardDeliveryCommit::AlreadyDelivered
                        | ResultCardDeliveryCommit::Stale => {}
                    }
                    continue;
                }

                if root_has_card {
                    if delivery.undo_result {
                        mark_result_card_delivered(
                            &paths,
                            &delivery.proposal_id,
                            delivery.version,
                            &delivery.record.id,
                            chrono::Utc::now(),
                        )?;
                    }
                    continue;
                }

                if !register(delivery.record.clone(), duration_ms).await || !delivery.undo_result {
                    continue;
                }
                match mark_result_card_delivered(
                    &paths,
                    &delivery.proposal_id,
                    delivery.version,
                    &delivery.record.id,
                    chrono::Utc::now(),
                )? {
                    ResultCardDeliveryCommit::Delivered
                    | ResultCardDeliveryCommit::AlreadyDelivered
                    | ResultCardDeliveryCommit::Stale => {}
                    ResultCardDeliveryCommit::Expired => {
                        dismiss_speaker_name_proposal_bubble_from_root(
                            &ui,
                            &paths,
                            &delivery.record.id,
                        )
                        .await?;
                    }
                }
            }
            reconcile_pending_speaker_name_bubble_dismissals(&ui, &paths).await
        })
        .await
}

fn schedule_delivered_speaker_name_proposal_expiries(state: Arc<DesktopState>) {
    let Ok(store) = load_speaker_name_proposals(&state.paths) else {
        return;
    };
    let now = chrono::Utc::now();
    for proposal in store.proposals {
        if proposal.status == SpeakerNameProposalStatus::Applied
            && proposal.undo_card_status == SpeakerNameProposalUndoCardStatus::Delivered
            && proposal.can_undo_at(&now)
        {
            schedule_speaker_name_proposal_expiry(
                state.clone(),
                proposal.id,
                proposal.version,
                proposal.expires_at,
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyingProposalRecovery {
    Applied,
    Conflict,
}

fn recover_applying_proposal(
    paths: &coosenpai_core::config::ConfigPaths,
    registry_id: &str,
    proposal: &mut StoredSpeakerNameProposal,
    current_name: Option<&str>,
) -> Result<ApplyingProposalRecovery, coosenpai_core::persistence::PersistenceError> {
    let Some(operation) = proposal.operation.as_ref() else {
        proposal.status = SpeakerNameProposalStatus::Conflict;
        return Ok(ApplyingProposalRecovery::Conflict);
    };
    let operation = operation.clone();
    if current_name == Some(operation.after_name.as_str()) {
        // 名前の保存後に停止した場合は同じ値を書き直さず、記録だけを確定する。
    } else if current_name == operation.before_name.as_deref() {
        save_speaker_name(
            paths,
            registry_id,
            &proposal.speaker_id,
            Some(&operation.after_name),
        )?;
    } else {
        proposal.status = SpeakerNameProposalStatus::Conflict;
        return Ok(ApplyingProposalRecovery::Conflict);
    }
    proposal.status = SpeakerNameProposalStatus::Applied;
    proposal.confirmation_bubble_id =
        Some(crate::speaker_name_proposal_presenter::result_id(proposal));
    Ok(ApplyingProposalRecovery::Applied)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UndoingProposalRecovery {
    Undone,
    Conflict,
}

fn recover_undoing_proposal(
    paths: &coosenpai_core::config::ConfigPaths,
    registry_id: &str,
    proposal: &mut StoredSpeakerNameProposal,
    current_name: Option<&str>,
) -> Result<UndoingProposalRecovery, coosenpai_core::persistence::PersistenceError> {
    let Some(operation) = proposal.operation.clone() else {
        proposal.status = SpeakerNameProposalStatus::Conflict;
        return Ok(UndoingProposalRecovery::Conflict);
    };
    if current_name == operation.before_name.as_deref() && operation.undo_id.is_some() {
    } else if current_name == Some(operation.after_name.as_str()) {
        if operation.undo_id.is_none() {
            proposal.status = SpeakerNameProposalStatus::Conflict;
            return Ok(UndoingProposalRecovery::Conflict);
        }
        save_speaker_name(
            paths,
            registry_id,
            &proposal.speaker_id,
            operation.before_name.as_deref(),
        )?;
    } else {
        proposal.status = SpeakerNameProposalStatus::Conflict;
        return Ok(UndoingProposalRecovery::Conflict);
    }
    proposal.status = SpeakerNameProposalStatus::Undone;
    proposal.confirmation_bubble_id = None;
    proposal.operation = Some(operation);
    Ok(UndoingProposalRecovery::Undone)
}

async fn sync_speaker_name_proposals(
    state: &DesktopState,
) -> Result<Vec<SpeakerNameProposalBubbleDelivery>, String> {
    let mut store = load_speaker_name_proposals(&state.paths)
        .map_err(|_| "話者名登録の提案を読み込めません".to_owned())?;
    if store.proposals.is_empty() {
        return Ok(Vec::new());
    }
    let expected_proposals = store.proposals.clone();
    let existing_bubble_ids = speaker_name_proposal_confirmation_bubble_ids(&store.proposals);
    let directory = load_helper_directory(state).await?;
    let Some(registry_id) = directory.registry_id.as_deref() else {
        let obsolete_bubble_ids = existing_bubble_ids.clone();
        for proposal in &mut store.proposals {
            if matches!(
                proposal.status,
                SpeakerNameProposalStatus::Proposed
                    | SpeakerNameProposalStatus::Applying
                    | SpeakerNameProposalStatus::Undoing
                    | SpeakerNameProposalStatus::Applied
            ) {
                proposal.status = SpeakerNameProposalStatus::Conflict;
                proposal.pending_bubble_dismissal_id = proposal.confirmation_bubble_id.clone();
                proposal.confirmation_bubble_id = None;
            }
        }
        for proposal in &mut store.proposals {
            if proposal.pending_bubble_dismissal_id.is_none() {
                proposal.pending_bubble_dismissal_id = proposal.confirmation_bubble_id.clone();
            }
        }
        update_speaker_name_proposals(&state.paths, |latest| {
            replace_unchanged_speaker_name_proposals(latest, &expected_proposals, &store.proposals);
            Ok(())
        })
        .map_err(|_| "話者名登録の状態を保存できません".to_owned())?;
        for id in obsolete_bubble_ids {
            dismiss_speaker_name_proposal_bubble_from_root(&state.ui, &state.paths, &id).await?;
        }
        return Ok(Vec::new());
    };
    let resolver = coosenpai_core::speaker_id::PromptSpeakerIdResolver::new(
        registry_id,
        directory.aliases.clone(),
    );
    let mut names = load_speaker_names(&state.paths, registry_id)
        .map_err(|_| "話者の現在の表示名を確認できません".to_owned())?;
    let user_messages = current_user_messages(state)?;
    let user_ids = user_messages
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let now = chrono::Utc::now();
    let conversation_generation = crate::bubbles::conversation_generation(state).await.ok();
    let mut updated = Vec::new();
    let mut recovered_directory_change = false;
    let mut recovered_conversation_record = false;
    let mut display = Vec::new();

    for proposal in &mut store.proposals {
        if proposal.status != SpeakerNameProposalStatus::Proposed
            && !proposal.status.has_unresolved_operation()
            && proposal.status != SpeakerNameProposalStatus::Applied
        {
            continue;
        }
        let previous_bubble_id = proposal.confirmation_bubble_id.clone();
        let expired = chrono::DateTime::parse_from_rfc3339(&proposal.expires_at)
            .ok()
            .is_none_or(|expires_at| expires_at.with_timezone(&chrono::Utc) <= now);
        if expired && proposal.status == SpeakerNameProposalStatus::Proposed {
            proposal.status = SpeakerNameProposalStatus::Expired;
            proposal.confirmation_bubble_id = None;
            updated.push(proposal.clone());
            continue;
        }
        if proposal.status == SpeakerNameProposalStatus::Proposed
            && !proposal
                .source_user_message_ids
                .iter()
                .all(|source_id| user_ids.contains(source_id.as_str()))
        {
            continue;
        }
        let current_name = names.get(&proposal.speaker_id).cloned();
        let identity_valid = proposal.registry_id == registry_id
            && directory
                .speakers
                .iter()
                .any(|id| id == &proposal.speaker_id);
        let evidence_valid = identity_valid
            && (proposal.status != SpeakerNameProposalStatus::Proposed
                || verify_stored_speaker_name_proposal(
                    &state.paths,
                    proposal,
                    &user_messages,
                    &resolver,
                    registry_id,
                    now,
                )
                .map_err(|_| "話者名登録の根拠を再確認できません".to_owned())?);
        if !evidence_valid {
            proposal.status = SpeakerNameProposalStatus::Conflict;
            proposal.confirmation_bubble_id = None;
            updated.push(proposal.clone());
            continue;
        }

        match proposal.status {
            SpeakerNameProposalStatus::Proposed => {
                if current_name != proposal.current_name {
                    proposal.status = SpeakerNameProposalStatus::Conflict;
                    proposal.confirmation_bubble_id = None;
                    updated.push(proposal.clone());
                    continue;
                }
                proposal.confirmation_bubble_id = Some(
                    crate::speaker_name_proposal_presenter::confirmation_id(proposal),
                );
                updated.push(proposal.clone());
                if let Some(generation) = conversation_generation {
                    display.push(SpeakerNameProposalBubbleDelivery {
                        proposal_id: proposal.id.clone(),
                        version: proposal.version,
                        undo_result: false,
                        record: crate::speaker_name_proposal_presenter::confirmation_record(
                            proposal,
                            &state.runtime_config(),
                            generation,
                        ),
                        replaced_ids: previous_bubble_id.into_iter().collect(),
                    });
                }
            }
            SpeakerNameProposalStatus::Applying => {
                match recover_applying_proposal(
                    &state.paths,
                    registry_id,
                    proposal,
                    current_name.as_deref(),
                )
                .map_err(|_| "話者名登録の保存を回復できません".to_owned())?
                {
                    ApplyingProposalRecovery::Applied => {
                        recovered_directory_change = true;
                        names.insert(proposal.speaker_id.clone(), proposal.name.clone());
                        proposal.pending_bubble_dismissal_id = previous_bubble_id.clone();
                        initialize_applied_result_state(proposal, conversation_generation, now);
                    }
                    ApplyingProposalRecovery::Conflict => {
                        proposal.confirmation_bubble_id = None;
                        updated.push(proposal.clone());
                        continue;
                    }
                }
                updated.push(proposal.clone());
            }
            SpeakerNameProposalStatus::Applied => {
                let Some(operation) = proposal.operation.as_ref() else {
                    proposal.status = SpeakerNameProposalStatus::Conflict;
                    proposal.confirmation_bubble_id = None;
                    updated.push(proposal.clone());
                    continue;
                };
                if current_name.as_deref() != Some(operation.after_name.as_str()) {
                    proposal.status = SpeakerNameProposalStatus::Conflict;
                    proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Resolved;
                    proposal.confirmation_bubble_id = None;
                    updated.push(proposal.clone());
                    continue;
                }
                if proposal.can_undo_at(&now)
                    && matches!(
                        proposal.undo_card_status,
                        SpeakerNameProposalUndoCardStatus::Pending
                            | SpeakerNameProposalUndoCardStatus::Delivering
                    )
                {
                    let result_id = crate::speaker_name_proposal_presenter::result_id(proposal);
                    if proposal.confirmation_bubble_id.as_deref() != Some(result_id.as_str()) {
                        proposal.confirmation_bubble_id = Some(result_id);
                        updated.push(proposal.clone());
                    }
                } else if !proposal.can_undo_at(&now)
                    && (proposal.undo_card_status != SpeakerNameProposalUndoCardStatus::Expired
                        || proposal.pending_bubble_dismissal_id != proposal.confirmation_bubble_id)
                {
                    proposal.undo_card_status = SpeakerNameProposalUndoCardStatus::Expired;
                    proposal.pending_bubble_dismissal_id = proposal.confirmation_bubble_id.clone();
                    updated.push(proposal.clone());
                }
            }
            SpeakerNameProposalStatus::Undoing => {
                match recover_undoing_proposal(
                    &state.paths,
                    registry_id,
                    proposal,
                    current_name.as_deref(),
                )
                .map_err(|_| "話者名の取り消しを回復できません".to_owned())?
                {
                    UndoingProposalRecovery::Undone => {
                        recovered_directory_change = true;
                        if let Some(name) = proposal
                            .operation
                            .as_ref()
                            .and_then(|operation| operation.before_name.clone())
                        {
                            names.insert(proposal.speaker_id.clone(), name);
                        } else {
                            names.remove(&proposal.speaker_id);
                        }
                    }
                    UndoingProposalRecovery::Conflict => {
                        proposal.confirmation_bubble_id = None;
                    }
                }
                updated.push(proposal.clone());
            }
            _ => {}
        }
    }

    if !updated.is_empty() {
        for proposal in &mut updated {
            let old_bubble_id = expected_proposals
                .iter()
                .find(|expected| expected.id == proposal.id)
                .and_then(|expected| expected.confirmation_bubble_id.as_ref());
            if proposal.pending_bubble_dismissal_id.is_none()
                && old_bubble_id.is_some()
                && proposal.confirmation_bubble_id.as_ref() != old_bubble_id
            {
                proposal.pending_bubble_dismissal_id = old_bubble_id.cloned();
            }
        }
        let written_ids = update_speaker_name_proposals(&state.paths, |latest| {
            Ok(replace_unchanged_speaker_name_proposals(
                latest,
                &expected_proposals,
                &updated,
            ))
        })
        .map_err(|_| "話者名登録の状態を保存できません".to_owned())?;
        display.retain(|delivery| written_ids.contains(&delivery.proposal_id));
    }
    let config = state.runtime_config();
    let result_sync = reconcile_speaker_name_proposal_results(
        &state.paths,
        &config,
        &mut store.proposals,
        conversation_generation,
        now,
    )?;
    recovered_conversation_record |= result_sync.conversation_record_appended;
    display.extend(result_sync.display);
    if recovered_directory_change {
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::SpeakerDirectoryChanged)
            .await;
    }
    if recovered_directory_change || recovered_conversation_record {
        state.refresh_conversation().await;
    }

    let visible_ids = display
        .iter()
        .map(|delivery| delivery.record.id.clone())
        .chain(store.proposals.iter().filter_map(|proposal| {
            (proposal.can_undo_at(&now)
                && matches!(
                    proposal.undo_card_status,
                    SpeakerNameProposalUndoCardStatus::Pending
                        | SpeakerNameProposalUndoCardStatus::Delivering
                        | SpeakerNameProposalUndoCardStatus::Delivered
                ))
            .then(|| proposal.confirmation_bubble_id.clone())
            .flatten()
        }))
        .collect::<std::collections::BTreeSet<_>>();
    for id in speaker_name_proposal_bubbles_to_dismiss(&existing_bubble_ids, &visible_ids) {
        dismiss_speaker_name_proposal_bubble_from_root(&state.ui, &state.paths, &id).await?;
    }
    Ok(display)
}

pub(crate) async fn dismiss_speaker_name_proposal_bubble(state: &Arc<DesktopState>, id: &str) {
    let queue = state.speaker_queue.clone();
    let queued_state = state.clone();
    let bubble_id = id.to_owned();
    let _ = queue
        .enqueue(move || async move {
            dismiss_speaker_name_proposal_bubble_from_root(
                &queued_state.ui,
                &queued_state.paths,
                &bubble_id,
            )
            .await
        })
        .await;
}

async fn dismiss_speaker_name_proposal_bubble_from_root(
    ui: &crate::ui_root::UiHandle,
    paths: &coosenpai_core::config::ConfigPaths,
    id: &str,
) -> Result<(), String> {
    if root_contains_speaker_name_bubble(ui, id).await? {
        crate::bubbles::mutate_checked(ui, crate::bubbles::BubbleMutation::Dismiss(id.to_owned()))
            .await?;
        if root_contains_speaker_name_bubble(ui, id).await? {
            return Ok(());
        }
    }
    update_speaker_name_proposals(paths, |store| {
        for proposal in &mut store.proposals {
            if proposal.pending_bubble_dismissal_id.as_deref() == Some(id) {
                proposal.pending_bubble_dismissal_id = None;
            }
            let result_card_dismissed = proposal.status == SpeakerNameProposalStatus::Applied
                && matches!(
                    proposal.undo_card_status,
                    SpeakerNameProposalUndoCardStatus::Dismissed
                        | SpeakerNameProposalUndoCardStatus::Expired
                        | SpeakerNameProposalUndoCardStatus::Resolved
                );
            if proposal.confirmation_bubble_id.as_deref() == Some(id)
                && (result_card_dismissed
                    || matches!(
                        proposal.status,
                        SpeakerNameProposalStatus::Rejected
                            | SpeakerNameProposalStatus::Undone
                            | SpeakerNameProposalStatus::Conflict
                            | SpeakerNameProposalStatus::Expired
                    ))
            {
                proposal.confirmation_bubble_id = None;
            }
        }
        Ok(())
    })
    .map(|_| ())
    .map_err(|_| "話者名登録カードの解除結果を保存できません".to_owned())
}

async fn root_contains_speaker_name_bubble(
    ui: &crate::ui_root::UiHandle,
    id: &str,
) -> Result<bool, String> {
    ui.query(crate::ui_events::UiView::Bubble, |reply| {
        crate::ui_events::UiEvent::BubbleQuery(crate::ui_events::BubbleQuery::ContainsRecord {
            id: id.to_owned(),
            reply,
        })
    })
    .await
}

async fn reconcile_pending_speaker_name_bubble_dismissals(
    ui: &crate::ui_root::UiHandle,
    paths: &coosenpai_core::config::ConfigPaths,
) -> Result<(), String> {
    let store = load_speaker_name_proposals(paths)
        .map_err(|_| "話者名登録カードの解除待ち状態を読み込めません".to_owned())?;
    let pending_ids = store
        .proposals
        .iter()
        .filter_map(|proposal| proposal.pending_bubble_dismissal_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for id in pending_ids {
        dismiss_speaker_name_proposal_bubble_from_root(ui, paths, &id).await?;
    }
    Ok(())
}

async fn dismiss_conflicted_speaker_name_proposal_bubble(
    ui: &crate::ui_root::UiHandle,
    paths: &coosenpai_core::config::ConfigPaths,
    proposal_id: &str,
    version: u64,
    bubble_id: &str,
) -> Result<(), String> {
    let is_conflict_card = load_speaker_name_proposals(paths)
        .ok()
        .is_some_and(|store| {
            store.proposals.iter().any(|proposal| {
                proposal.id == proposal_id
                    && proposal.version == version
                    && proposal.status == SpeakerNameProposalStatus::Conflict
                    && proposal.confirmation_bubble_id.as_deref() == Some(bubble_id)
            })
        });
    if !is_conflict_card {
        return Ok(());
    }
    update_speaker_name_proposals(paths, |store| {
        if let Some(proposal) = store.proposals.iter_mut().find(|proposal| {
            proposal.id == proposal_id
                && proposal.version == version
                && proposal.status == SpeakerNameProposalStatus::Conflict
                && proposal.confirmation_bubble_id.as_deref() == Some(bubble_id)
        }) {
            proposal.pending_bubble_dismissal_id = Some(bubble_id.to_owned());
        }
        Ok(())
    })
    .map_err(|_| "話者名登録カードの解除待ち状態を保存できません".to_owned())?;
    dismiss_speaker_name_proposal_bubble_from_root(ui, paths, bubble_id).await
}

async fn recover_speaker_proposal_card_after_action_error(
    queue: SpeakerManagementQueue,
    ui: crate::ui_root::UiHandle,
    paths: coosenpai_core::config::ConfigPaths,
    proposal_id: String,
    version: u64,
    bubble_id: String,
) -> Result<(), String> {
    queue
        .enqueue(move || async move {
            dismiss_conflicted_speaker_name_proposal_bubble(
                &ui,
                &paths,
                &proposal_id,
                version,
                &bubble_id,
            )
            .await?;
            reconcile_pending_speaker_name_bubble_dismissals(&ui, &paths).await
        })
        .await
}

pub(crate) fn speaker_name_proposal_confirmation_bubble_ids(
    proposals: &[StoredSpeakerNameProposal],
) -> Vec<String> {
    proposals
        .iter()
        .filter_map(|proposal| proposal.confirmation_bubble_id.clone())
        .collect()
}

pub(crate) fn speaker_name_proposal_bubbles_to_dismiss(
    existing_ids: &[String],
    visible_ids: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    existing_ids
        .iter()
        .filter(|id| !visible_ids.contains(*id))
        .cloned()
        .collect()
}

fn replace_unchanged_speaker_name_proposals(
    latest: &mut coosenpai_core::speaker_name_proposals::SpeakerNameProposalStore,
    expected: &[StoredSpeakerNameProposal],
    updates: &[StoredSpeakerNameProposal],
) -> std::collections::BTreeSet<String> {
    let mut written = std::collections::BTreeSet::new();
    for update in updates {
        let Some(expected) = expected.iter().find(|proposal| proposal.id == update.id) else {
            continue;
        };
        if let Some(existing) = latest
            .proposals
            .iter_mut()
            .find(|proposal| proposal.id == update.id && *proposal == expected)
        {
            *existing = update.clone();
            written.insert(update.id.clone());
        }
    }
    written
}

async fn execute_management(
    state: Arc<DesktopState>,
    arguments: Vec<String>,
) -> Result<(), String> {
    state.cancel_audio_and_wait().await;
    let result = match spawn_speaker_helper(&state, arguments).await {
        Ok(output) if output.status.success() => {
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::SpeakerDirectoryChanged)
                .await;
            Ok(())
        }
        Ok(output) => Err(helper_failure_message(&output)),
        Err(message) => Err(message),
    };
    state.sync_audio();
    result
}

#[tauri::command]
pub async fn speaker_management(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeakerManagementPayload,
) -> TauriIpcResult<()> {
    authorize_main_or_details(&window)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let arguments = match management_arguments(payload, locale) {
        Ok(arguments) => arguments,
        Err(message) => return Ok(IpcResult::failure(message)),
    };
    let queue = state.speaker_queue.clone();
    let queued_state = state.inner().clone();
    Ok(
        match queue
            .enqueue(move || async move { execute_management(queued_state, arguments).await })
            .await
        {
            Ok(()) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
}
