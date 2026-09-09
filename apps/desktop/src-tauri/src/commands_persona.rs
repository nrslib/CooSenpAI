use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, config_commit_failure_for_locale, dispatch_result, validate_id_for_locale,
    CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use coosenpai_core::locale::{localize_error_message, Locale, TextKey};
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
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.id, locale)?;
    let store = match PersonaStore::from_paths(&state.paths) {
        Ok(store) => store,
        Err(error) => {
            return Ok(IpcResult::failure(localize_error_message(
                &error.to_string(),
                TextKey::PersonaOperationFailed,
                locale,
            )))
        }
    };
    let entries = match store.list() {
        Ok(entries) => entries,
        Err(error) => {
            return Ok(IpcResult::failure(localize_error_message(
                &error.to_string(),
                TextKey::PersonaOperationFailed,
                locale,
            )))
        }
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
                (false, Err(error)) => {
                    return Ok(IpcResult::failure(localize_error_message(
                        &error.to_string(),
                        TextKey::PersonaOperationFailed,
                        locale,
                    )))
                }
            },
        }),
        Err(error) => IpcResult::failure(localize_error_message(
            &error.to_string(),
            TextKey::PersonaOperationFailed,
            locale,
        )),
    })
}

#[tauri::command]
pub async fn persona_save(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaSavePayload,
) -> TauriIpcResult<coosenpai_core::config::Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.id, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    let ui = state.ui.clone();
    let result = dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaSave,
        move |context| async move {
            let store = match PersonaStore::from_paths(&handler_state.paths) {
                Ok(store) => store,
                Err(error) => {
                    return IpcResult::failure(localize_error_message(
                        &error.to_string(),
                        TextKey::PersonaOperationFailed,
                        locale,
                    ))
                }
            };
            let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
            if let Err(error) = store.save_custom(
                &payload.id,
                &payload.display_name,
                &payload.body,
                &timestamp,
            ) {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::PersonaOperationFailed,
                    locale,
                ));
            }
            match handler_state
                .command_switch_persona(&context, payload.id)
                .await
            {
                Ok(config) => IpcResult::success(config),
                Err(error) => IpcResult::failure(error.format_for_locale(Locale::from_config(
                    &handler_state.runtime_config().ui.language,
                ))),
            }
        },
    )
    .await;
    ui.input(
        crate::ui_events::UiView::Chat,
        crate::ui_events::UiEvent::App(crate::app_presenter::AppEvent::PersonasChanged),
    );
    Ok(result)
}

#[tauri::command]
pub async fn persona_delete(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: PersonaGetPayload,
) -> TauriIpcResult<coosenpai_core::config::Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.id, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    let ui = state.ui.clone();
    let result = dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaDelete,
        move |context| async move {
            let store = match PersonaStore::from_paths(&handler_state.paths) {
                Ok(store) => store,
                Err(error) => {
                    return IpcResult::failure(localize_error_message(
                        &error.to_string(),
                        TextKey::PersonaOperationFailed,
                        locale,
                    ))
                }
            };
            let selected = handler_state.runtime_config().companion.persona;
            let config = if persona_fallback_after_delete(&selected, &payload.id).is_some() {
                match handler_state
                    .command_switch_persona(&context, "coo-chan".to_owned())
                    .await
                {
                    Ok(config) => config,
                    Err(error) => {
                        return IpcResult::failure(error.format_for_locale(Locale::from_config(
                            &handler_state.runtime_config().ui.language,
                        )))
                    }
                }
            } else {
                handler_state.runtime_config()
            };
            if let Err(error) = store.delete_custom(&payload.id) {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::PersonaOperationFailed,
                    locale,
                ));
            }
            IpcResult::success(config)
        },
    )
    .await;
    ui.input(
        crate::ui_events::UiView::Chat,
        crate::ui_events::UiEvent::App(crate::app_presenter::AppEvent::PersonasChanged),
    );
    Ok(result)
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
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.id, locale)?;
    validate_id_for_locale(&payload.version, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaRestore,
        move |context| async move {
            let store = match PersonaStore::from_paths(&handler_state.paths) {
                Ok(store) => store,
                Err(error) => {
                    return IpcResult::failure(localize_error_message(
                        &error.to_string(),
                        TextKey::PersonaOperationFailed,
                        locale,
                    ))
                }
            };
            let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
            if let Err(error) = store.restore_version(&payload.id, &payload.version, &timestamp) {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::PersonaOperationFailed,
                    locale,
                ));
            }
            match handler_state.command_reload_persona(&context).await {
                Ok(()) => IpcResult::success(()),
                Err(error) => IpcResult::failure(error.format_for_locale(Locale::from_config(
                    &handler_state.runtime_config().ui.language,
                ))),
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
    let ui = state.ui.clone();
    let result = dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::PersonaReload,
        move |context| async move {
            match handler_state.command_reload_persona(&context).await {
                Ok(()) => IpcResult::success(()),
                Err(error) => config_commit_failure_for_locale(
                    error,
                    Locale::from_config(&handler_state.runtime_config().ui.language),
                ),
            }
        },
    )
    .await;
    ui.input(
        crate::ui_events::UiView::Chat,
        crate::ui_events::UiEvent::App(crate::app_presenter::AppEvent::PersonasChanged),
    );
    Ok(result)
}
