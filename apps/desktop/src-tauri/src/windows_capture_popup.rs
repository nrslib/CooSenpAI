use coosenpai_core::ports::{ScreenPoint, WindowBounds};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWindow};
use tauri::LogicalPosition;

const SELECTION_POPUP_GAP: f64 = 8.0;

struct DisplayArea {
    frame: WindowBounds,
    visible_frame: WindowBounds,
}

pub(super) fn position_near_selection(
    window: &tauri::WebviewWindow,
    selection: WindowBounds,
) -> tauri::Result<Option<LogicalPosition<f64>>> {
    let main_thread = MainThreadMarker::new().ok_or_else(|| {
        tauri::Error::Io(std::io::Error::other(
            "送信ポップアップの配置にはメインスレッドが必要です",
        ))
    })?;
    let screens = NSScreen::screens(main_thread);
    let Some(primary) = screens.firstObject() else {
        return Ok(None);
    };
    let main_screen_height = primary.frame().size.height;
    let displays = screens
        .iter()
        .map(|screen| DisplayArea {
            frame: appkit_to_top_left(rect(screen.frame()), main_screen_height),
            visible_frame: appkit_to_top_left(rect(screen.visibleFrame()), main_screen_height),
        })
        .collect::<Vec<_>>();
    let native_window = window.ns_window()?;
    // SAFETY: Tauri returns the live NSWindow associated with this WebviewWindow.
    let native_window: &NSWindow = unsafe { &*native_window.cast() };
    let popup_size = native_window.frame().size;
    Ok(
        popup_position(selection, popup_size.width, popup_size.height, &displays)
            .map(|position| LogicalPosition::new(position.x, position.y)),
    )
}

fn rect(rect: objc2_foundation::NSRect) -> WindowBounds {
    WindowBounds {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

fn appkit_to_top_left(bounds: WindowBounds, main_screen_height: f64) -> WindowBounds {
    WindowBounds {
        y: main_screen_height - bounds.y - bounds.height,
        ..bounds
    }
}

fn popup_position(
    selection: WindowBounds,
    popup_width: f64,
    popup_height: f64,
    displays: &[DisplayArea],
) -> Option<ScreenPoint> {
    if !valid_rect(selection)
        || !popup_width.is_finite()
        || !popup_height.is_finite()
        || popup_width <= 0.0
        || popup_height <= 0.0
    {
        return None;
    }
    let mut selected_display = None;
    let mut greatest_overlap = 0.0;
    for display in displays
        .iter()
        .filter(|display| valid_rect(display.frame) && valid_rect(display.visible_frame))
    {
        let overlap = intersection_area(selection, display.frame);
        if overlap > greatest_overlap {
            selected_display = Some(display);
            greatest_overlap = overlap;
        }
    }
    let area = selected_display?.visible_frame;
    let below_y = selection.y + selection.height + SELECTION_POPUP_GAP;
    let above_y = selection.y - popup_height - SELECTION_POPUP_GAP;
    let desired_y = if below_y + popup_height <= area.y + area.height {
        below_y
    } else {
        above_y
    };
    Some(ScreenPoint {
        x: clamp_origin(selection.x, area.x, popup_width, area.width),
        y: clamp_origin(desired_y, area.y, popup_height, area.height),
    })
}

fn valid_rect(rect: WindowBounds) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width > 0.0
        && rect.height > 0.0
}

fn intersection_area(left: WindowBounds, right: WindowBounds) -> f64 {
    let width = (left.x + left.width).min(right.x + right.width) - left.x.max(right.x);
    let height = (left.y + left.height).min(right.y + right.height) - left.y.max(right.y);
    width.max(0.0) * height.max(0.0)
}

fn clamp_origin(value: f64, area_origin: f64, popup_length: f64, area_length: f64) -> f64 {
    value
        .max(area_origin)
        .min((area_origin + area_length - popup_length).max(area_origin))
}
