use crate::bubbles;
use crate::command_guard::{CommandContext, CommandSource, DesktopCommand, DispatchError};
use crate::commands_config::{
    apply_config_patch, command_for_config_patch, config_failure_for_locale,
};
use crate::snapshot::AppSnapshot;
use crate::state::{ConfigCommitError, DesktopState};
use crate::ui_commands::UserCommand;
use crate::ui_events::{UiEvent, UiView};
use coosenpai_core::config::{Config, ConfigError, ConfigValidationIssue};
use coosenpai_core::locale::{localize_error_message, text, Locale, TextKey};
use coosenpai_core::memory::{DailySummary, FactCandidate, FactRecord, FactUpdate, WeeklySummary};
use coosenpai_core::ports::{SystemSettingsPane, SystemSettingsPort};
use coosenpai_core::runtime::RuntimeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use tauri::{AppHandle, Manager, State, WebviewWindow};

pub(super) const MAX_CHAT_BYTES: usize = 32 * 1024;

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
    Details,
    Avatar,
}

pub(crate) fn authorize(label: &str, required: CommandOrigin) -> Result<(), String> {
    let actual = match label {
        "main" => CommandOrigin::Main,
        "bubble" => CommandOrigin::Bubble,
        "capture-popup" => CommandOrigin::CapturePopup,
        "speech-popup" => CommandOrigin::SpeechPopup,
        "model-popup" => CommandOrigin::ModelPopup,
        "details" => CommandOrigin::Details,
        "avatar" => CommandOrigin::Avatar,
        _ => return Err(text(TextKey::CommandWindowNotAllowed, Locale::Ja).to_owned()),
    };
    if actual == required {
        Ok(())
    } else {
        Err(text(TextKey::CommandWindowNotAllowed, Locale::Ja).to_owned())
    }
}

pub(super) fn authorize_window<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
    required: CommandOrigin,
) -> Result<(), String> {
    authorize(window.label(), required)
}

/// main と details の両方が発行できる command の入力源を window label から決める。
pub(super) fn main_or_details_source(window: &WebviewWindow) -> Result<CommandSource, String> {
    match window.label() {
        "main" => {
            authorize_window(window, CommandOrigin::Main)?;
            Ok(CommandSource::IpcMain)
        }
        "details" => {
            authorize_window(window, CommandOrigin::Details)?;
            Ok(CommandSource::IpcDetails)
        }
        _ => Err(text(TextKey::CommandWindowNotAllowed, Locale::Ja).to_owned()),
    }
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
    let locale = Locale::from_config(&state.runtime_config().ui.language);
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
        Err(error) => IpcResult::failure(error.format_for_locale(locale)),
    }
}

/// ポップアップ送信の逐次: dispatch が受理・失敗・拒否のどれで終わっても、
/// 結果を呼び出し元へ返す前にメイン画面を前面に出す。
#[cfg(test)]
pub(super) async fn dispatch_send_and_present<F, Fut>(
    logger: &dyn RuntimeLogger,
    locale: Locale,
    dispatch: F,
    present_main: impl FnOnce(),
) -> IpcResult<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<IpcResult<String>, DispatchError>> + Send + 'static,
{
    let result = run_detached(dispatch()).await;
    let outcome = match &result {
        Ok(IpcResult::Success { .. }) => crate::windows::SendOutcome::Accepted,
        Ok(IpcResult::Failure { .. }) => crate::windows::SendOutcome::Failed,
        Err(DispatchError::Rejected(_)) => crate::windows::SendOutcome::Rejected,
        Err(DispatchError::Failed(_) | DispatchError::Indeterminate(_)) => {
            crate::windows::SendOutcome::Failed
        }
    };
    crate::windows::present_main_after_send(logger, outcome, present_main);
    match result {
        Ok(result) => result,
        Err(error) => IpcResult::failure(error.format_for_locale(locale)),
    }
}

fn run_detached<T, Fut>(future: Fut) -> impl std::future::Future<Output = Result<T, DispatchError>>
where
    T: Send + 'static,
    Fut: std::future::Future<Output = Result<T, DispatchError>> + Send + 'static,
{
    // 設定操作の大きな Future を spawn の各層へ値渡しせず、この境界で保持する。
    let future = Box::pin(future);
    async move {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let result = future.await;
            let _ = result_tx.send(result);
        });
        match result_rx.await {
            Ok(result) => result,
            Err(_) => Err(DispatchError::indeterminate(text(
                TextKey::CommandResultUnavailable,
                Locale::Ja,
            ))),
        }
    }
}

