use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, validate_id, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::snapshot::AppSnapshot;
use crate::state::tutorial_state::TutorialFinishEntry;
use crate::state::{DesktopState, TutorialResponseStatus};
use crate::tutorial::{
    tutorial_step_can_be_skipped, TUTORIAL_AUTO_ADVANCE_MESSAGE, TUTORIAL_SKIP_ACTION,
};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TutorialAdvanceAction {
    Finish { skipped: bool },
    PresentSavedResponse,
    WaitForResponse,
}

fn tutorial_advance_action(
    response_presented: bool,
    response_status: TutorialResponseStatus,
) -> TutorialAdvanceAction {
    if response_presented {
        return TutorialAdvanceAction::Finish { skipped: false };
    }
    match response_status {
        TutorialResponseStatus::None => TutorialAdvanceAction::Finish { skipped: true },
        TutorialResponseStatus::Pending => TutorialAdvanceAction::WaitForResponse,
        TutorialResponseStatus::SavedForPresentation => TutorialAdvanceAction::PresentSavedResponse,
    }
}

#[tauri::command]
pub async fn bubble_interact(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleInteractionPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    validate_id(&payload.id)?;
    validate_id(&payload.action)?;
    if payload.value.as_ref().is_some_and(|value| value.is_empty()) {
        return Ok(IpcResult::failure("選択値は空にできません"));
    }
    let command = bubble_interaction_command(&payload.action);
    let state = state.inner().clone();
    if payload.action == "watch-fullscreen-confirm" {
        let result = state
            .dispatch_watch_fullscreen_consent(CommandSource::IpcBubble, payload.id)
            .await;
        return Ok(match result {
            Ok(_) => IpcResult::success(()),
            Err(error) => IpcResult::failure(error.format_for_user()),
        });
    }
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcBubble,
        command,
        move |context| async move {
            match handler_state
                .command_handle_bubble_interaction(
                    &context,
                    &payload.id,
                    &payload.action,
                    payload.value.as_deref(),
                )
                .await
            {
                Ok(()) => IpcResult::success(()),
                Err(error) => IpcResult::failure(error.format_for_user()),
            }
        },
    )
    .await)
}

fn bubble_interaction_command(action: &str) -> DesktopCommand {
    match action {
        "memory-confirm" => DesktopCommand::MemoryConfirm,
        "memory-reject" => DesktopCommand::MemoryReject,
        "conversation-reset-confirm" => DesktopCommand::ConversationReset,
        "conversation-reset-cancel" => DesktopCommand::ConversationResetDismiss,
        "watch-fullscreen-confirm" => DesktopCommand::ConfigWatchUpdate,
        "watch-fullscreen-settings" => DesktopCommand::SettingsOpen,
        TUTORIAL_SKIP_ACTION => DesktopCommand::TutorialAdvance,
        _ => DesktopCommand::TutorialInteract,
    }
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
    let handler_state = state.clone();
    let dispatched = dispatch_result(
        state.clone(),
        CommandSource::IpcMain,
        DesktopCommand::TutorialAdvance,
        move |context| async move {
            let step = handler_state.tutorial_current_step().await;
            if step.is_some_and(|step| !tutorial_step_can_be_skipped(step)) {
                return IpcResult::failure(TUTORIAL_AUTO_ADVANCE_MESSAGE);
            }
            let result = match step {
                Some(step) => {
                    let response_presented = handler_state.tutorial_step_response_presented().await;
                    let response_status = if response_presented {
                        TutorialResponseStatus::None
                    } else {
                        match handler_state.tutorial_response_status(step).await {
                            Ok(status) => status,
                            Err(error) => return IpcResult::failure(error.to_string()),
                        }
                    };
                    match tutorial_advance_action(response_presented, response_status) {
                        TutorialAdvanceAction::Finish { skipped } => handler_state
                            .command_finish_tutorial_step(&context, step, skipped)
                            .await
                            .map_err(|error| error.to_string()),
                        TutorialAdvanceAction::PresentSavedResponse => {
                            if !handler_state
                                .command_accept_saved_tutorial_response(&context)
                                .await
                            {
                                return IpcResult::failure(
                                    "保存済みの返事を表示済みにできませんでした",
                                );
                            }
                            handler_state
                                .command_finish_tutorial_step(&context, step, false)
                                .await
                                .map_err(|error| error.to_string())
                        }
                        TutorialAdvanceAction::WaitForResponse => {
                            return IpcResult::failure(
                                "返事の表示が終わるまで、この案内を進められません",
                            );
                        }
                    }
                }
                None => Ok(()),
            };
            match result {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error),
            }
        },
    )
    .await;
    dispatched
}

#[tauri::command]
pub async fn tutorial_settings_presented(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::TutorialSettingsPresented,
        move |context| async move {
            match handler_state
                .command_tutorial_settings_presented(&context)
                .await
            {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn tutorial_finish(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::TutorialFinish,
        move |context| async move {
            match handler_state
                .command_finish_tutorial(&context, TutorialFinishEntry::Main)
                .await
            {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error.format_for_user()),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn tutorial_restart(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::TutorialRestart,
        move |context| async move {
            match handler_state.command_restart_tutorial(&context).await {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error.format_for_user()),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn setup_restart(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::SetupRestart,
        move |context| async move {
            match handler_state.command_reset_setup(&context).await {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn setup_prompt(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::SetupPrompt,
        move |context| async move {
            match handler_state
                .command_announce_initial_onboarding(&context)
                .await
            {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await)
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
    let handler_state = state.clone();
    dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::ConversationReset,
        move |context| async move {
            match handler_state.command_reset_conversation(&context).await {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await
}

