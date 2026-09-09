use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult, MAX_CHAT_BYTES};
use crate::speech::{SpeechPopupSnapshot, SpeechSource};
use crate::state::DesktopState;
use crate::ui_commands::UserCommand;
use crate::ui_events::{UiEvent, UiView};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::SystemSettingsPane;
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
    generation: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechSettingsPayload {
    kind: String,
}

pub(crate) async fn start_composer_voice(
    ui: &crate::ui_root::UiHandle,
) -> Result<Option<String>, String> {
    ui.request(
        crate::ui_events::UiView::Chat,
        crate::ui_events::UiEvent::Voice(crate::ui_events::VoiceAction::Start(
            SpeechSource::Composer,
        )),
    )
    .await
}

#[tauri::command]
pub async fn speech_start(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechStartPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    if payload.source != "composer" {
        return Ok(IpcResult::failure(text(
            TextKey::SpeechSourceInvalid,
            locale,
        )));
    }
    Ok(match start_composer_voice(&state.ui).await {
        Ok(_) => IpcResult::success(()),
        Err(message) => IpcResult::failure(message),
    })
}

#[tauri::command]
pub async fn speech_finish(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Chat,
                crate::ui_events::UiEvent::Voice(crate::ui_events::VoiceAction::Finish),
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[tauri::command]
pub async fn speech_cancel(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Chat,
                crate::ui_events::UiEvent::Voice(crate::ui_events::VoiceAction::Cancel),
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[tauri::command]
pub async fn speech_popup_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<SpeechPopupSnapshot> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    state
        .ui
        .query(UiView::SpeechPopup, |reply| {
            UiEvent::UserCommand(UserCommand::SpeechSnapshot(reply))
        })
        .await
}

#[tauri::command]
pub async fn speech_popup_send(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechSendPayload,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    let (reply, result) = tokio::sync::oneshot::channel();
    state.ui.input(
        UiView::SpeechPopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::VoiceSend {
            generation: payload.generation,
            reply,
        })),
    );
    Ok(match result.await {
        Ok(Ok(id)) => IpcResult::success(id),
        Ok(Err(message)) => IpcResult::failure(message),
        Err(_) => IpcResult::failure("音声入力の受付は終了しています"),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeechEditPayload {
    generation: u64,
    edit_revision: u64,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechKeyPayload {
    generation: u64,
    input: crate::capture::PopupKeyInput,
}

#[tauri::command]
pub async fn speech_popup_edit(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechEditPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    if payload.text.len() > MAX_CHAT_BYTES {
        return Ok(IpcResult::failure("文章が長すぎます"));
    }
    state.ui.input(
        UiView::SpeechPopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::VoiceEdit {
            generation: payload.generation,
            edit_revision: payload.edit_revision,
            text: payload.text,
        })),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn speech_popup_key(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechKeyPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    state.ui.input(
        UiView::SpeechPopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::VoiceKey {
            generation: payload.generation,
            input: payload.input,
        })),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn speech_popup_cancel(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechSendPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::SpeechPopup)?;
    state.ui.input(
        UiView::SpeechPopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::VoiceCancel {
            generation: payload.generation,
        })),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn speech_open_system_settings(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeechSettingsPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let pane = match speech_settings_pane_for_locale(&payload.kind, locale) {
        Ok(pane) => pane,
        Err(message) => return Ok(IpcResult::failure(message)),
    };
    let failure = if payload.kind == "recognition" {
        TextKey::SpeechRecognitionSettingsFailed
    } else {
        TextKey::SpeechMicrophoneSettingsFailed
    };
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::SystemSettings {
                pane,
                failure,
                reply,
            })
        })
        .await
}

fn speech_settings_pane_for_locale(
    kind: &str,
    locale: Locale,
) -> Result<SystemSettingsPane, &'static str> {
    match kind {
        "microphone" => Ok(SystemSettingsPane::Microphone),
        "recognition" => Ok(SystemSettingsPane::SpeechRecognition),
        _ => Err(text(TextKey::SpeechSettingsKindInvalid, locale)),
    }
}

