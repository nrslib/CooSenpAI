use crate::avatar_presenter::AvatarState;
use coosenpai_core::locale::{text, Locale, TextKey};
use tauri::{Emitter, Manager, WebviewWindow};

pub(crate) fn render<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    value: &AvatarState,
) -> Result<(), String> {
    render_to(value, |label, value| {
        app.get_webview_window(label)
            .ok_or_else(|| format!("WebView がありません: {label}"))?;
        app.emit_to(label, "coosenpai:avatar:changed", value)
            .map_err(|error| error.to_string())
    })
}

// 両方が必須。片方の失敗で他方への配送を省略しない。
pub(crate) fn render_to(
    value: &AvatarState,
    mut emit: impl FnMut(&str, &AvatarState) -> Result<(), String>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for label in ["main", "avatar"] {
        if let Err(error) = emit(label, value) {
            errors.push(format!("{label}: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "アバターの投影を配送できません: {}",
            errors.join("; ")
        ))
    }
}

pub(crate) fn visibility(
    app: &tauri::AppHandle,
    visible: bool,
    position: bool,
    language: &str,
) -> Result<(), String> {
    let locale = Locale::from_config(language);
    let window = app
        .get_webview_window("avatar")
        .ok_or_else(|| text(TextKey::AvatarWindowMissing, locale).to_owned())?;
    if position {
        position_initial(&window).map_err(|error| {
            text(TextKey::AvatarPositionFailed, locale).replace("{error}", &error.to_string())
        })?;
    }
    if visible {
        window.show()
    } else {
        window.hide()
    }
    .map_err(|error| {
        text(TextKey::AvatarVisibilityFailed, locale).replace("{error}", &error.to_string())
    })
}

fn position_initial(window: &WebviewWindow) -> tauri::Result<()> {
    let monitor = window
        .current_monitor()?
        .or(window.primary_monitor()?)
        .ok_or(tauri::Error::WindowNotFound)?;
    let area = monitor.work_area();
    let size = window.outer_size()?;
    let margin = (20.0 * monitor.scale_factor()).round() as u32;
    window.set_position(tauri::PhysicalPosition::new(
        trailing_position(area.position.x, area.size.width, size.width, margin),
        trailing_position(area.position.y, area.size.height, size.height, margin),
    ))
}

pub(crate) fn trailing_position(origin: i32, available: u32, size: u32, margin: u32) -> i32 {
    let offset = available.saturating_sub(size.saturating_add(margin));
    (i64::from(origin) + i64::from(offset)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

