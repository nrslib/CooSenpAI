use coosenpai_core::ports::ScreenPoint;
use tauri::{LogicalPosition, LogicalSize};

pub(crate) fn position(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    update_layout(window, "bottom-right", "main")
}

pub(crate) fn update_layout(
    window: &tauri::WebviewWindow,
    position: &str,
    display: &str,
) -> tauri::Result<()> {
    resize(window, 104, position, display)
}

pub(crate) fn resize(
    window: &tauri::WebviewWindow,
    height: u32,
    position: &str,
    display: &str,
) -> tauri::Result<()> {
    let Some(area) = resolve_display_area(window, display, None)? else {
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

pub(crate) fn cursor_at_bubble_edge(
    window: &tauri::WebviewWindow,
    position: &str,
    display: &str,
) -> anyhow::Result<BubbleEdgeCheck> {
    let cursor = crate::platform::bubble_display()
        .cursor_point()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let resolved = resolve_edge_display_area(window, display, cursor)?;
    Ok(BubbleEdgeCheck {
        at_edge: crate::bubble_edge_recall::is_at_bubble_edge(
            cursor,
            crate::bubble_edge_recall::BubbleDisplayArea {
                x: resolved.area.x,
                y: resolved.area.y,
                width: resolved.area.width,
                height: resolved.area.height,
            },
            position,
        ),
        warning: resolved.warning,
    })
}

pub(crate) struct BubbleEdgeCheck {
    pub(crate) at_edge: bool,
    pub(crate) warning: Option<anyhow::Error>,
}

struct DisplayTarget {
    point: Option<ScreenPoint>,
    warning: Option<anyhow::Error>,
}

fn resolve_display_area(
    window: &tauri::WebviewWindow,
    display: &str,
    cursor: Option<ScreenPoint>,
) -> tauri::Result<Option<MonitorArea>> {
    let target = resolve_display_target(display, cursor);
    resolve_monitor_area_for_window(window, target.point)
}

fn resolve_edge_display_area(
    window: &tauri::WebviewWindow,
    display: &str,
    cursor: ScreenPoint,
) -> anyhow::Result<ResolvedEdgeDisplay> {
    let target = resolve_display_target(display, Some(cursor));
    let area = resolve_monitor_area_for_window(window, target.point)
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| anyhow::anyhow!("吹き出しの表示画面を解決できません"))?;
    Ok(ResolvedEdgeDisplay {
        area,
        warning: target.warning,
    })
}

struct ResolvedEdgeDisplay {
    area: MonitorArea,
    warning: Option<anyhow::Error>,
}

fn resolve_display_target(display: &str, cursor: Option<ScreenPoint>) -> DisplayTarget {
    let display_port = crate::platform::bubble_display();
    match display {
        "cursor" => DisplayTarget {
            point: cursor.or_else(|| display_port.cursor_point().ok()),
            warning: None,
        },
        "front" => match display_port.frontmost_window_point() {
            Ok(point) => DisplayTarget {
                point,
                warning: None,
            },
            Err(error) => DisplayTarget {
                point: None,
                warning: Some(anyhow::anyhow!(error.to_string())),
            },
        },
        _ => DisplayTarget {
            point: None,
            warning: None,
        },
    }
}

fn resolve_monitor_area_for_window(
    window: &tauri::WebviewWindow,
    target: Option<ScreenPoint>,
) -> tauri::Result<Option<MonitorArea>> {
    let primary = window.primary_monitor()?;
    let monitors = window.available_monitors()?;
    let areas = monitors.iter().map(monitor_area).collect::<Vec<_>>();
    Ok(resolve_monitor_area(
        primary.as_ref().map(monitor_area),
        &areas,
        target,
    ))
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

