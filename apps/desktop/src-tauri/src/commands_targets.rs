use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, config_commit_failure_for_locale, dispatch_result, validate_id_for_locale,
    CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use coosenpai_core::config::{Config, WatchAppConfig};
use coosenpai_core::locale::{localize_error_message, text, Locale, TextKey};
use coosenpai_core::ports::RunningApplication;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchTargetPayload {
    bundle_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchTargetEnabledPayload {
    bundle_id: String,
    enabled: bool,
}

#[tauri::command]
pub async fn running_apps_list(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<Vec<RunningApplication>> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    Ok(match crate::platform::MacApplicationCapture::running() {
        Ok(applications) => IpcResult::success(applications),
        Err(error) => IpcResult::failure(localize_error_message(
            &error.to_string(),
            TextKey::RunningApplicationsListFailed,
            locale,
        )),
    })
}

#[tauri::command]
pub async fn watch_target_add(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: WatchTargetPayload,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id_for_locale(
        &payload.bundle_id,
        Locale::from_config(&state.runtime_config().ui.language),
    )?;
    let selected = crate::platform::MacApplicationCapture::running()
        .ok()
        .and_then(|applications| {
            applications
                .into_iter()
                .find(|application| application.bundle_id == payload.bundle_id)
        });
    let Some(selected) = selected else {
        return Ok(IpcResult::failure(text(
            TextKey::WatchTargetMissing,
            Locale::from_config(&state.runtime_config().ui.language),
        )));
    };
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::WatchTargetUpdate,
        move |context| async move {
            match handler_state
                .command_update_config_with(&context, move |mut config| {
                    if let Some(existing) = config
                        .watch
                        .apps
                        .iter_mut()
                        .find(|application| application.bundle_id == selected.bundle_id)
                    {
                        existing.name = selected.name;
                        existing.enabled = true;
                    } else {
                        config.watch.apps.push(WatchAppConfig {
                            bundle_id: selected.bundle_id,
                            name: selected.name,
                            enabled: true,
                        });
                    }
                    Ok(config)
                })
                .await
            {
                Ok(outcome) => IpcResult::success_with_issues(outcome.config, outcome.issues),
                Err(error) => config_commit_failure_for_locale(
                    error,
                    Locale::from_config(&handler_state.runtime_config().ui.language),
                ),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn watch_target_remove(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: WatchTargetPayload,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id_for_locale(
        &payload.bundle_id,
        Locale::from_config(&state.runtime_config().ui.language),
    )?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::WatchTargetUpdate,
        move |context| async move {
            match handler_state
                .command_update_config_with(&context, move |mut config| {
                    config
                        .watch
                        .apps
                        .retain(|application| application.bundle_id != payload.bundle_id);
                    Ok(config)
                })
                .await
            {
                Ok(outcome) => IpcResult::success_with_issues(outcome.config, outcome.issues),
                Err(error) => config_commit_failure_for_locale(
                    error,
                    Locale::from_config(&handler_state.runtime_config().ui.language),
                ),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn watch_target_set_enabled(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: WatchTargetEnabledPayload,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    validate_id_for_locale(
        &payload.bundle_id,
        Locale::from_config(&state.runtime_config().ui.language),
    )?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::WatchTargetUpdate,
        move |context| async move {
            match handler_state
                .command_update_config_with(&context, move |mut config| {
                    let Some(application) = config
                        .watch
                        .apps
                        .iter_mut()
                        .find(|application| application.bundle_id == payload.bundle_id)
                    else {
                        return Err(coosenpai_core::config::ConfigError::Validation(vec![
                            coosenpai_core::config::ConfigValidationIssue {
                                path: "watch.apps".to_owned(),
                                message: text(
                                    TextKey::WatchTargetMissing,
                                    Locale::from_config(&config.ui.language),
                                )
                                .to_owned(),
                            },
                        ]));
                    };
                    application.enabled = payload.enabled;
                    Ok(config)
                })
                .await
            {
                Ok(outcome) => IpcResult::success_with_issues(outcome.config, outcome.issues),
                Err(error) => config_commit_failure_for_locale(
                    error,
                    Locale::from_config(&handler_state.runtime_config().ui.language),
                ),
            }
        },
    )
    .await)
}

