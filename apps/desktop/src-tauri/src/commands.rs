use crate::bubbles;
use crate::command_guard::{CommandContext, CommandSource, DesktopCommand, DispatchError};
use crate::commands_config::{apply_config_patch, command_for_config_patch, config_failure};
use crate::snapshot::AppSnapshot;
use crate::state::{ConfigCommitError, DesktopState};
use coosenpai_core::config::{Config, ConfigError, ConfigValidationIssue};
use coosenpai_core::memory::{DailySummary, FactCandidate, FactRecord, FactUpdate, WeeklySummary};
use coosenpai_core::ports::{RuntimeLogger, SystemSettingsPane, SystemSettingsPort};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

pub(super) const MAX_CHAT_BYTES: usize = 32 * 1024;
static BUBBLE_PASSTHROUGH_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum IpcResult<T: Serialize> {
    Success {
        ok: bool,
        value: T,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        issues: Vec<ConfigValidationIssue>,
    },
    Failure {
        ok: bool,
        error: IpcError,
    },
}

pub(super) type TauriIpcResult<T> = Result<IpcResult<T>, String>;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IpcError {
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<ConfigValidationIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatPayload {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertivenessPayload {
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubbleHoverPayload {
    id: String,
    hovering: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdPayload {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaSelectPayload {
    persona: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputStatePayload {
    active: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubbleResizePayload {
    height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfirmPayload {
    pub(super) candidate_id: String,
    pub(super) confirmation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRejectPayload {
    pub(super) candidate_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfirmUpdatePayload {
    pub(super) update_id: String,
    pub(super) confirmation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRejectUpdatePayload {
    pub(super) update_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryDeletePayload {
    pub(super) fact_id: String,
    pub(super) confirmation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConsolidatePayload {
    pub(super) period: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCatalog {
    pub(super) facts: Vec<FactRecord>,
    pub(super) candidates: Vec<FactCandidate>,
    pub(super) updates: Vec<FactUpdate>,
    pub(super) daily: Vec<DailySummary>,
    pub(super) weekly: Vec<WeeklySummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommandOrigin {
    Main,
    Bubble,
    CapturePopup,
    SpeechPopup,
    ModelPopup,
}

pub(crate) fn authorize(label: &str, required: CommandOrigin) -> Result<(), String> {
    let actual = match label {
        "main" => CommandOrigin::Main,
        "bubble" => CommandOrigin::Bubble,
        "capture-popup" => CommandOrigin::CapturePopup,
        "speech-popup" => CommandOrigin::SpeechPopup,
        "model-popup" => CommandOrigin::ModelPopup,
        _ => return Err("このウィンドウからは操作できません".to_owned()),
    };
    if actual == required {
        Ok(())
    } else {
        Err("このウィンドウからは操作できません".to_owned())
    }
}

pub(super) fn authorize_window(
    window: &WebviewWindow,
    required: CommandOrigin,
) -> Result<(), String> {
    authorize(window.label(), required)
}

pub(super) async fn dispatch_result<T, F, Fut>(
    state: Arc<DesktopState>,
    source: CommandSource,
    command: DesktopCommand,
    handler: F,
) -> IpcResult<T>
where
    T: Serialize + Send + 'static,
    F: FnOnce(CommandContext) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = IpcResult<T>> + Send + 'static,
{
    let result = run_detached(async move {
        state
            .dispatch(source, command, move |context| async move {
                Ok(handler(context).await)
            })
            .await
    })
    .await;
    match result {
        Ok(result) => result,
        Err(error) => IpcResult::failure(error.format_for_user()),
    }
}

async fn run_detached<T, Fut>(future: Fut) -> Result<T, DispatchError>
where
    T: Send + 'static,
    Fut: std::future::Future<Output = Result<T, DispatchError>> + Send + 'static,
{
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tauri::async_runtime::spawn(async move {
        let result = future.await;
        let _ = result_tx.send(result);
    });
    match result_rx.await {
        Ok(result) => result,
        Err(_) => Err(DispatchError::indeterminate(
            "処理結果を確認できませんでした",
        )),
    }
}

pub(super) fn validate_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        Err("id は空にできません".to_owned())
    } else {
        Ok(())
    }
}

impl<T: Serialize> IpcResult<T> {
    pub(super) fn success(value: T) -> Self {
        Self::Success {
            ok: true,
            value,
            issues: Vec::new(),
        }
    }

    pub(super) fn success_with_issues(value: T, issues: Vec<ConfigValidationIssue>) -> Self {
        Self::Success {
            ok: true,
            value,
            issues,
        }
    }

    pub(super) fn failure(message: impl Into<String>) -> Self {
        Self::Failure {
            ok: false,
            error: IpcError {
                message: message.into(),
                issues: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
#[async_trait]
trait CommandRuntime: Send + Sync {
    async fn snapshot(&self) -> AppSnapshot;
    async fn chat(&self, message: String) -> Result<String, String>;
}

#[tauri::command]
pub async fn snapshot_get(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(IpcResult::success(state.snapshot().await))
}

#[tauri::command]
pub async fn watch_start(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let request = async move { state.dispatch_watch_start(CommandSource::IpcMain).await };
    Ok(match run_detached(request).await {
        Ok(snapshot) => IpcResult::success(snapshot),
        Err(error) => IpcResult::failure(error.format_for_user()),
    })
}

#[tauri::command]
pub async fn watch_stop(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::WatchStop,
        move |context| async move {
            match handler_state.command_stop_watch(&context).await {
                Ok(snapshot) => IpcResult::success(snapshot),
                Err(error) => config_commit_failure(error),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn chat_send(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    payload: ChatPayload,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::ChatSend,
        move |context| async move {
            if payload.message.trim().is_empty() {
                return IpcResult::failure("message は空にできません");
            }
            if payload.message.len() > MAX_CHAT_BYTES {
                return IpcResult::failure("message が長すぎます");
            }
            match handler_state
                .command_enqueue_user_message(
                    &context,
                    payload.message,
                    Vec::new(),
                    crate::state::user_input::UserMessageAttachment::None,
                )
                .await
            {
                Ok(id) => IpcResult::success(id),
                Err(error) => IpcResult::failure(error),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn chat_cancel(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::ChatCancel,
        move |_context| async move {
            match handler_state.core_runtime().cancel_user_message().await {
                Ok(id) => IpcResult::success(id),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await)
}
#[tauri::command]
pub async fn chat_retry(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::ChatRetry,
        move |_context| async move {
            match handler_state.core_runtime().retry_user_message().await {
                Ok(id) => IpcResult::success(id),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await)
}
#[tauri::command]
pub async fn config_get(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(IpcResult::success(state.runtime_config()))
}

#[tauri::command]
pub async fn model_popup_open(window: WebviewWindow, app: AppHandle) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(match crate::windows::show_model_popup(&app) {
        Ok(()) => IpcResult::success(()),
        Err(error) => IpcResult::failure(format!("モデル変更を開けません: {error}")),
    })
}

#[tauri::command]
pub async fn model_popup_close(window: WebviewWindow) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::ModelPopup)?;
    Ok(match window.hide() {
        Ok(()) => IpcResult::success(()),
        Err(error) => IpcResult::failure(format!("モデル変更を閉じられません: {error}")),
    })
}

#[tauri::command]
pub async fn model_popup_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::ModelPopup)?;
    Ok(IpcResult::success(state.snapshot().await))
}

#[tauri::command]
pub async fn model_popup_config_update(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    patch: Value,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::ModelPopup)?;
    if let Err(error) = validate_model_popup_patch(&patch) {
        return Ok(IpcResult::failure(error));
    }
    Ok(update_config_for_source(
        state.inner().clone(),
        patch,
        None,
        None,
        CommandSource::IpcModelPopup,
    )
    .await)
}

fn validate_model_popup_patch(patch: &Value) -> Result<(), String> {
    let Some(root) = patch.as_object() else {
        return Err("モデル設定はオブジェクトで指定してください".to_owned());
    };
    if root.len() != 1 || !root.contains_key("companion") {
        return Err("モデル設定では companion だけを変更できます".to_owned());
    }
    let Some(companion) = root.get("companion").and_then(Value::as_object) else {
        return Err("モデル設定の companion はオブジェクトで指定してください".to_owned());
    };
    if companion.is_empty() {
        return Err("モデル設定を一つ以上指定してください".to_owned());
    }
    if companion
        .keys()
        .any(|key| !matches!(key.as_str(), "provider" | "model" | "effort"))
    {
        return Err("モデル設定では provider、model、effort だけを変更できます".to_owned());
    }
    Ok(())
}

#[tauri::command]
pub async fn config_update(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    patch: Value,
    avatar_image: Option<Vec<u8>>,
    base_config_revision: Option<u64>,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    let normalized_avatar = match avatar_image {
        None => None,
        Some(bytes) => {
            let normalized =
                tokio::task::spawn_blocking(move || crate::avatar::normalize_image(&bytes)).await;
            match normalized {
                Ok(Ok(bytes)) => Some(bytes),
                Ok(Err(error)) => {
                    return Ok(IpcResult::failure(format!(
                        "アバター画像を処理できません: {error}"
                    )))
                }
                Err(error) => {
                    return Ok(IpcResult::failure(format!(
                        "アバター画像の処理を完了できません: {error}"
                    )))
                }
            }
        }
    };
    let mut patch = patch;
    if normalized_avatar.is_some() {
        if let Err(error) = force_avatar_path(&mut patch) {
            return Ok(config_failure(error));
        }
    }
    Ok(update_config_for_source(
        state.inner().clone(),
        patch,
        normalized_avatar,
        base_config_revision,
        CommandSource::IpcMain,
    )
    .await)
}

async fn update_config_for_source(
    state: Arc<DesktopState>,
    patch: Value,
    normalized_avatar: Option<Vec<u8>>,
    base_config_revision: Option<u64>,
    source: CommandSource,
) -> IpcResult<Config> {
    let signed = crate::state::signed_build();
    let persisted = match coosenpai_core::config::load_config(&state.paths) {
        Ok(config) => config,
        Err(ConfigError::Json(_)) => state.runtime_config(),
        Err(error) => return config_failure(error),
    };
    let command = match command_for_config_patch(&persisted, &patch, signed) {
        Ok(command) => command,
        Err(error) => return config_failure(error),
    };
    let staged_avatar = match normalized_avatar {
        None => None,
        Some(bytes) => {
            let paths = state.paths.clone();
            let staged = tokio::task::spawn_blocking(move || {
                crate::avatar::stage_normalized(&paths, &bytes)
            })
            .await;
            match staged {
                Ok(Ok(staged)) => Some(staged),
                Ok(Err(error)) => {
                    return IpcResult::failure(format!("アバター画像を一時保存できません: {error}"))
                }
                Err(error) => {
                    return IpcResult::failure(format!(
                        "アバター画像の一時保存を完了できません: {error}"
                    ))
                }
            }
        }
    };
    let handler_state = state.clone();
    dispatch_result(state, source, command, move |context| async move {
        let update = move |current| apply_config_patch(current, patch, signed);
        let result = match staged_avatar {
            Some(staged_avatar) => match base_config_revision {
                Some(expected_revision) => {
                    handler_state
                        .command_update_config_with_staged_avatar_expected_revision(
                            &context,
                            staged_avatar,
                            expected_revision,
                            update,
                        )
                        .await
                }
                None => {
                    handler_state
                        .command_update_config_with_staged_avatar(&context, staged_avatar, update)
                        .await
                }
            },
            None => match base_config_revision {
                Some(expected_revision) => {
                    handler_state
                        .command_update_config_with_expected_revision(
                            &context,
                            expected_revision,
                            update,
                        )
                        .await
                }
                None => {
                    handler_state
                        .command_update_config_with(&context, update)
                        .await
                }
            },
        };
        match result {
            Ok(outcome) => IpcResult::success_with_issues(outcome.config, outcome.issues),
            Err(error) => config_commit_failure(error),
        }
    })
    .await
}

fn force_avatar_path(patch: &mut Value) -> Result<(), ConfigError> {
    let Some(object) = patch.as_object_mut() else {
        return Err(ConfigError::Validation(vec![ConfigValidationIssue {
            path: "config".to_owned(),
            message: "設定はオブジェクトで指定してください。".to_owned(),
        }]));
    };
    let ui = object
        .entry("ui".to_owned())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(ui) = ui.as_object_mut() else {
        return Err(ConfigError::Validation(vec![ConfigValidationIssue {
            path: "ui".to_owned(),
            message: "設定はオブジェクトで指定してください。".to_owned(),
        }]));
    };
    ui.insert(
        "avatarPath".to_owned(),
        Value::String(crate::avatar::CONFIG_PATH.to_owned()),
    );
    Ok(())
}

#[tauri::command]
pub async fn companion_assertiveness_set(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    payload: AssertivenessPayload,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    if !matches!(payload.value.as_str(), "low" | "normal" | "high") {
        return Ok(IpcResult::failure("積極性の値が不正です"));
    }
    let value = payload.value;
    let state = state.inner().clone();
    let handler_state = state.clone();
    let result = dispatch_result(
        state.clone(),
        CommandSource::IpcMain,
        DesktopCommand::ConfigDisplayUpdate,
        move |_context| {
            let value = value.clone();
            async move {
                IpcResult::success(handler_state.set_temporary_assertiveness(value).await)
            }
        },
    )
    .await;
    Ok(result)
}

#[tauri::command]
pub async fn persona_list(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<Vec<crate::factory::PersonaOption>> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(match crate::factory::persona_options(&state.paths) {
        Ok(values) => IpcResult::success(values),
        Err(error) => IpcResult::failure(error),
    })
}

#[tauri::command]
pub async fn provider_models(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<Vec<crate::factory::ProviderModelOptions>> {
    crate::commands_provider_models::provider_models_for_state(window.label(), state.inner()).await
}

#[tauri::command]
pub async fn model_popup_companion_model_catalog(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<crate::model_catalog::ModelCatalogView> {
    authorize_window(&window, CommandOrigin::ModelPopup)?;
    Ok(IpcResult::success(
        crate::model_catalog::catalog_for_state(state.inner()).await,
    ))
}

#[tauri::command]
pub async fn model_popup_opencode_models_reload(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<crate::model_catalog::ModelCatalogView> {
    authorize_window(&window, CommandOrigin::ModelPopup)?;
    Ok(IpcResult::success(
        crate::model_catalog::reload_opencode_models(state.inner()).await,
    ))
}

#[tauri::command]
pub async fn persona_select(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaSelectPayload,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id(&payload.persona)?;
    if !crate::factory::persona_names(&state.paths).contains(&payload.persona) {
        return Ok(IpcResult::failure("選択した性格が見つかりません"));
    }
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaSelect,
        move |context| async move {
            match handler_state
                .command_switch_persona(&context, payload.persona)
                .await
            {
                Ok(config) => IpcResult::success(config),
                Err(error) => config_commit_failure(error),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn panel_open_system_settings(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let result = crate::platform::MacSystemSettings
        .open(
            SystemSettingsPane::ScreenCapture,
            state.cancellation.clone(),
        )
        .await;
    Ok(match result {
        Ok(()) => IpcResult::success(()),
        _ => IpcResult::failure("システム設定を開けませんでした"),
    })
}

#[tauri::command]
pub async fn app_relaunch(window: WebviewWindow, app: AppHandle) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    app.restart();
}

#[tauri::command]
pub async fn app_exit(window: WebviewWindow, app: AppHandle) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    app.exit(0);
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn advice_selected(
    window: WebviewWindow,
    app: AppHandle,
    payload: IdPayload,
) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id(&payload.id)?;
    crate::windows::show_main(&app);
    let _ = app.emit("coosenpai:conversation:selected", payload.id);
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn settings_requested(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
        let state = state.inner().clone();
        let handler_state = state.clone();
        let result = dispatch_result(
            state,
            CommandSource::IpcMain,
            DesktopCommand::SettingsOpen,
            move |context| async move {
                match handler_state
                    .command_tutorial_settings_opened(&context)
                    .await
                {
                    Ok(true) => {
                        handler_state.refresh_speech_input_devices().await;
                        crate::windows::show_main(&handler_state.app);
                        let _ = handler_state.app.emit("coosenpai:settings:requested", ());
                        IpcResult::success(())
                    }
                    Ok(false) => IpcResult::failure("性格の案内までお待ちください"),
                    Err(error) => IpcResult::failure(error.to_string()),
                }
            },
        )
        .await;
        if matches!(&result, IpcResult::Failure { .. }) {
            return Ok(result);
        }
        return Ok(result);
    }
    crate::windows::show_main(&app);
    let _ = app.emit("coosenpai:settings:requested", ());
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn chat_input_state(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    payload: InputStatePayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .input_active
        .store(payload.active, std::sync::atomic::Ordering::Release);
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn unread_read(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    state.publish(|snapshot| snapshot.unread_count = 0).await;
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn bubble_click(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: IdPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    validate_id(&payload.id)?;
    let _ = state.logger.write(
        "INFO",
        &format!("吹き出しのクリックを受け付けました: id={}", payload.id),
    );
    let open_url = state.bubbles.lock().await.open_url_for(&payload.id);
    if let Some(url) = open_url {
        if url != crate::update_check::RELEASES_URL {
            return Err("吹き出しの外部リンクが不正です".to_owned());
        }
        crate::platform::open_external_url(&url)
            .await
            .map_err(|error| error.to_string())?;
        bubbles::complete_action(state.inner().as_ref(), &payload.id).await;
        let _ = state.logger.write(
            "INFO",
            &format!("吹き出しから外部リンクを開きました: id={}", payload.id),
        );
        return Ok(IpcResult::success(()));
    }
    crate::windows::show_main_now(&app).await?;
    bubbles::complete_action(state.inner().as_ref(), &payload.id).await;
    let _ = state.logger.write(
        "INFO",
        &format!(
            "吹き出しからメインウィンドウを表示しました: id={}",
            payload.id
        ),
    );
    let _ = app.emit("coosenpai:conversation:selected", payload.id);
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn bubble_hover(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    payload: BubbleHoverPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    validate_id(&payload.id)?;
    bubbles::set_hover(state.inner().clone(), &payload.id, payload.hovering).await;
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn bubble_focus(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    let main = state
        .app
        .get_webview_window("main")
        .ok_or_else(|| "吹き出しをキーボード操作できる状態にできません".to_owned())?;
    let focus_events = state.bubble_focus_events();
    window
        .set_focusable(true)
        .map_err(|_| "吹き出しをキーボード操作できる状態にできません".to_owned())?;
    let focused = crate::windows::activate_and_focus_window(&main, &window, focus_events)
        .await
        .map_err(|_| "吹き出しをキーボード操作できる状態にできません".to_owned())?;
    if focused {
        let _ = state
            .logger
            .write("INFO", "吹き出しのキーボードフォーカスを確認しました");
        return Ok(IpcResult::success(()));
    }
    let details = crate::windows::focus_failure_details(&window);
    crate::windows::log_focus_failure(state.logger.as_ref(), "吹き出し", &details);
    Err("吹き出しをキーボード操作できる状態にできません".to_owned())
}

#[tauri::command]
pub async fn bubble_passthrough(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    let generation = BUBBLE_PASSTHROUGH_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    window
        .set_ignore_cursor_events(true)
        .map_err(|_| "吹き出しのクリック透過を有効にできません".to_owned())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let record_count = state.bubbles.lock().await.snapshot().records.len();
        if BUBBLE_PASSTHROUGH_GENERATION.load(Ordering::Relaxed) == generation
            && crate::bubbles::accepts_pointer(record_count)
        {
            let _ = window.set_ignore_cursor_events(false);
        }
    });
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn bubble_resize(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleResizePayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    if !valid_bubble_height(payload.height) {
        return Ok(IpcResult::failure("吹き出しの高さが範囲外です"));
    }
    let _window_sync = state.bubble_window_sync.lock().await;
    let config = state.runtime_config();
    let preview = state.bubbles.lock().await.appearance_preview();
    let position = preview
        .as_ref()
        .map_or(config.bubble.position.as_str(), |value| {
            value.position.as_str()
        });
    let display = preview
        .as_ref()
        .map_or(config.bubble.display.as_str(), |value| {
            value.display.as_str()
        });
    Ok(
        match crate::window_bubble::resize(&window, payload.height, position, display) {
            Ok(()) => IpcResult::success(()),
            Err(_) => IpcResult::failure("吹き出しの大きさを変更できません"),
        },
    )
}

fn valid_bubble_height(height: u32) -> bool {
    (80..=680).contains(&height)
}

pub(super) fn config_commit_failure<T: Serialize>(error: ConfigCommitError) -> IpcResult<T> {
    IpcResult::Failure {
        ok: false,
        error: IpcError {
            message: error.format_for_user(),
            issues: error.issues(),
        },
    }
}

