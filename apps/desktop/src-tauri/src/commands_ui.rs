use crate::commands::{IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use crate::ui_events::{UiEvent, UiView};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[tauri::command]
pub(crate) async fn ui_view_mounted<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    ui: State<'_, crate::ui_root::UiHandle>,
) -> TauriIpcResult<()> {
    let view = match window.label() {
        "main" => UiView::Chat,
        "capture-popup" => UiView::CapturePopup,
        "speech-popup" => UiView::SpeechPopup,
        "details" => UiView::Details,
        "model-popup" => UiView::ModelPicker,
        "bubble" => UiView::Bubble,
        "avatar" => UiView::Avatar,
        _ => return Err("未登録のViewです".to_owned()),
    };
    Ok(match ui.request(view, UiEvent::Mounted(view)).await {
        Ok(_) => IpcResult::success(()),
        Err(message) => IpcResult::failure(message),
    })
}

#[tauri::command]
pub(crate) async fn settings_close(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    crate::commands::authorize_window(&window, crate::commands::CommandOrigin::Main)?;
    Ok(
        match state.ui.request(UiView::Settings, UiEvent::Close).await {
            Ok(_) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[tauri::command]
pub(crate) async fn ui_panel_event(
    window: WebviewWindow,
    ui: State<'_, crate::ui_root::UiHandle>,
    payload: crate::panels::PanelRequest,
) -> TauriIpcResult<crate::panels::PanelOutput> {
    if !payload.kind.allowed(window.label())
        || payload.session.is_empty()
        || payload.session.len() > 128
    {
        return Err("このパネル操作は許可されていません".into());
    }
    let owner = payload.kind.owner(window.label() == "details");
    let result = ui
        .query(UiView::Application, |reply| UiEvent::Panel {
            owner,
            request: payload,
            reply,
        })
        .await?;
    Ok(match result {
        Ok(output) => IpcResult::success(output),
        Err(error) => IpcResult::failure(error),
    })
}
