use crate::commands::{main_or_details_source, TauriIpcResult};
use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
use crate::ui_commands::UserCommand;
use crate::ui_events::{UiEvent, UiView};
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
    state
        .ui
        .query(UiView::Chat, |reply| {
            UiEvent::UserCommand(UserCommand::ConversationSelect {
                generation: payload.generation,
                source,
                reply,
            })
        })
        .await
}
