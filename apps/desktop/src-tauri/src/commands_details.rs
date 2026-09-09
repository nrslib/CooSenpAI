use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult};
use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn details_open(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Chat,
                crate::ui_events::UiEvent::OpenDetails,
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[tauri::command]
pub async fn details_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Details)?;
    Ok(IpcResult::success(state.snapshot().await))
}

const DATAFLOW_LOG_LIMIT: usize = 200;

#[tauri::command]
pub async fn details_dataflow_log(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<coosenpai_core::dataflow_log::DataFlowLog> {
    authorize_window(&window, CommandOrigin::Details)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    Ok(
        match coosenpai_core::dataflow_log::read_dataflow_log(&state.paths, DATAFLOW_LOG_LIMIT) {
            Ok(log) => IpcResult::success(log),
            Err(error) => IpcResult::failure(
                text(TextKey::DetailsDataflowLogReadFailed, locale)
                    .replace("{error}", &error.to_string()),
            ),
        },
    )
}
