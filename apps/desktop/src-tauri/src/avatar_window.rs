use crate::avatar_presenter::AvatarState;
use crate::commands::{authorize, CommandOrigin, IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

fn authorize_read_or_hide(label: &str) -> Result<(), String> {
    authorize(label, CommandOrigin::Main).or_else(|_| authorize(label, CommandOrigin::Avatar))
}

#[tauri::command]
pub(crate) async fn avatar_state_get(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AvatarState> {
    authorize_read_or_hide(window.label())?;
    read(&state, false).await
}

async fn visibility_input(
    window: &WebviewWindow,
    state: &DesktopState,
    requested: Option<bool>,
) -> IpcResult<()> {
    let view = if window.label() == "avatar" {
        crate::ui_events::UiView::Avatar
    } else {
        crate::ui_events::UiView::Chat
    };
    match state
        .ui
        .request(view, crate::ui_events::UiEvent::AvatarVisibility(requested))
        .await
    {
        Ok(_) => IpcResult::success(()),
        Err(error) => IpcResult::failure(error),
    }
}

#[tauri::command]
pub(crate) async fn avatar_show(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize(window.label(), CommandOrigin::Main)?;
    Ok(visibility_input(&window, &state, Some(true)).await)
}

#[tauri::command]
pub(crate) async fn avatar_hide(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_read_or_hide(window.label())?;
    Ok(visibility_input(&window, &state, Some(false)).await)
}

#[tauri::command]
pub(crate) async fn avatar_toggle(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize(window.label(), CommandOrigin::Main)?;
    Ok(visibility_input(&window, &state, None).await)
}

#[tauri::command]
pub(crate) async fn avatar_model_changed(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AvatarState> {
    authorize(window.label(), CommandOrigin::Main)?;
    read(&state, true).await
}

async fn read(state: &DesktopState, model_changed: bool) -> TauriIpcResult<AvatarState> {
    let (reply, response) = tokio::sync::oneshot::channel();
    state
        .ui
        .request(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::AvatarRead {
                model_changed,
                reply,
            },
        )
        .await?;
    Ok(IpcResult::success(response.await.map_err(|_| {
        "アバター状態を取得できませんでした".to_owned()
    })?))
}
