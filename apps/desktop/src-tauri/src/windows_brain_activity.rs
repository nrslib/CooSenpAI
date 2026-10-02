use crate::state::DesktopState;
use crate::ui_events::{UiEvent, UiView};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow, WebviewWindowBuilder, WindowEvent};

pub(crate) fn ensure_brain_activity_window<R: Runtime>(
    app: &AppHandle<R>,
) -> tauri::Result<WebviewWindow<R>> {
    if let Some(window) = app.get_webview_window("brain-activity") {
        return Ok(window);
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == "brain-activity")
        .ok_or(tauri::Error::WindowNotFound)?;
    let window = WebviewWindowBuilder::from_config(app, config)?.build()?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Some(state) = handle.try_state::<Arc<DesktopState>>() {
                state.ui.input(UiView::BrainActivity, UiEvent::Close);
            }
        }
    });
    Ok(window)
}

pub(crate) fn show_brain_activity<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let window = ensure_brain_activity_window(app)?;
    window.show()?;
    window.set_focus()
}
