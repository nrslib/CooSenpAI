use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{dispatch_result, IpcResult};
use crate::snapshot::AppSnapshot;
use crate::state::tutorial_state::TutorialFinishEntry;
use crate::state::{DesktopState, TutorialResponseStatus};
use crate::tutorial::{tutorial_step_can_be_skipped, TUTORIAL_SKIP_ACTION};
use crate::ui_commands::{CommandCompletion, UserCommand};
use crate::ui_events::{UiEvent, UiView};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::SystemSettingsPort;
use std::sync::Arc;

pub(crate) async fn execute(state: Arc<DesktopState>, command: UserCommand) -> UiEvent {
    let owner = command.owner();
    let completion = match command {
        UserCommand::CaptureSnapshot(reply) => {
            let locale = Locale::from_config(&state.runtime_config().ui.language);
            let result = match crate::capture::snapshot(&state).await {
                Ok(snapshot) => {
                    IpcResult::success(crate::commands_capture::CapturePopupIpcSnapshot {
                        snapshot,
                    })
                }
                Err(error) => IpcResult::failure(coosenpai_core::locale::localize_capture_message(
                    &error, locale,
                )),
            };
            CommandCompletion::CaptureSnapshot {
                result: Box::new(result),
                reply,
            }
        }
        UserCommand::SpeechSnapshot(reply) => CommandCompletion::SpeechSnapshot {
            result: Box::new(IpcResult::success(state.speech_popup_snapshot().await)),
            reply,
        },

        UserCommand::WatchStart { source, reply } => {
            let locale = Locale::from_config(&state.runtime_config().ui.language);
            let result = match state.dispatch_watch_start(source).await {
                Ok(snapshot) => IpcResult::success(snapshot),
                Err(error) => IpcResult::failure(error.format_for_locale(locale)),
            };
            CommandCompletion::Snapshot {
                result: Box::new(result),
                reply,
            }
        }
        UserCommand::WatchStop { source, reply } => {
            let handler = state.clone();
            let result = dispatch_result(
                state,
                source,
                DesktopCommand::WatchStop,
                move |context| async move {
                    let locale = Locale::from_config(&handler.runtime_config().ui.language);
                    match handler.command_stop_watch(&context).await {
                        Ok(snapshot) => IpcResult::success(snapshot),
                        Err(error) => {
                            crate::commands::config_commit_failure_for_locale(error, locale)
                        }
                    }
                },
            )
            .await;
            CommandCompletion::Snapshot {
                result: Box::new(result),
                reply,
            }
        }
        UserCommand::EmotionsReset { source, reply } => {
            let handler = state.clone();
            let result = dispatch_result(
                state,
                source,
                DesktopCommand::CompanionEmotionsReset,
                move |context| async move {
                    let locale = Locale::from_config(&handler.runtime_config().ui.language);
                    match handler.command_reset_companion_emotions(&context).await {
                        Ok(snapshot) => IpcResult::success(snapshot),
                        Err(error) => crate::commands::runtime_failure_for_locale(error, locale),
                    }
                },
            )
            .await;
            CommandCompletion::Snapshot {
                result: Box::new(result),
                reply,
            }
        }
        UserCommand::ChatCancel(reply) => CommandCompletion::Text {
            result: chat_operation(state, false).await,
            reply,
        },
        UserCommand::ChatRetry(reply) => CommandCompletion::Text {
            result: chat_operation(state, true).await,
            reply,
        },
        UserCommand::TutorialNext(reply) => CommandCompletion::Snapshot {
            result: Box::new(tutorial_next(state).await),
            reply,
        },
        UserCommand::TutorialSettingsPresented(reply) => CommandCompletion::Snapshot {
            result: Box::new(
                tutorial_operation(state, DesktopCommand::TutorialSettingsPresented).await,
            ),
            reply,
        },
        UserCommand::TutorialFinish(reply) => CommandCompletion::Snapshot {
            result: Box::new(tutorial_operation(state, DesktopCommand::TutorialFinish).await),
            reply,
        },
        UserCommand::TutorialRestart(reply) => CommandCompletion::Snapshot {
            result: Box::new(tutorial_operation(state, DesktopCommand::TutorialRestart).await),
            reply,
        },
        UserCommand::SetupRestart(reply) => CommandCompletion::Snapshot {
            result: Box::new(tutorial_operation(state, DesktopCommand::SetupRestart).await),
            reply,
        },
        UserCommand::SetupPrompt(reply) => CommandCompletion::Snapshot {
            result: Box::new(tutorial_operation(state, DesktopCommand::SetupPrompt).await),
            reply,
        },
        UserCommand::ConversationReset(reply) => CommandCompletion::Snapshot {
            result: Box::new(tutorial_operation(state, DesktopCommand::ConversationReset).await),
            reply,
        },
        UserCommand::BubbleInteract {
            id,
            action,
            value,
            reply,
        } => CommandCompletion::Unit {
            result: bubble_interact(state, id, action, value).await,
            reply,
        },
        UserCommand::BubbleDismiss { .. } => {
            unreachable!("BubblePresenter resolves the dismiss policy")
        }
        UserCommand::BubblePreview { preview, reply } => {
            let handler = state.clone();
            let result = dispatch_result(
                state,
                CommandSource::IpcMain,
                DesktopCommand::SettingsAppearancePreview,
                move |_| async move {
                    match mutate(
                        &handler.ui,
                        crate::bubbles::BubbleMutation::Preview(preview),
                    )
                    .await
                    {
                        Ok(_) => IpcResult::success(()),
                        Err(error) => IpcResult::failure(error),
                    }
                },
            )
            .await;
            CommandCompletion::Unit { result, reply }
        }
        UserCommand::BubbleFastForward { id, reply } => {
            let handler = state.clone();
            let result = dispatch_result(
                state,
                CommandSource::IpcBubble,
                DesktopCommand::BubbleFastForward,
                move |_| async move {
                    match mutate(&handler.ui, crate::bubbles::BubbleMutation::FastForward(id)).await
                    {
                        Ok(changed) => IpcResult::success(changed),
                        Err(error) => IpcResult::failure(error),
                    }
                },
            )
            .await;
            CommandCompletion::Bool { result, reply }
        }
        UserCommand::BubbleNavigate { direction, reply } => {
            let handler = state.clone();
            let result = dispatch_result(
                state,
                CommandSource::IpcBubble,
                DesktopCommand::BubbleNavigate,
                move |_| async move {
                    match mutate(
                        &handler.ui,
                        crate::bubbles::BubbleMutation::Navigate(direction),
                    )
                    .await
                    {
                        Ok(changed) => IpcResult::success(changed),
                        Err(error) => IpcResult::failure(error),
                    }
                },
            )
            .await;
            CommandCompletion::Bool { result, reply }
        }
        UserCommand::ModelSave { .. } => {
            unreachable!("ModelPicker supplies the presentation generation")
        }
        UserCommand::SystemSettings {
            pane,
            failure,
            reply,
        } => {
            let result = match crate::platform::MacSystemSettings
                .open(pane, state.cancellation.child_token())
                .await
            {
                Ok(()) => IpcResult::success(()),
                Err(_) => IpcResult::failure(text(
                    failure,
                    Locale::from_config(&state.runtime_config().ui.language),
                )),
            };
            CommandCompletion::Unit { result, reply }
        }
    };
    UiEvent::CommandFinished { owner, completion }
}