pub(super) fn validate_id_for_locale(id: &str, locale: Locale) -> Result<(), String> {
    if id.trim().is_empty() {
        Err(text(TextKey::IdentifierEmpty, locale).to_owned())
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
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::WatchStart {
                source: CommandSource::IpcMain,
                reply,
            })
        })
        .await
}

#[tauri::command]
pub async fn companion_emotions_reset(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    let source = main_or_details_source(&window)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::EmotionsReset { source, reply })
        })
        .await
}

#[tauri::command]
pub async fn watch_stop(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::WatchStop {
                source: CommandSource::IpcMain,
                reply,
            })
        })
        .await
}

#[tauri::command]
pub async fn chat_send(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    payload: ChatPayload,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Chat,
                crate::ui_events::UiEvent::SubmitChat(payload.message),
            )
            .await
        {
            Ok(Some(id)) => IpcResult::success(id),
            Ok(None) => IpcResult::failure("チャットの受付結果がありません"),
            Err(error) => IpcResult::failure(error),
        },
    )
}

#[tauri::command]
pub async fn chat_cancel(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::ChatCancel(reply))
        })
        .await
}
#[tauri::command]
pub async fn chat_retry(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::ChatRetry(reply))
        })
        .await
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
pub async fn model_popup_open(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Chat,
                crate::ui_events::UiEvent::OpenModelPicker,
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[tauri::command]
pub async fn model_popup_close(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::ModelPopup)?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::ModelPicker,
                crate::ui_events::UiEvent::Close,
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
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
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    if let Err(error) = validate_model_popup_patch(&patch, locale) {
        return Ok(IpcResult::failure(error));
    }
    state
        .ui
        .query(UiView::ModelPicker, |reply| {
            UiEvent::UserCommand(UserCommand::ModelSave { patch, reply })
        })
        .await
}

fn validate_model_popup_patch(patch: &Value, locale: Locale) -> Result<(), String> {
    let Some(root) = patch.as_object() else {
        return Err(text(TextKey::ModelConfigObject, locale).to_owned());
    };
    if root.len() != 1 || !root.contains_key("companion") {
        return Err(text(TextKey::ModelConfigCompanionOnly, locale).to_owned());
    }
    let Some(companion) = root.get("companion").and_then(Value::as_object) else {
        return Err(text(TextKey::ModelConfigCompanionObject, locale).to_owned());
    };
    if companion.is_empty() {
        return Err(text(TextKey::ModelConfigRequired, locale).to_owned());
    }
    if companion
        .keys()
        .any(|key| !matches!(key.as_str(), "provider" | "model" | "effort"))
    {
        return Err(text(TextKey::ModelConfigKeys, locale).to_owned());
    }
    Ok(())
}

#[tauri::command]
pub async fn config_update(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    patch: Value,
    avatar_image: Option<Vec<u8>>,
    base_config_revision: u64,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let normalized_avatar = match avatar_image {
        None => None,
        Some(bytes) => {
            let normalized =
                tokio::task::spawn_blocking(move || crate::avatar::normalize_image(&bytes)).await;
            match normalized {
                Ok(Ok(bytes)) => Some(bytes),
                Ok(Err(error)) => {
                    return Ok(IpcResult::failure(
                        text(TextKey::AvatarImageProcessFailed, locale)
                            .replace("{error}", &error.to_string()),
                    ))
                }
                Err(error) => {
                    return Ok(IpcResult::failure(
                        text(TextKey::AvatarImageProcessingIncomplete, locale)
                            .replace("{error}", &error.to_string()),
                    ))
                }
            }
        }
    };
    let mut patch = patch;
    if normalized_avatar.is_some() {
        if let Err(error) = force_avatar_path(&mut patch) {
            return Ok(config_failure_for_locale(
                error,
                Locale::from_config(&state.runtime_config().ui.language),
            ));
        }
    }
    Ok(update_config_for_source(
        state.inner().clone(),
        patch,
        normalized_avatar,
        Some(base_config_revision),
        CommandSource::IpcMain,
    )
    .await)
}

