use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::provider::ProviderName;
use coosenpai_core::provider_api_keys::ProviderApiKeyStatus;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderApiKeySetPayload {
    provider: String,
    api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderApiKeyDeletePayload {
    provider: String,
}

#[tauri::command]
pub async fn provider_api_keys_get(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<ProviderApiKeyStatus> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    Ok(match state.factory.provider_api_key_status() {
        Ok(status) => IpcResult::success(status),
        Err(error) => IpcResult::failure(error.format_for_locale(locale)),
    })
}

#[tauri::command]
pub async fn provider_api_key_set(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: ProviderApiKeySetPayload,
) -> TauriIpcResult<ProviderApiKeyStatus> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let provider = match parse_provider_for_locale(&payload.provider, locale) {
        Ok(provider) => provider,
        Err(error) => return Ok(IpcResult::failure(error)),
    };
    if let Err(error) = validate_api_key_for_locale(&payload.api_key, locale) {
        return Ok(IpcResult::failure(error));
    }
    let state = state.inner().clone();
    let handler_state = state.clone();
    let api_key = payload.api_key;
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::ProviderApiKeyUpdate,
        move |_context| async move {
            match handler_state
                .factory
                .update_provider_api_key(provider, Some(&api_key))
                .await
            {
                Ok(status) => IpcResult::success(status),
                Err(error) => IpcResult::failure(error.format_for_locale(Locale::from_config(
                    &handler_state.runtime_config().ui.language,
                ))),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn provider_api_key_delete(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: ProviderApiKeyDeletePayload,
) -> TauriIpcResult<ProviderApiKeyStatus> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let provider = match parse_provider_for_locale(&payload.provider, locale) {
        Ok(provider) => provider,
        Err(error) => return Ok(IpcResult::failure(error)),
    };
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::ProviderApiKeyUpdate,
        move |_context| async move {
            match handler_state
                .factory
                .update_provider_api_key(provider, None)
                .await
            {
                Ok(status) => IpcResult::success(status),
                Err(error) => IpcResult::failure(error.format_for_locale(Locale::from_config(
                    &handler_state.runtime_config().ui.language,
                ))),
            }
        },
    )
    .await)
}

fn parse_provider_for_locale(value: &str, locale: Locale) -> Result<ProviderName, String> {
    match value {
        "codex" => Ok(ProviderName::Codex),
        "claude" => Ok(ProviderName::Claude),
        "opencode" => Ok(ProviderName::Opencode),
        _ => Err(text(TextKey::SetupProviderInvalid, locale).to_owned()),
    }
}

fn validate_api_key_for_locale(value: &str, locale: Locale) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(text(TextKey::SetupApiKeyEmpty, locale).to_owned());
    }
    if value.contains('\0') {
        return Err(text(TextKey::SetupApiKeyInvalid, locale).to_owned());
    }
    Ok(())
}