async fn chat_operation(state: Arc<DesktopState>, retry: bool) -> IpcResult<String> {
    let handler = state.clone();
    let command = if retry {
        DesktopCommand::ChatRetry
    } else {
        DesktopCommand::ChatCancel
    };
    dispatch_result(
        state,
        CommandSource::IpcMain,
        command,
        move |_| async move {
            let result = if retry {
                handler.core_runtime().retry_user_message().await
            } else {
                handler.core_runtime().cancel_user_message().await
            };
            match result {
                Ok(id) => IpcResult::success(id),
                Err(error) => {
                    IpcResult::failure(error.format_for_locale(Locale::from_config(
                        &handler.runtime_config().ui.language,
                    )))
                }
            }
        },
    )
    .await
}

async fn tutorial_operation(
    state: Arc<DesktopState>,
    command: DesktopCommand,
) -> IpcResult<AppSnapshot> {
    let handler = state.clone();
    dispatch_result(
        state,
        CommandSource::IpcMain,
        command,
        move |context| async move {
            let locale = Locale::from_config(&handler.runtime_config().ui.language);
            let result = match command {
                DesktopCommand::TutorialSettingsPresented => handler
                    .command_tutorial_settings_presented(&context)
                    .await
                    .map_err(|error| error.format_for_locale(locale)),
                DesktopCommand::TutorialFinish => handler
                    .command_finish_tutorial(&context, TutorialFinishEntry::Main)
                    .await
                    .map_err(|error| error.format_for_locale(locale)),
                DesktopCommand::TutorialRestart => handler
                    .command_restart_tutorial(&context)
                    .await
                    .map_err(|error| error.format_for_locale(locale)),
                DesktopCommand::SetupRestart => handler
                    .command_reset_setup(&context)
                    .await
                    .map_err(|error| error.format_for_locale(locale)),
                DesktopCommand::SetupPrompt => handler
                    .command_announce_initial_onboarding(&context)
                    .await
                    .map_err(|error| error.format_for_locale(locale)),
                DesktopCommand::ConversationReset => handler
                    .command_reset_conversation(&context)
                    .await
                    .map_err(|error| error.format_for_locale(locale)),
                _ => unreachable!("tutorial snapshot operation"),
            };
            match result {
                Ok(()) => IpcResult::success(handler.snapshot().await),
                Err(error) => IpcResult::failure(error),
            }
        },
    )
    .await
}

