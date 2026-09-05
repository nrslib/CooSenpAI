use crate::command_guard::DesktopCommand;
use crate::commands::{authorize_window, CommandOrigin, IpcError, IpcResult, TauriIpcResult};
use coosenpai_core::config::{
    load_config, parse_config, Config, ConfigError, ConfigValidationIssue, NUMERIC_CONFIG_PATHS,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[tauri::command]
pub(super) async fn config_get_persisted(
    window: WebviewWindow,
    state: State<'_, Arc<crate::state::DesktopState>>,
) -> TauriIpcResult<Config> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(match load_config(&state.paths) {
        Ok(config) => IpcResult::success(config),
        Err(error) => config_failure(error),
    })
}

pub(super) fn validate_numeric_patch(patch: &Value) -> Result<(), ConfigError> {
    let issues = NUMERIC_CONFIG_PATHS
        .iter()
        .filter_map(|path| {
            value_at_path(patch, path)
                .filter(|value| !value.is_number())
                .map(|_| ConfigValidationIssue {
                    path: (*path).to_owned(),
                    message: "数値で指定してください。".to_owned(),
                })
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(issues))
    }
}

pub(super) fn validate_config_patch(patch: &Value) -> Result<(), ConfigError> {
    validate_numeric_patch(patch)
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

pub(super) fn deep_merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                if let Some(current) = target.get_mut(&key) {
                    deep_merge(current, value);
                } else {
                    target.insert(key, value);
                }
            }
        }
        (target, patch) => *target = patch,
    }
}

pub(super) fn apply_config_patch(
    current: Config,
    patch: Value,
    signed_build: bool,
) -> Result<Config, ConfigError> {
    validate_config_patch(&patch)?;
    let audio_was_enabled = current.audio.enabled;
    let mut merged = serde_json::to_value(current)?;
    deep_merge(&mut merged, patch);
    let mut config = parse_config(merged)?;
    coosenpai_core::config::normalize_audio_sources_on_enable(audio_was_enabled, &mut config);
    if !signed_build && config.notification.mode != "bubble" {
        return Err(ConfigError::Validation(vec![ConfigValidationIssue {
            path: "notification.mode".to_owned(),
            message: "OS 通知は署名済みビルドでのみ選択できます。".to_owned(),
        }]));
    }
    Ok(config)
}

pub(crate) fn command_for_config_patch(
    current: &Config,
    patch: &Value,
    signed_build: bool,
) -> Result<DesktopCommand, ConfigError> {
    let next = apply_config_patch(current.clone(), patch.clone(), signed_build)?;
    Ok(command_for_config_change(current, &next))
}

fn command_for_config_change(current: &Config, next: &Config) -> DesktopCommand {
    if current.watch != next.watch {
        DesktopCommand::ConfigWatchUpdate
    } else if current.keymap != next.keymap {
        DesktopCommand::ConfigKeymapUpdate
    } else if invalidates_running_operations(current, next) {
        DesktopCommand::ConfigProviderUpdate
    } else {
        DesktopCommand::ConfigDisplayUpdate
    }
}

pub(crate) fn invalidates_running_operations(current: &Config, next: &Config) -> bool {
    watch_runtime_settings_changed(current, next)
        || current.observer.provider != next.observer.provider
        || current.observer.model != next.observer.model
        || current.observer.effort != next.observer.effort
        || current.observer.executable != next.observer.executable
        || current.companion.provider != next.companion.provider
        || current.companion.model != next.companion.model
        || current.companion.effort != next.companion.effort
        || current.companion.executable != next.companion.executable
        || current.memory.enabled != next.memory.enabled
        || current.memory.provider_consent != next.memory.provider_consent
}

fn watch_runtime_settings_changed(current: &Config, next: &Config) -> bool {
    let mut current_watch = current.watch.clone();
    let mut next_watch = next.watch.clone();
    current_watch.enabled = false;
    next_watch.enabled = false;
    current_watch != next_watch
}

pub(super) fn config_failure<T: Serialize>(error: ConfigError) -> IpcResult<T> {
    let issues = match &error {
        ConfigError::Validation(issues) => issues.clone(),
        _ => Vec::new(),
    };
    IpcResult::Failure {
        ok: false,
        error: IpcError {
            message: error.format_for_user(),
            issues,
        },
    }
}

