use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use serde::Deserialize;
use std::path::Path;
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

const DATAFLOW_LOG_LIMIT: usize = 200;
const CONVERSATION_LOG_DAYS: i64 = 7;
const CONVERSATION_LOG_LIMIT: usize = 500;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationLogPayload {
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationLogDeletePayload {
    date: String,
}

fn conversation_log_date(
    payload: &ConversationLogPayload,
    locale: Locale,
) -> Result<Option<chrono::NaiveDate>, String> {
    let Some(raw) = payload.date.as_deref() else {
        return Ok(None);
    };
    parse_conversation_log_date(raw, locale).map(Some)
}

fn parse_conversation_log_date(raw: &str, locale: Locale) -> Result<chrono::NaiveDate, String> {
    let invalid = || text(TextKey::CommandInvalidInput, locale).to_owned();
    let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| invalid())?;
    let window = coosenpai_core::conversation_log::conversation_log_window(
        chrono::Local::now().date_naive(),
        CONVERSATION_LOG_DAYS,
    );
    if !window.contains(&date) {
        return Err(invalid());
    }
    Ok(date)
}

fn conversation_log_delete_date(
    payload: &ConversationLogDeletePayload,
    locale: Locale,
) -> Result<chrono::NaiveDate, String> {
    parse_conversation_log_date(&payload.date, locale)
}

#[tauri::command]
pub async fn details_conversation_log(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: ConversationLogPayload,
) -> TauriIpcResult<coosenpai_core::conversation_log::ConversationLog> {
    authorize_window(&window, CommandOrigin::Details)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let date = match conversation_log_date(&payload, locale) {
        Ok(date) => date,
        Err(message) => return Ok(IpcResult::failure(message)),
    };
    Ok(
        match coosenpai_core::conversation_log::read_conversation_log(
            &state.paths,
            date,
            CONVERSATION_LOG_DAYS,
            CONVERSATION_LOG_LIMIT,
        ) {
            Ok(log) => IpcResult::success(log),
            Err(error) => IpcResult::failure(
                text(TextKey::DetailsConversationLogReadFailed, locale)
                    .replace("{error}", &error.to_string()),
            ),
        },
    )
}

#[tauri::command]
pub async fn details_delete_conversation_log(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: ConversationLogDeletePayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Details)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let date = match conversation_log_delete_date(&payload, locale) {
        Ok(date) => date,
        Err(message) => return Ok(IpcResult::failure(message)),
    };
    let result = state
        .delete_conversation_log_day(state.paths.clone(), date)
        .await;
    Ok(match result {
        Ok(()) => IpcResult::success(()),
        Err(error) => IpcResult::failure(
            text(TextKey::DetailsConversationLogDeleteFailed, locale)
                .replace("{error}", &error.to_string()),
        ),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataflowPathPayload {
    path: String,
}

fn allowed_dataflow_path(paths: &coosenpai_core::config::ConfigPaths, requested: &Path) -> bool {
    let Ok(requested) = std::fs::canonicalize(requested) else {
        return false;
    };
    if !requested.is_file() {
        return false;
    }
    [paths.frame_buffer.as_path(), paths.transcripts.as_path()]
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .any(|root| requested.starts_with(root))
}

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

#[tauri::command]
pub async fn details_dataflow_open_path(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: DataflowPathPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Details)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let path = std::path::PathBuf::from(payload.path);
    if !allowed_dataflow_path(&state.paths, &path) {
        return Ok(IpcResult::failure(text(
            TextKey::CommandInvalidInput,
            locale,
        )));
    }
    Ok(match crate::platform::open_file(&path).await {
        Ok(()) => IpcResult::success(()),
        Err(_) => IpcResult::failure(text(TextKey::DetailsDataflowPathOpenFailed, locale)),
    })
}
