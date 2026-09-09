use crate::commands::{
    authorize_window, validate_id_for_locale, CommandOrigin, IpcResult, TauriIpcResult,
    MAX_CHAT_BYTES,
};
use crate::state::DesktopState;
use crate::ui_commands::UserCommand;
use crate::ui_events::{UiEvent, UiView};
use coosenpai_core::locale::{
    localize_capture_message, localize_error_message, text, Locale, TextKey,
};
use coosenpai_core::ports::SystemSettingsPane;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureSendPayload {
    capture_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentPayload {
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePopupIpcSnapshot {
    #[serde(flatten)]
    pub(crate) snapshot: crate::capture::CapturePopupSnapshot,
}

#[tauri::command]
pub async fn capture_popup_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<CapturePopupIpcSnapshot> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    state
        .ui
        .query(UiView::CapturePopup, |reply| {
            UiEvent::UserCommand(UserCommand::CaptureSnapshot(reply))
        })
        .await
}

#[tauri::command]
pub async fn capture_popup_send(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: CaptureSendPayload,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.capture_id, locale)?;
    let (reply, result) = tokio::sync::oneshot::channel();
    state.ui.input(
        UiView::CapturePopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::PopupSubmit {
            capture_id: payload.capture_id,
            reply,
        })),
    );
    Ok(match result.await {
        Ok(Ok(id)) => IpcResult::success(id),
        Ok(Err(error)) => IpcResult::failure(localize_capture_message(&error, locale)),
        Err(_) => IpcResult::failure("入力の受付は終了しています"),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureCancelPayload {
    generation: u64,
    source: CaptureCancelSource,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureEditPayload {
    generation: u64,
    edit_revision: u64,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureKeyPayload {
    generation: u64,
    input: crate::capture::PopupKeyInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureActionPayload {
    generation: u64,
    index: usize,
}

#[tauri::command]
pub async fn capture_popup_edit(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: CaptureEditPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    if payload.message.len() > MAX_CHAT_BYTES {
        return Err("入力文が長すぎます".to_owned());
    }
    state.ui.input(
        UiView::CapturePopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::PopupEdit {
            generation: payload.generation,
            edit_revision: payload.edit_revision,
            message: payload.message,
        })),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn capture_popup_key(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: CaptureKeyPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    state.ui.input(
        UiView::CapturePopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::PopupKey {
            generation: payload.generation,
            input: payload.input,
        })),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn capture_popup_action(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: CaptureActionPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    state.ui.input(
        UiView::CapturePopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::PopupAction {
            generation: payload.generation,
            index: payload.index,
        })),
    );
    Ok(IpcResult::success(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CaptureCancelSource {
    Esc,
    CloseButton,
}

#[tauri::command]
pub async fn capture_popup_cancel(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: CaptureCancelPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    let source = match payload.source {
        CaptureCancelSource::Esc => crate::capture::CancelSource::Esc,
        CaptureCancelSource::CloseButton => crate::capture::CancelSource::CloseButton,
    };
    state.ui.input(
        UiView::CapturePopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::PopupCancel {
            generation: payload.generation,
            source,
        })),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn capture_popup_open_accessibility_settings(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    state
        .ui
        .query(UiView::CapturePopup, |reply| {
            UiEvent::UserCommand(UserCommand::SystemSettings {
                pane: SystemSettingsPane::Accessibility,
                failure: TextKey::CaptureAccessibilitySettingsFailed,
                reply,
            })
        })
        .await
}

#[tauri::command]
pub async fn attachment_read(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: AttachmentPayload,
) -> TauriIpcResult<Vec<u8>> {
    authorize_window(&window, CommandOrigin::Main)?;
    let config = state.runtime_config();
    let locale = Locale::from_config(&config.ui.language);
    let storage = coosenpai_core::companion_storage::CompanionStorage::from_paths(
        &state.paths,
        config.retention.conversation_days,
    );
    let path = match storage.resolve_attachment(&payload.path) {
        Ok(path) => path,
        Err(error) => {
            return Ok(IpcResult::failure(localize_error_message(
                &error.to_string(),
                TextKey::AttachmentReadFailed,
                locale,
            )))
        }
    };
    Ok(match tokio::fs::read(path).await {
        Ok(bytes) => IpcResult::success(bytes),
        Err(_) => IpcResult::failure(text(TextKey::AttachmentReadFailed, locale)),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRetryPayload {
    generation: u64,
}

#[tauri::command]
pub async fn capture_popup_retry(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: CaptureRetryPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    state.ui.input(
        UiView::CapturePopup,
        UiEvent::CaptureCompleted(Box::new(crate::capture::CaptureEvent::PopupRetry {
            generation: payload.generation,
        })),
    );
    Ok(IpcResult::success(()))
}