pub(crate) async fn update_config_for_source(
    state: Arc<DesktopState>,
    patch: Value,
    normalized_avatar: Option<Vec<u8>>,
    base_config_revision: Option<u64>,
    source: CommandSource,
) -> IpcResult<Config> {
    let signed = crate::state::signed_build();
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let persisted = match coosenpai_core::config::load_config(&state.paths) {
        Ok(config) => config,
        Err(ConfigError::Json(_)) => state.runtime_config(),
        Err(error) => {
            return config_failure_for_locale(
                error,
                Locale::from_config(&state.runtime_config().ui.language),
            )
        }
    };
    let command = match command_for_config_patch(&persisted, &patch, signed) {
        Ok(command) => command,
        Err(error) => {
            return config_failure_for_locale(
                error,
                Locale::from_config(&state.runtime_config().ui.language),
            )
        }
    };
    let command = if normalized_avatar.is_some() && command == DesktopCommand::ConfigAudioUpdate {
        DesktopCommand::ConfigDisplayUpdate
    } else {
        command
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
                    return IpcResult::failure(
                        text(TextKey::AvatarImageStageFailed, locale)
                            .replace("{error}", &error.to_string()),
                    )
                }
                Err(error) => {
                    return IpcResult::failure(
                        text(TextKey::AvatarImageStageIncomplete, locale)
                            .replace("{error}", &error.to_string()),
                    )
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
            Err(error) => config_commit_failure_for_locale(
                error,
                Locale::from_config(&handler_state.runtime_config().ui.language),
            ),
        }
    })
    .await
}

