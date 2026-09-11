use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult};
use crate::snapshot::AppSnapshot;
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

#[tauri::command]
pub async fn details_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<AppSnapshot> {
    authorize_window(&window, CommandOrigin::Details)?;
    Ok(IpcResult::success(state.snapshot().await))
}

const DATAFLOW_LOG_LIMIT: usize = 200;

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

#[test]
fn dataflow_open_path_requires_an_existing_file_under_the_allowed_directories() {
    let directory = tempfile::tempdir().unwrap();
    let paths = coosenpai_core::config::ConfigPaths::from_root(directory.path().join("coo"));
    std::fs::create_dir_all(&paths.frame_buffer).unwrap();
    let frame = paths.frame_buffer.join("frame.png");
    let outside = directory.path().join("outside.txt");
    std::fs::write(&frame, b"frame").unwrap();
    std::fs::write(&outside, b"outside").unwrap();
    assert!(allowed_dataflow_path(&paths, &frame));
    assert!(!allowed_dataflow_path(&paths, &outside));
    assert!(!allowed_dataflow_path(&paths, &paths.frame_buffer));
    assert!(!allowed_dataflow_path(
        &paths,
        &paths.frame_buffer.join("missing.png")
    ));
    #[cfg(unix)]
    {
        let link = paths.frame_buffer.join("outside.png");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(!allowed_dataflow_path(&paths, &link));
    }
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
