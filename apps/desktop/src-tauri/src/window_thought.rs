use tauri::{PhysicalPosition, WebviewWindow};

pub(crate) const THOUGHT_WIDTH_LOGICAL: f64 = 340.0;
const GAP_LOGICAL: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScreenRect {
    pub origin: ScreenPoint,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MainWindowGeometry {
    pub main: ScreenRect,
    pub work_area: ScreenRect,
    pub scale_factor: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThoughtPlacement {
    pub position: ScreenPoint,
    pub bubble_tail: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThoughtWindowNativeState {
    pub visible: bool,
    pub position: ScreenPoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThoughtWindowOperation {
    Position(ThoughtPlacement),
    Show,
    Hide,
}

pub(crate) fn main_geometry<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
) -> Result<MainWindowGeometry, String> {
    let monitor = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or(window
            .primary_monitor()
            .map_err(|error| error.to_string())?)
        .ok_or_else(|| "メインウィンドウの配置先ディスプレイがありません".to_owned())?;
    let main_position = window.outer_position().map_err(|error| error.to_string())?;
    let main_size = window.outer_size().map_err(|error| error.to_string())?;
    let work_area = monitor.work_area();

    Ok(MainWindowGeometry {
        main: ScreenRect {
            origin: ScreenPoint {
                x: main_position.x,
                y: main_position.y,
            },
            width: main_size.width,
            height: main_size.height,
        },
        work_area: ScreenRect {
            origin: ScreenPoint {
                x: work_area.position.x,
                y: work_area.position.y,
            },
            width: work_area.size.width,
            height: work_area.size.height,
        },
        scale_factor: monitor.scale_factor(),
    })
}

pub(crate) fn is_on_active_space<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWindow;

        let native_window = window.ns_window().map_err(|error| error.to_string())?;
        // SAFETY: Tauri returns the live NSWindow associated with this WebviewWindow.
        let native_window: &NSWindow = unsafe { &*native_window.cast() };
        Ok(native_window.isOnActiveSpace())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(true)
    }
}

pub(crate) fn target_placement(geometry: MainWindowGeometry) -> ThoughtPlacement {
    let scale = valid_scale(geometry.scale_factor);
    let thought_width = logical_to_physical(THOUGHT_WIDTH_LOGICAL, scale);
    let gap = logical_to_physical(GAP_LOGICAL, scale);
    let main_right = i64::from(geometry.main.origin.x) + i64::from(geometry.main.width);
    let side_x = main_right + gap;
    let work_area_right =
        i64::from(geometry.work_area.origin.x) + i64::from(geometry.work_area.width);
    let side_fits = side_x + thought_width <= work_area_right;

    let (x, bubble_tail) = if side_fits {
        (side_x, true)
    } else {
        (main_right - thought_width, false)
    };

    ThoughtPlacement {
        position: ScreenPoint {
            x: clamp_coordinate(x),
            y: geometry.main.origin.y,
        },
        bubble_tail,
    }
}

pub(crate) fn set_position(window: &WebviewWindow, target: ScreenPoint) -> tauri::Result<()> {
    let target = PhysicalPosition::new(target.x, target.y);
    if window.outer_position()? != target {
        window.set_position(target)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn attach_to_main_window<R: tauri::Runtime>(
    main: &WebviewWindow<R>,
    thought: &WebviewWindow<R>,
) -> Result<(), String> {
    let main_thread = objc2::MainThreadMarker::new().is_some();
    let main = main.clone();
    let thought = thought.clone();
    let dispatcher = main.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let task = move || {
        let result = attach_to_main_window_on_main_thread(&main, &thought);
        let _ = sender.send(result);
    };

    if main_thread {
        task();
    } else {
        dispatcher
            .run_on_main_thread(task)
            .map_err(|error| error.to_string())?;
    }
    receiver
        .recv()
        .map_err(|error| format!("子ウィンドウ化の結果を受け取れませんでした: {error}"))?
}

#[cfg(target_os = "macos")]
fn attach_to_main_window_on_main_thread<R: tauri::Runtime>(
    main: &WebviewWindow<R>,
    thought: &WebviewWindow<R>,
) -> Result<(), String> {
    let _main_thread = objc2::MainThreadMarker::new()
        .ok_or_else(|| "子ウィンドウ化がメインスレッド外で呼ばれました".to_owned())?;
    use objc2_app_kit::{NSWindow, NSWindowOrderingMode};

    let main_window = main.ns_window().map_err(|error| error.to_string())?;
    let thought_window = thought.ns_window().map_err(|error| error.to_string())?;
    // SAFETY: Tauri returns the live NSWindow objects associated with these distinct windows.
    let main_window: &NSWindow = unsafe { &*main_window.cast() };
    // SAFETY: Tauri returns the live NSWindow objects associated with these distinct windows.
    let thought_window: &NSWindow = unsafe { &*thought_window.cast() };

    thought_window.setLevel(main_window.level());
    if thought_window
        .parentWindow()
        .is_some_and(|parent| std::ptr::eq(parent.as_ref(), main_window))
    {
        return Ok(());
    }

    if let Some(parent) = thought_window.parentWindow() {
        parent.removeChildWindow(thought_window);
    }
    // SAFETY: the live main and thought windows are distinct, so this cannot create a parent cycle.
    unsafe {
        main_window.addChildWindow_ordered(thought_window, NSWindowOrderingMode::Above);
    }
    if !thought_window
        .parentWindow()
        .is_some_and(|parent| std::ptr::eq(parent.as_ref(), main_window))
    {
        return Err("思考吹き出しがメインウィンドウの子になっていません".to_owned());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn attach_to_main_window<R: tauri::Runtime>(
    _main: &WebviewWindow<R>,
    _thought: &WebviewWindow<R>,
) -> Result<(), String> {
    Ok(())
}

fn valid_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn logical_to_physical(value: f64, scale: f64) -> i64 {
    (value * scale).round() as i64
}

fn clamp_coordinate(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