fn force_avatar_path(patch: &mut Value) -> Result<(), ConfigError> {
    let Some(object) = patch.as_object_mut() else {
        return Err(ConfigError::Validation(vec![ConfigValidationIssue {
            path: "config".to_owned(),
            message: text(TextKey::ConfigInvalidObject, Locale::Ja).to_owned(),
        }]));
    };
    let ui = object
        .entry("ui".to_owned())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(ui) = ui.as_object_mut() else {
        return Err(ConfigError::Validation(vec![ConfigValidationIssue {
            path: "ui".to_owned(),
            message: text(TextKey::ConfigInvalidObject, Locale::Ja).to_owned(),
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
        return Ok(IpcResult::failure(text(
            TextKey::AssertivenessInvalid,
            Locale::from_config(&state.runtime_config().ui.language),
        )));
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
        Err(error) => IpcResult::failure(localize_error_message(
            &error,
            TextKey::PersonaOperationFailed,
            Locale::from_config(&state.runtime_config().ui.language),
        )),
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
    validate_id_for_locale(
        &payload.persona,
        Locale::from_config(&state.runtime_config().ui.language),
    )?;
    Ok(select_persona_for_view(state.inner().clone(), payload.persona, false).await)
}
#[tauri::command]
pub async fn persona_select_setup(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaSelectPayload,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id_for_locale(
        &payload.persona,
        Locale::from_config(&state.runtime_config().ui.language),
    )?;
    Ok(select_persona_for_view(state.inner().clone(), payload.persona, true).await)
}
pub(crate) async fn select_persona_for_view(
    state: Arc<DesktopState>,
    persona: String,
    setup: bool,
) -> IpcResult<Config> {
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    if !crate::factory::persona_names(&state.paths).contains(&persona) {
        return IpcResult::failure(text(TextKey::PersonaNotFound, locale));
    }
    let handler = state.clone();
    dispatch_result(
        state,
        CommandSource::IpcMain,
        if setup {
            DesktopCommand::SetupPersonaSelect
        } else {
            DesktopCommand::PersonaSelect
        },
        move |context| async move {
            let result = if setup {
                handler
                    .command_switch_persona_during_setup(&context, persona)
                    .await
            } else {
                handler.command_switch_persona(&context, persona).await
            };
            match result {
                Ok(config) => IpcResult::success(config),
                Err(error) => config_commit_failure_for_locale(
                    error,
                    Locale::from_config(&handler.runtime_config().ui.language),
                ),
            }
        },
    )
    .await
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
        _ => IpcResult::failure(text(
            TextKey::SystemSettingsOpenFailed,
            Locale::from_config(&state.runtime_config().ui.language),
        )),
    })
}

#[tauri::command]
pub async fn app_relaunch(window: WebviewWindow, app: AppHandle) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    app.request_restart();
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn app_exit(window: WebviewWindow, app: AppHandle) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or("UIの状態がありません")?;
    state.ui.input(
        crate::ui_events::UiView::Chat,
        crate::ui_events::UiEvent::Shutdown,
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn advice_selected(
    window: WebviewWindow,
    app: AppHandle,
    payload: IdPayload,
) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| Locale::from_config(&state.runtime_config().ui.language))
        .unwrap_or(Locale::Ja);
    validate_id_for_locale(&payload.id, locale)?;
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or("UIの状態がありません")?;
    state
        .ui
        .request(
            crate::ui_events::UiView::Chat,
            crate::ui_events::UiEvent::OpenMain,
        )
        .await?;
    state
        .ui
        .request(
            crate::ui_events::UiView::Chat,
            crate::ui_events::UiEvent::SelectConversation(payload.id),
        )
        .await?;
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn settings_requested(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<IpcResult<()>, String> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or("UIの状態がありません")?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Chat,
                crate::ui_events::UiEvent::OpenSettings,
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[tauri::command]
pub async fn chat_input_state(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    payload: InputStatePayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .request(UiView::Chat, UiEvent::ChatInputActive(payload.active))
        .await?;
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn unread_read(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    state.ui.request(UiView::Chat, UiEvent::UnreadRead).await?;
    Ok(IpcResult::success(()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubbleClickPayload {
    id: String,
    #[serde(default)]
    body: bool,
}

#[tauri::command]
pub async fn bubble_click<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, crate::bubble_click::BubbleClickState>,
    payload: BubbleClickPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    let host = &state.inner().0;
    validate_id_for_locale(&payload.id, host.locale())?;
    host.input_click(payload.id, payload.body).await?;
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn bubble_hover(
    state: State<'_, Arc<DesktopState>>,
    window: WebviewWindow,
    payload: BubbleHoverPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    validate_id_for_locale(
        &payload.id,
        Locale::from_config(&state.runtime_config().ui.language),
    )?;
    bubbles::set_hover(state.inner().clone(), &payload.id, payload.hovering).await;
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn bubble_focus(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Bubble,
                crate::ui_events::UiEvent::RequestFocus,
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(error) => IpcResult::failure(error),
        },
    )
}

#[tauri::command]
pub async fn bubble_passthrough(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    state.ui.input(
        crate::ui_events::UiView::Bubble,
        crate::ui_events::UiEvent::PointerPassthrough,
    );
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
        return Ok(IpcResult::failure(text(
            TextKey::BubbleHeightOutOfRange,
            Locale::from_config(&state.runtime_config().ui.language),
        )));
    }
    state.ui.input(
        crate::ui_events::UiView::Bubble,
        crate::ui_events::UiEvent::BubbleResize(payload.height),
    );
    Ok(IpcResult::success(()))
}

fn valid_bubble_height(height: u32) -> bool {
    (80..=680).contains(&height)
}

pub(super) fn runtime_failure_for_locale<T: Serialize>(
    error: RuntimeError,
    locale: Locale,
) -> IpcResult<T> {
    IpcResult::failure(error.format_for_locale(locale))
}

pub(super) fn config_commit_failure_for_locale<T: Serialize>(
    error: ConfigCommitError,
    locale: Locale,
) -> IpcResult<T> {
    IpcResult::Failure {
        ok: false,
        error: IpcError {
            message: error.format_for_locale(locale),
            issues: error.issues_for_locale(locale),
        },
    }
}

#[tauri::command]
pub async fn model_picker_input(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: crate::model_picker_presenter::ModelPickerInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::ModelPopup)?;
    if matches!(&payload, crate::model_picker_presenter::ModelPickerInput::Model { value } | crate::model_picker_presenter::ModelPickerInput::Effort { value } | crate::model_picker_presenter::ModelPickerInput::Provider { value } if value.len() > MAX_CHAT_BYTES)
    {
        return Err("入力文が長すぎます".into());
    }
    state.ui.input(
        UiView::ModelPicker,
        UiEvent::ModelPicker(crate::model_picker_presenter::ModelPickerEvent::Input(
            payload,
        )),
    );
    Ok(IpcResult::success(()))
}
