use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, CommandOrigin, IpcResult, TauriIpcResult, MAX_CHAT_BYTES,
};
use crate::speech::{SpeechPopupSnapshot, SpeechSource};
use crate::state::DesktopState;
use coosenpai_core::ports::{SystemSettingsPane, SystemSettingsPort};
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechStartPayload {
    source: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechSendPayload {
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechSettingsPayload {
    kind: String,
}

#[tauri::command]
pub async fn speech_start(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechStartPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    if payload.source != "composer" {
        return Ok(IpcResult::failure("source は composer で指定してください"));
    }
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::SpeechStart,
        move |context| async move {
            match handler_state
                .command_speech_begin(&context, SpeechSource::Composer)
                .await
            {
                Ok(()) => IpcResult::success(()),
                Err(message) => IpcResult::failure(message),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn speech_finish(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::SpeechFinish,
        move |context| async move {
            handler_state.command_speech_finish(&context);
            IpcResult::success(())
        },
    )
    .await)
}

#[tauri::command]
pub async fn speech_cancel(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::SpeechCancel,
        move |context| async move {
            match handler_state.command_speech_cancel(&context) {
                Ok(()) => IpcResult::success(()),
                Err(message) => IpcResult::failure(message),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn speech_popup_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<SpeechPopupSnapshot> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    Ok(IpcResult::success(state.speech_popup_snapshot().await))
}

#[tauri::command]
pub async fn speech_popup_send(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechSendPayload,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    if payload.text.trim().is_empty() {
        return Ok(IpcResult::failure("text は空にできません"));
    }
    if payload.text.len() > MAX_CHAT_BYTES {
        return Ok(IpcResult::failure("text が長すぎます"));
    }
    let state = state.inner().clone();
    let Some(generation) = state.speech_confirming_generation() else {
        return Ok(IpcResult::failure("確認する音声入力がありません"));
    };
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcSpeechPopup,
        DesktopCommand::SpeechConfirm,
        move |context| async move {
            if context
                .fence(crate::command_guard::GenerationResource::Speech)
                .is_none_or(|stamp| stamp.value != generation)
            {
                return IpcResult::failure("古い音声入力です");
            }
            match handler_state
                .command_speech_confirm(&context, payload.text)
                .await
            {
                Ok(id) => IpcResult::success(id),
                Err(message) => IpcResult::failure(message),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn speech_popup_cancel(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcSpeechPopup,
        DesktopCommand::SpeechCancel,
        move |context| async move {
            match handler_state.command_speech_cancel(&context) {
                Ok(()) => IpcResult::success(()),
                Err(message) => IpcResult::failure(message),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn speech_open_system_settings(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechSettingsPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let pane = match speech_settings_pane(&payload.kind) {
        Ok(pane) => pane,
        Err(message) => return Ok(IpcResult::failure(message)),
    };
    let result = crate::platform::MacSystemSettings
        .open(pane, state.cancellation.child_token())
        .await;
    Ok(match result {
        Ok(()) => IpcResult::success(()),
        _ => IpcResult::failure(speech_settings_open_error(&payload.kind)),
    })
}

fn speech_settings_open_error(kind: &str) -> &'static str {
    if kind == "recognition" {
        "音声認識のシステム設定を開けませんでした"
    } else {
        "マイクのシステム設定を開けませんでした"
    }
}

fn speech_settings_pane(kind: &str) -> Result<SystemSettingsPane, &'static str> {
    match kind {
        "microphone" => Ok(SystemSettingsPane::Microphone),
        "recognition" => Ok(SystemSettingsPane::SpeechRecognition),
        _ => Err("kind は microphone または recognition です"),
    }
}

