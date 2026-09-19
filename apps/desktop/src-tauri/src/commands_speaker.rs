use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::HelperResolverPort;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerManagementPayload {
    operation: String,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    speaker_id: Option<String>,
}

fn valid_speaker_id(value: &str) -> bool {
    let Some(number) = value.strip_prefix("speaker-") else {
        return false;
    };
    !number.is_empty() && number.parse::<u64>().is_ok_and(|number| number > 0)
}

fn management_arguments(
    payload: SpeakerManagementPayload,
    locale: Locale,
) -> Result<Vec<String>, String> {
    let invalid = || text(TextKey::CommandInvalidInput, locale).to_owned();
    let mut arguments = vec!["--speaker-management".to_owned()];
    match payload.operation.as_str() {
        "merge" => {
            let (Some(source_id), Some(target_id)) = (payload.source_id, payload.target_id) else {
                return Err(invalid());
            };
            if !valid_speaker_id(&source_id)
                || !valid_speaker_id(&target_id)
                || payload.speaker_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend([
                "merge".to_owned(),
                "--speaker-from".to_owned(),
                source_id,
                "--speaker-to".to_owned(),
                target_id,
            ]);
        }
        "undoMerge" => {
            let Some(source_id) = payload.source_id else {
                return Err(invalid());
            };
            if !valid_speaker_id(&source_id)
                || payload.target_id.is_some()
                || payload.speaker_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend([
                "undo-merge".to_owned(),
                "--speaker-from".to_owned(),
                source_id,
            ]);
        }
        "reregister" => {
            let Some(speaker_id) = payload.speaker_id else {
                return Err(invalid());
            };
            if !valid_speaker_id(&speaker_id)
                || payload.source_id.is_some()
                || payload.target_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend([
                "reregister".to_owned(),
                "--speaker-id".to_owned(),
                speaker_id,
            ]);
        }
        "delete" => {
            let Some(speaker_id) = payload.speaker_id else {
                return Err(invalid());
            };
            if !valid_speaker_id(&speaker_id)
                || payload.source_id.is_some()
                || payload.target_id.is_some()
            {
                return Err(invalid());
            }
            arguments.extend(["delete".to_owned(), "--speaker-id".to_owned(), speaker_id]);
        }
        "deleteAll" => {
            if payload.source_id.is_some()
                || payload.target_id.is_some()
                || payload.speaker_id.is_some()
            {
                return Err(invalid());
            }
            arguments.push("delete-all".to_owned());
        }
        _ => return Err(invalid()),
    }
    Ok(arguments)
}

fn hearing_helper(state: &DesktopState) -> Option<std::path::PathBuf> {
    let executable_directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_owned))?;
    crate::platform::MacHelperResolver
        .resolve_hearing_helper(&executable_directory, &state.paths.root)
}

#[tauri::command]
pub async fn speaker_management(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: SpeakerManagementPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let arguments = match management_arguments(payload, locale) {
        Ok(arguments) => arguments,
        Err(message) => return Ok(IpcResult::failure(message)),
    };
    let Some(helper) = hearing_helper(&state) else {
        return Ok(IpcResult::failure(
            "話者管理 helper が見つかりません".to_owned(),
        ));
    };

    state.cancel_audio_and_wait().await;
    let ledger = state.paths.speakers.join("registry.enc");
    let output = match tokio::process::Command::new(helper)
        .args(arguments)
        .arg("--speaker-ledger")
        .arg(ledger)
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) => {
            state.sync_audio();
            return Ok(IpcResult::failure(format!(
                "話者管理 helper を起動できません: {error}"
            )));
        }
    };
    let result = if output.status.success() {
        IpcResult::success(())
    } else {
        let message = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                (!stderr.is_empty()).then_some(stderr)
            })
            .unwrap_or_else(|| "話者管理 helper が失敗しました".to_owned());
        IpcResult::failure(message)
    };
    state.sync_audio();
    Ok(result)
}

