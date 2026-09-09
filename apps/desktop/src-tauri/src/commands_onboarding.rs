use crate::commands::{
    authorize_window, validate_id_for_locale, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
#[cfg(test)]
use crate::ui_command_effects::{
    bubble_interaction_command, tutorial_advance_action, TutorialAdvanceAction,
};
use crate::ui_commands::UserCommand;
use crate::ui_events::{UiEvent, UiView};
use coosenpai_core::locale::{text, Locale, TextKey};
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BubbleInteractionPayload {
    id: String,
    action: String,
    value: Option<String>,
}

#[tauri::command]
pub async fn bubble_interact(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleInteractionPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.id, locale)?;
    validate_id_for_locale(&payload.action, locale)?;
    if payload.value.as_ref().is_some_and(|value| value.is_empty()) {
        return Ok(IpcResult::failure(text(
            TextKey::SetupSelectionEmpty,
            locale,
        )));
    }
    state
        .ui
        .query(UiView::Bubble, |reply| {
            UiEvent::UserCommand(UserCommand::BubbleInteract {
                id: payload.id,
                action: payload.action,
                value: payload.value,
                reply,
            })
        })
        .await
}

#[tauri::command]
pub async fn tutorial_next(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(tutorial_next_for_state(state.inner().clone()).await)
}

pub(crate) async fn tutorial_next_for_state(state: Arc<DesktopState>) -> IpcResult<AppSnapshot> {
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::TutorialNext(reply))
        })
        .await
        .unwrap_or_else(IpcResult::failure)
}

#[tauri::command]
pub async fn tutorial_settings_presented(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::TutorialSettingsPresented(reply))
        })
        .await
}

#[tauri::command]
pub async fn tutorial_finish(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::TutorialFinish(reply))
        })
        .await
}

#[tauri::command]
pub async fn tutorial_restart(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::TutorialRestart(reply))
        })
        .await
}

#[tauri::command]
pub async fn setup_restart(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::SetupRestart(reply))
        })
        .await
}

#[tauri::command]
pub async fn setup_prompt(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::SetupPrompt(reply))
        })
        .await
}

#[tauri::command]
pub async fn conversation_reset(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(conversation_reset_for_state(state.inner().clone()).await)
}

pub(crate) async fn conversation_reset_for_state(
    state: Arc<DesktopState>,
) -> IpcResult<AppSnapshot> {
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::ConversationReset(reply))
        })
        .await
        .unwrap_or_else(IpcResult::failure)
}

