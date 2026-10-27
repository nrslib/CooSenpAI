use coosenpai_core::ports::ScreenPoint;
use tauri::{LogicalPosition, LogicalSize, Manager};

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
    resize_with_port(
        window,
        height,
        position,
        display,
        crate::platform::bubble_display(),
    )
}

fn resize_with_port<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    height: u32,
    position: &str,
    display: &str,
    display_port: std::sync::Arc<dyn coosenpai_core::ports::BubbleDisplayPort>,
) -> tauri::Result<()> {
    let window = window.clone();
    let dispatcher = window.app_handle().clone();
    let position = position.to_owned();
    let display = display.to_owned();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let task = move || {
        let result = resize_on_main(&window, height, &position, &display, display_port);
        let _ = sender.send(result);
    };
    if objc2::MainThreadMarker::new().is_some() {
        task();
    } else {
        dispatcher.run_on_main_thread(task)?;
    }
    receiver.recv().map_err(|error| {
        tauri::Error::Anyhow(anyhow::anyhow!(
            "吹き出しの配置結果を受け取れませんでした: {error}"
        ))
    })?
}

fn resize_on_main<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    height: u32,
    position: &str,
    display: &str,
    display_port: std::sync::Arc<dyn coosenpai_core::ports::BubbleDisplayPort>,
) -> tauri::Result<()> {
    let target = resolve_display_target(display, None, display_port.as_ref());
    let Some(area) = resolve_display_area(&target, display_port.as_ref())? else {
        return Ok(());
    };
    let width = bubble_width(area.width, position);
    let height = bubble_height(area.height, f64::from(height));
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

pub(crate) async fn cursor_at_bubble_edge(
    app: tauri::AppHandle,
    position: String,
    display: String,
) -> anyhow::Result<BubbleEdgeCheck> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(cursor_at_bubble_edge_on_main(&position, &display));
    })?;
    receiver
        .await
        .map_err(|_| anyhow::anyhow!("画面端再表示の画面解決に失敗しました"))?
}

fn cursor_at_bubble_edge_on_main(position: &str, display: &str) -> anyhow::Result<BubbleEdgeCheck> {
    let display_port = crate::platform::bubble_display();
    let cursor = display_port
        .cursor_point()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let resolved = resolve_edge_display_area(display, cursor, display_port.as_ref())?;
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
    uuid: Option<String>,
    warning: Option<anyhow::Error>,
}

fn resolve_display_area(
    target: &DisplayTarget,
    display_port: &dyn coosenpai_core::ports::BubbleDisplayPort,
) -> anyhow::Result<Option<MonitorArea>> {
    let displays = display_port
        .displays()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(resolve_monitor_area(&displays, target))
}

fn resolve_edge_display_area(
    display: &str,
    cursor: ScreenPoint,
    display_port: &dyn coosenpai_core::ports::BubbleDisplayPort,
) -> anyhow::Result<ResolvedEdgeDisplay> {
    let target = resolve_display_target(display, Some(cursor), display_port);
    let area = resolve_display_area(&target, display_port)?
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

fn resolve_display_target(
    display: &str,
    cursor: Option<ScreenPoint>,
    display_port: &dyn coosenpai_core::ports::BubbleDisplayPort,
) -> DisplayTarget {
    match display {
        "cursor" => DisplayTarget {
            point: cursor.or_else(|| display_port.cursor_point().ok()),
            uuid: None,
            warning: None,
        },
        "front" => match display_port.frontmost_window_point() {
            Ok(point) => DisplayTarget {
                point,
                uuid: None,
                warning: None,
            },
            Err(error) => DisplayTarget {
                point: None,
                uuid: None,
                warning: Some(anyhow::anyhow!(error.to_string())),
            },
        },
        _ => DisplayTarget {
            point: None,
            uuid: coosenpai_core::config::bubble_display_uuid(display).map(str::to_owned),
            warning: None,
        },
    }
}

fn geometry_differs(current: f64, target: f64) -> bool {
    (current - target).abs() >= 0.5
}

fn resolve_monitor_area(
    displays: &[coosenpai_core::ports::BubbleDisplay],
    target: &DisplayTarget,
) -> Option<MonitorArea> {
    target
        .uuid
        .as_ref()
        .and_then(|uuid| {
            displays
                .iter()
                .find(|display| display.uuid.eq_ignore_ascii_case(uuid))
        })
        .or_else(|| {
            target.point.and_then(|point| {
                displays
                    .iter()
                    .find(|display| MonitorArea::from(display.bounds).contains(point))
            })
        })
        .or_else(|| displays.iter().find(|display| display.is_main))
        .map(|display| MonitorArea::from(display.bounds))
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

impl From<coosenpai_core::ports::WindowBounds> for MonitorArea {
    fn from(bounds: coosenpai_core::ports::WindowBounds) -> Self {
        Self {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        }
    }
}

fn bubble_width(display_width: f64, position: &str) -> f64 {
    let margin = 16.0_f64.min(display_width / 4.0);
    let content_width: f64 = if position.ends_with("left") {
        392.0
    } else {
        374.0
    };
    content_width.min(display_width - margin * 2.0)
}

fn bubble_height(display_height: f64, requested_height: f64) -> f64 {
    let margin = 16.0_f64.min(display_height / 4.0);
    requested_height.min(display_height - margin * 2.0)
}

fn bubble_origin(monitor: MonitorArea, width: f64, height: f64, position: &str) -> ScreenPoint {
    let left = position.ends_with("left");
    let bottom = position.starts_with("bottom");
    let horizontal_margin = 16.0_f64.min((monitor.width - width) / 2.0);
    ScreenPoint {
        x: if left {
            monitor.x + horizontal_margin
        } else {
            monitor.x + monitor.width - width - horizontal_margin
        },
        y: if bottom {
            monitor.y + monitor.height - height - 16.0_f64.min((monitor.height - height) / 2.0)
        } else {
            monitor.y + 16.0_f64.min((monitor.height - height) / 2.0)
        },
    }
}
