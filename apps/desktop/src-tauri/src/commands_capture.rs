use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, validate_id, CommandOrigin, IpcResult, TauriIpcResult,
    MAX_CHAT_BYTES,
};
use crate::state::DesktopState;
use coosenpai_core::ports::{SystemSettingsPane, SystemSettingsPort};
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureSendPayload {
    capture_id: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentPayload {
    path: String,
}

#[tauri::command]
pub async fn capture_popup_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<crate::capture::CapturePopupSnapshot> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    Ok(
        match crate::capture::snapshot(state.inner().as_ref()).await {
            Ok(snapshot) => IpcResult::success(snapshot),
            Err(error) => IpcResult::failure(error),
        },
    )
}

#[tauri::command]
pub async fn capture_popup_send(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: CaptureSendPayload,
) -> TauriIpcResult<String> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    validate_id(&payload.capture_id)?;
    if payload.message.len() > MAX_CHAT_BYTES {
        return Ok(IpcResult::failure("message が長すぎます"));
    }
    let attachment_kind = crate::capture::snapshot(state.inner().as_ref())
        .await
        .ok()
        .map(|snapshot| snapshot.attachment_kind);
    let command = if attachment_kind == Some("text") {
        DesktopCommand::CaptureSendText
    } else {
        DesktopCommand::CaptureSendImage
    };
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcCapturePopup,
        command,
        move |context| async move {
            match crate::capture::send(
                handler_state.as_ref(),
                &context,
                &payload.capture_id,
                payload.message,
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
pub async fn capture_popup_cancel(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcCapturePopup,
        DesktopCommand::CaptureCancel,
        move |context| async move {
            crate::capture::cancel(handler_state.as_ref(), &context).await;
            IpcResult::success(())
        },
    )
    .await)
}

#[tauri::command]
pub async fn capture_popup_open_accessibility_settings(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::CapturePopup)?;
    let result = crate::platform::MacSystemSettings
        .open(
            SystemSettingsPane::Accessibility,
            state.cancellation.child_token(),
        )
        .await;
    Ok(match result {
        Ok(()) => IpcResult::success(()),
        _ => IpcResult::failure("アクセシビリティのシステム設定を開けませんでした"),
    })
}

#[tauri::command]
pub async fn attachment_read(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: AttachmentPayload,
) -> TauriIpcResult<Vec<u8>> {
    authorize_window(&window, CommandOrigin::Main)?;
    let config = state.runtime_config();
    let storage = coosenpai_core::companion_storage::CompanionStorage::from_paths(
        &state.paths,
        config.retention.conversation_days,
    );
    let path = match storage.resolve_attachment(&payload.path) {
        Ok(path) => path,
        Err(error) => return Ok(IpcResult::failure(error.to_string())),
    };
    Ok(match tokio::fs::read(path).await {
        Ok(bytes) => IpcResult::success(bytes),
        Err(_) => IpcResult::failure("添付画像を読み込めません"),
    })
}

