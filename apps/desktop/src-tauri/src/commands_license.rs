use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use serde::Deserialize;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseDocumentPayload {
    document: String,
}

fn license_document_file_name(document: &str) -> Option<&'static str> {
    match document {
        "license" => Some("LICENSE"),
        "eula" => Some("EULA.md"),
        "eula-en" => Some("EULA.en.md"),
        _ => None,
    }
}

#[tauri::command]
pub async fn license_document_open(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    payload: LicenseDocumentPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let Some(file_name) = license_document_file_name(&payload.document) else {
        return Ok(IpcResult::failure(text(
            TextKey::CommandInvalidInput,
            locale,
        )));
    };
    let state = state.inner().clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::LicenseDocumentOpen,
        move |_context| async move {
            let path = match app.path().resource_dir() {
                Ok(directory) => directory.join(file_name),
                Err(_) => {
                    return IpcResult::failure(text(TextKey::LicenseDocumentOpenFailed, locale))
                }
            };
            if !path.is_file() {
                return IpcResult::failure(text(TextKey::LicenseDocumentOpenFailed, locale));
            }
            match crate::platform::open_file(&path).await {
                Ok(()) => IpcResult::success(()),
                Err(_) => IpcResult::failure(text(TextKey::LicenseDocumentOpenFailed, locale)),
            }
        },
    )
    .await)
}

