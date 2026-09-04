use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, config_commit_failure, dispatch_result, validate_id, CommandOrigin,
    IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use coosenpai_core::persona_store::{PersonaStore, PersonaVersion};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonaGetPayload {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonaSavePayload {
    id: String,
    display_name: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonaRestorePayload {
    id: String,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaDocument {
    id: String,
    body: String,
    builtin: bool,
    versions: Vec<PersonaVersion>,
}

#[tauri::command]
pub async fn persona_get(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaGetPayload,
) -> TauriIpcResult<PersonaDocument> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id(&payload.id)?;
    let store = match PersonaStore::from_paths(&state.paths) {
        Ok(store) => store,
        Err(error) => return Ok(IpcResult::failure(error.to_string())),
    };
    let entries = match store.list() {
        Ok(entries) => entries,
        Err(error) => return Ok(IpcResult::failure(error.to_string())),
    };
    let builtin = entries
        .into_iter()
        .find(|value| value.id == payload.id)
        .is_some_and(|value| value.builtin);
    Ok(match store.load_body(&payload.id) {
        Ok(body) => IpcResult::success(PersonaDocument {
            id: payload.id.clone(),
            body,
            builtin,
            versions: match (builtin, store.versions(&payload.id)) {
                (true, _) => Vec::new(),
                (false, Ok(versions)) => versions,
                (false, Err(error)) => return Ok(IpcResult::failure(error.to_string())),
            },
        }),
        Err(error) => IpcResult::failure(error.to_string()),
    })
}

#[tauri::command]
pub async fn persona_save(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaSavePayload,
) -> TauriIpcResult<coosenpai_core::config::Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id(&payload.id)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaSave,
        move |context| async move {
            let store = match PersonaStore::from_paths(&handler_state.paths) {
                Ok(store) => store,
                Err(error) => return IpcResult::failure(error.to_string()),
            };
            let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
            if let Err(error) = store.save_custom(
                &payload.id,
                &payload.display_name,
                &payload.body,
                &timestamp,
            ) {
                return IpcResult::failure(error.to_string());
            }
            match handler_state
                .command_switch_persona(&context, payload.id)
                .await
            {
                Ok(config) => IpcResult::success(config),
                Err(error) => IpcResult::failure(error.format_for_user()),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn persona_delete(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaGetPayload,
) -> TauriIpcResult<coosenpai_core::config::Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id(&payload.id)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaDelete,
        move |context| async move {
            let store = match PersonaStore::from_paths(&handler_state.paths) {
                Ok(store) => store,
                Err(error) => return IpcResult::failure(error.to_string()),
            };
            let selected = handler_state.runtime_config().companion.persona;
            let config = if persona_fallback_after_delete(&selected, &payload.id).is_some() {
                match handler_state
                    .command_switch_persona(&context, "coo-chan".to_owned())
                    .await
                {
                    Ok(config) => config,
                    Err(error) => return IpcResult::failure(error.format_for_user()),
                }
            } else {
                handler_state.runtime_config()
            };
            if let Err(error) = store.delete_custom(&payload.id) {
                return IpcResult::failure(error.to_string());
            }
            IpcResult::success(config)
        },
    )
    .await)
}

fn persona_fallback_after_delete(selected: &str, deleted: &str) -> Option<&'static str> {
    (selected == deleted).then_some("coo-chan")
}

#[tauri::command]
pub async fn persona_restore(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaRestorePayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id(&payload.id)?;
    validate_id(&payload.version)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaRestore,
        move |context| async move {
            let store = match PersonaStore::from_paths(&handler_state.paths) {
                Ok(store) => store,
                Err(error) => return IpcResult::failure(error.to_string()),
            };
            let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
            if let Err(error) = store.restore_version(&payload.id, &payload.version, &timestamp) {
                return IpcResult::failure(error.to_string());
            }
            match handler_state.command_reload_persona(&context).await {
                Ok(()) => IpcResult::success(()),
                Err(error) => IpcResult::failure(error.format_for_user()),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn persona_reload(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaReload,
        move |context| async move {
            match handler_state.command_reload_persona(&context).await {
                Ok(()) => IpcResult::success(()),
                Err(error) => config_commit_failure(error),
            }
        },
    )
    .await)
}