pub(crate) async fn bubble_interact(
    state: Arc<DesktopState>,
    id: String,
    action: String,
    value: Option<String>,
) -> IpcResult<()> {
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    if action == "watch-fullscreen-confirm" {
        return match state
            .dispatch_watch_fullscreen_consent(CommandSource::IpcBubble, id)
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(error) => IpcResult::failure(error.format_for_locale(locale)),
        };
    }
    let command = bubble_interaction_command(&action);
    let handler = state.clone();
    dispatch_result(
        state,
        CommandSource::IpcBubble,
        command,
        move |context| async move {
            match handler
                .command_handle_bubble_interaction(&context, &id, &action, value.as_deref())
                .await
            {
                Ok(()) => IpcResult::success(()),
                Err(error) => {
                    IpcResult::failure(error.format_for_locale(Locale::from_config(
                        &handler.runtime_config().ui.language,
                    )))
                }
            }
        },
    )
    .await
}

pub(crate) async fn mutate(
    ui: &crate::ui_root::UiHandle,
    mutation: crate::bubbles::BubbleMutation,
) -> Result<bool, String> {
    let (reply, response) = tokio::sync::oneshot::channel();
    ui.request(UiView::Bubble, UiEvent::BubbleMutation { mutation, reply })
        .await?;
    response
        .await
        .map_err(|_| "吹き出しの応答がありません".to_owned())
}

async fn tutorial_next(state: Arc<DesktopState>) -> IpcResult<AppSnapshot> {
    let handler_state = state.clone();
    let dispatched = dispatch_result(
        state.clone(),
        CommandSource::IpcMain,
        DesktopCommand::TutorialAdvance,
        move |context| async move {
            let step = handler_state.tutorial_current_step().await;
            let locale = Locale::from_config(&handler_state.runtime_config().ui.language);
            if step.is_some_and(|step| !tutorial_step_can_be_skipped(step)) {
                return IpcResult::failure(text(TextKey::TutorialAutoAdvance, locale));
            }
            let result = match step {
                Some(step) => {
                    let response_presented = handler_state.tutorial_step_response_presented().await;
                    let response_status = if response_presented {
                        TutorialResponseStatus::None
                    } else {
                        match handler_state.tutorial_response_status(step).await {
                            Ok(status) => status,
                            Err(error) => {
                                return IpcResult::failure(error.format_for_locale(locale))
                            }
                        }
                    };
                    match tutorial_advance_action(response_presented, response_status) {
                        TutorialAdvanceAction::Finish { skipped } => handler_state
                            .command_finish_tutorial_step(&context, step, skipped)
                            .await
                            .map_err(|error| error.format_for_locale(locale)),
                        TutorialAdvanceAction::PresentSavedResponse => {
                            if !handler_state
                                .command_accept_saved_tutorial_response(&context)
                                .await
                            {
                                return IpcResult::failure(text(
                                    TextKey::SetupSavedResponseUnavailable,
                                    locale,
                                ));
                            }
                            handler_state
                                .command_finish_tutorial_step(&context, step, false)
                                .await
                                .map_err(|error| error.format_for_locale(locale))
                        }
                        TutorialAdvanceAction::WaitForResponse => {
                            return IpcResult::failure(text(TextKey::SetupResponsePending, locale));
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

pub(crate) fn tutorial_advance_action(
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

pub(crate) fn bubble_interaction_command(action: &str) -> DesktopCommand {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TutorialAdvanceAction {
    Finish { skipped: bool },
    PresentSavedResponse,
    WaitForResponse,
}

pub(crate) async fn dismiss_bubble(
    state: Arc<DesktopState>,
    id: String,
    restart_setup: bool,
    reply: crate::ui_commands::Reply<()>,
) -> UiEvent {
    let result = dismiss_bubble_result(state, id, restart_setup).await;
    UiEvent::CommandFinished {
        owner: crate::ui_events::PresenterId::Bubble,
        completion: CommandCompletion::Unit { result, reply },
    }
}

pub(crate) async fn dismiss_bubble_result(
    state: Arc<DesktopState>,
    id: String,
    restart_setup: bool,
) -> IpcResult<()> {
    let handler = state.clone();
    let command = if restart_setup {
        DesktopCommand::SetupRestart
    } else {
        DesktopCommand::BubbleDismiss
    };
    dispatch_result(
        state,
        CommandSource::IpcBubble,
        command,
        move |context| async move {
            if restart_setup {
                match handler.command_dismiss_setup_bubble(&context, &id).await {
                    Ok(()) => IpcResult::success(()),
                    Err(error) => IpcResult::failure(error.format_for_locale(Locale::from_config(
                        &handler.runtime_config().ui.language,
                    ))),
                }
            } else {
                crate::bubbles::dismiss(&handler, &id).await;
                IpcResult::success(())
            }
        },
    )
    .await
}

pub(crate) async fn save_model(
    state: Arc<DesktopState>,
    patch: serde_json::Value,
    generation: u64,
    reply: crate::ui_commands::Reply<coosenpai_core::config::Config>,
) -> UiEvent {
    let result = crate::commands::update_config_for_source(
        state,
        patch,
        None,
        None,
        CommandSource::IpcModelPopup,
    )
    .await;
    UiEvent::ModelSaved {
        generation,
        result: Box::new(result),
        reply,
    }
}
