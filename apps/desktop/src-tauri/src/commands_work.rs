use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use crate::work::WorkSnapshot;
use coosenpai_core::work::ApprovalDecision;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopRequest {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionRequest {
    id: String,
    decision: ApprovalDecision,
}

#[tauri::command]
pub(crate) async fn work_status(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<WorkSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(IpcResult::success(state.work.snapshot()))
}

#[tauri::command]
pub(crate) async fn work_stop(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: StopRequest,
) -> TauriIpcResult<WorkSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    if let Err(error) = state.work.stop(&payload.id) {
        return Ok(IpcResult::failure(error));
    }
    Ok(IpcResult::success(state.work.snapshot()))
}

#[tauri::command]
pub(crate) async fn work_approve(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: DecisionRequest,
) -> TauriIpcResult<WorkSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let worker = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::WorkApprove,
        move |_| async move {
            match worker.work.approvals.decide(&payload.id, payload.decision) {
                Ok(()) => IpcResult::success(worker.work.snapshot()),
                Err(error) => IpcResult::failure(error),
            }
        },
    )
    .await)
}
