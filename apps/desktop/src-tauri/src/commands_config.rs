use crate::command_guard::DesktopCommand;
use crate::commands::{authorize_window, CommandOrigin, IpcError, IpcResult, TauriIpcResult};
use coosenpai_core::config::{
    load_config, parse_config, Config, ConfigError, ConfigValidationIssue, NUMERIC_CONFIG_PATHS,
};
use coosenpai_core::locale::Locale;
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
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    Ok(match load_config(&state.paths) {
        Ok(config) => IpcResult::success(config),
        Err(error) => config_failure_for_locale(error, locale),
    })
}

pub(super) fn validate_numeric_patch(patch: &Value) -> Result<(), ConfigError> {
    let issues = NUMERIC_CONFIG_PATHS
        .iter()
        .filter_map(|path| {
            value_at_path(patch, path)
                .filter(|value| {
                    let nullable = matches!(
                        *path,
                        "companion.dailyProactiveLimit" | "voiceOutput.voicevoxStyleId"
                    );
                    !value.is_number() && !(nullable && value.is_null())
                })
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
    if patch.get("audio").is_some()
        && coosenpai_core::config::audio_config_is_only_difference(current, &next)
    {
        return Ok(DesktopCommand::ConfigAudioUpdate);
    }
    Ok(command_for_config_change(current, &next))
}

fn command_for_config_change(current: &Config, next: &Config) -> DesktopCommand {
    if current.watch != next.watch {
        DesktopCommand::ConfigWatchUpdate
    } else if current.keymap != next.keymap {
        DesktopCommand::ConfigKeymapUpdate
    } else if invalidates_running_operations(current, next) {
        DesktopCommand::ConfigProviderUpdate
    } else if current.work != next.work {
        DesktopCommand::WorkConfigure
    } else {
        DesktopCommand::ConfigDisplayUpdate
    }
}

pub(crate) fn validate_work_config_change(
    current: &Config,
    next: &Config,
    normal_mode: bool,
) -> Result<(), ConfigError> {
    if !normal_mode && current.work != next.work {
        return Err(ConfigError::Validation(vec![ConfigValidationIssue {
            path: "work".into(),
            message:
                "作業の承認方法と許可ルートはチュートリアルとセットアップの終了後に変更できます"
                    .into(),
        }]));
    }
    Ok(())
}

pub(crate) fn invalidates_running_operations(current: &Config, next: &Config) -> bool {
    (current.work != next.work && !work_config_is_only_difference(current, next))
        || watch_runtime_settings_changed(current, next)
        || current.ui.language != next.ui.language
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

pub(super) fn config_failure_for_locale<T: Serialize>(
    error: ConfigError,
    locale: Locale,
) -> IpcResult<T> {
    let issues = match &error {
        ConfigError::Validation(issues) => {
            issues.iter().map(|issue| issue.localized(locale)).collect()
        }
        _ => Vec::new(),
    };
    IpcResult::Failure {
        ok: false,
        error: IpcError {
            message: error.format_for_locale(locale),
            issues,
        },
    }
}

/// 承認方式と許可ルートだけの変更は runtime を再構築せず、進行中の作業を中断しない。
pub(crate) fn work_config_is_only_difference(current: &Config, next: &Config) -> bool {
    let mut comparable = current.clone();
    comparable.work = next.work.clone();
    comparable.revision = next.revision;
    comparable == *next
}
