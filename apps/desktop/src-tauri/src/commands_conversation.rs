use crate::command_guard::DesktopCommand;
use crate::commands::{dispatch_result, main_or_details_source, IpcResult, TauriIpcResult};
use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
use coosenpai_core::locale::Locale;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSelectPayload {
    generation: u64,
}

#[tauri::command]
pub async fn conversation_select(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: ConversationSelectPayload,
) -> TauriIpcResult<AppSnapshot> {
    let source = main_or_details_source(&window)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    Ok(dispatch_result(
        state,
        source,
        DesktopCommand::ConversationSelect,
        move |context| async move {
            match handler_state
                .command_select_conversation(&context, payload.generation)
                .await
            {
                Ok(()) => IpcResult::success(handler_state.snapshot().await),
                Err(error) => IpcResult::failure(error.format_for_locale(locale)),
            }
        },
    )
    .await)
}
