use coosenpai_core::ports::ScreenPoint;
use tauri::{LogicalPosition, LogicalSize};

pub(crate) fn position(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    update_layout(window, 1, "bottom-right", "main")
}

pub(crate) fn update_layout(
    window: &tauri::WebviewWindow,
    record_count: usize,
    position: &str,
    display: &str,
) -> tauri::Result<()> {
    resize(
        window,
        104_u32.saturating_mul(u32::try_from(record_count.max(1)).unwrap_or(3)),
        position,
        display,
    )
}

pub(crate) fn resize(
    window: &tauri::WebviewWindow,
    height: u32,
    position: &str,
    display: &str,
) -> tauri::Result<()> {
    let primary = window.primary_monitor()?;
    let monitors = window.available_monitors()?;
    let display_port = crate::platform::bubble_display();
    let target = match display {
        "cursor" => display_port.cursor_point().ok(),
        "front" => display_port.frontmost_window_point().ok().flatten(),
        _ => None,
    };
    let areas = monitors.iter().map(monitor_area).collect::<Vec<_>>();
    let Some(area) = resolve_monitor_area(primary.as_ref().map(monitor_area), &areas, target)
    else {
        return Ok(());
    };
    let width = 390.0_f64;
    let height = f64::from(height);
    let origin = bubble_origin(area, width, height, position);
    let scale = window.scale_factor()?;
    let current_size = window.outer_size()?.to_logical::<f64>(scale);
    let current_origin = window.outer_position()?.to_logical::<f64>(scale);
    if geometry_differs(current_size.width, width) || geometry_differs(current_size.height, height)
    {
        window.set_size(LogicalSize::new(width, height))?;
    }
    if geometry_differs(current_origin.x, origin.x) || geometry_differs(current_origin.y, origin.y)
    {
        window.set_position(LogicalPosition::new(origin.x, origin.y))?;
    }
    Ok(())
}

fn geometry_differs(current: f64, target: f64) -> bool {
    (current - target).abs() >= 0.5
}

fn resolve_monitor_area(
    primary: Option<MonitorArea>,
    monitors: &[MonitorArea],
    target: Option<ScreenPoint>,
) -> Option<MonitorArea> {
    target
        .and_then(|point| monitors.iter().copied().find(|area| area.contains(point)))
        .or(primary)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MonitorArea {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl MonitorArea {
    fn contains(self, point: ScreenPoint) -> bool {
        point.x >= self.x
            && point.x < self.x + self.width
            && point.y >= self.y
            && point.y < self.y + self.height
    }
}

fn monitor_area(monitor: &tauri::Monitor) -> MonitorArea {
    let scale = monitor.scale_factor();
    let size = monitor.size().to_logical::<f64>(scale);
    let position = monitor.position().to_logical::<f64>(scale);
    MonitorArea {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    }
}

fn bubble_origin(monitor: MonitorArea, width: f64, height: f64, position: &str) -> ScreenPoint {
    let left = position.ends_with("left");
    let bottom = position.starts_with("bottom");
    ScreenPoint {
        x: if left {
            monitor.x + 16.0
        } else {
            monitor.x + (monitor.width - width - 16.0).max(0.0)
        },
        y: if bottom {
            monitor.y + (monitor.height - height - 16.0).max(0.0)
        } else {
            monitor.y + 16.0
        },
    }
}

