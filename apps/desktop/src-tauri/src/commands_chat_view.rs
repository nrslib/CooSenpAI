use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult, MAX_CHAT_BYTES};
use crate::composer_presenter::{ComposerEvent, ComposerInput};
use crate::conversation_presenter::{ConversationEvent, ConversationInput};
use crate::state::DesktopState;
use crate::ui_events::{UiEvent, UiView};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn composer_input(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: ComposerInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let text = match &payload {
        ComposerInput::Edit { text, .. }
        | ComposerInput::Submit { text, .. }
        | ComposerInput::Composition { text, .. }
        | ComposerInput::Key { text, .. } => Some(text),
        _ => None,
    };
    if text.is_some_and(|text| text.len() > MAX_CHAT_BYTES) {
        return Err("入力文が長すぎます".into());
    }
    state.ui.input(
        UiView::Chat,
        UiEvent::Composer(ComposerEvent::Input(payload)),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn conversation_input(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: ConversationInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    if let ConversationInput::Action { input_id, .. } = &payload {
        crate::commands::validate_id_for_locale(
            input_id,
            coosenpai_core::locale::Locale::from_config(&state.runtime_config().ui.language),
        )?;
    }
    state.ui.input(
        UiView::Chat,
        UiEvent::Conversation(ConversationEvent::Input(payload)),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn work_approval_input(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: crate::work_approval_presenter::WorkApprovalInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    use crate::work_approval_presenter::WorkApprovalInput;
    let (WorkApprovalInput::Mounted { input_id }
    | WorkApprovalInput::Unmounted { input_id }
    | WorkApprovalInput::Action { input_id, .. }) = &payload;
    if input_id.is_empty() || input_id.len() > 128 {
        return Err("作業IDが不正です".into());
    }
    state.ui.input(
        UiView::Chat,
        UiEvent::WorkApproval(crate::work_approval_presenter::WorkApprovalEvent::Input(
            payload,
        )),
    );
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn app_view_input(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: crate::app_presenter::AppInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    state.ui.input(
        UiView::Chat,
        UiEvent::App(crate::app_presenter::AppEvent::Input(payload)),
    );
    Ok(IpcResult::success(()))
}
#[tauri::command]
pub async fn app_select_persona(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    persona: String,
) -> TauriIpcResult<coosenpai_core::config::Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    crate::commands::validate_id_for_locale(
        &persona,
        coosenpai_core::locale::Locale::from_config(&state.runtime_config().ui.language),
    )?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::App(crate::app_presenter::AppEvent::SelectPersona { persona, reply })
        })
        .await
}

#[tauri::command]
pub async fn avatar_scene_input(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: crate::avatar_scene_presenter::AvatarSceneInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Avatar)?;
    state
        .ui
        .input(UiView::Avatar, UiEvent::AvatarScene(payload));
    Ok(IpcResult::success(()))
}
#[tauri::command]
pub async fn motion_settings_input(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: crate::motion_settings_presenter::MotionInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    state.ui.input(
        UiView::Settings,
        UiEvent::MotionSettings(crate::motion_settings_presenter::MotionEvent::Input(
            payload,
        )),
    );
    Ok(IpcResult::success(()))
}
