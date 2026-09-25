use crate::commands::{authorize_main_or_details, IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::HelperResolverPort;
use coosenpai_core::speaker_names::{load_speaker_names, save_speaker_name, validate_speaker_name};
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
    let output = spawn_speaker_helper(
        state,
        vec!["--speaker-management".to_owned(), "list".to_owned()],
    )
    .await?;
    if !output.status.success() {
        return Err(helper_failure_message(&output));
    }
    let raw = parse_helper_directory(&output.stdout)?;
    let names = match &raw.registry_id {
        Some(registry_id) => {
            load_speaker_names(&state.paths, registry_id).map_err(|error| error.to_string())?
        }
        None => BTreeMap::new(),
    };
    Ok(build_directory(raw, names))
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

