use crate::state::DesktopState;
use std::future::Future;
#[cfg(any(target_os = "macos", test))]
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::Manager;

const WINDOW_FOCUS_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CapturePopupPresentation {
    pub show: Duration,
    pub focus: Duration,
    pub focused: bool,
    pub self_active_after_request: bool,
}

impl CapturePopupPresentation {
    pub(crate) fn focus_result(self) -> FocusRequestResult {
        FocusRequestResult {
            focused: self.focused,
            self_active_after_request: self.self_active_after_request,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FocusRequestResult {
    pub(crate) focused: bool,
    pub(crate) self_active_after_request: bool,
}

#[derive(Debug)]
pub(crate) struct FocusRequestError {
    pub(crate) message: String,
    window_show_requested: bool,
}

impl FocusRequestError {
    pub(crate) fn before_window_show_request(message: String) -> Self {
        Self {
            message,
            window_show_requested: false,
        }
    }

    pub(crate) fn after_window_show_request(message: String) -> Self {
        Self {
            message,
            window_show_requested: true,
        }
    }

    pub(crate) fn window_show_requested(&self) -> bool {
        self.window_show_requested
    }
}

pub(crate) async fn focus_bubble_if_capture_popup_idle<
    CanFocus,
    CanFocusFuture,
    SetFocusable,
    Focus,
    FocusFuture,
>(
    focus_gate: &tokio::sync::Mutex<()>,
    mut can_focus: CanFocus,
    set_focusable: SetFocusable,
    focus: Focus,
) -> Result<Option<FocusRequestResult>, String>
where
    CanFocus: FnMut() -> CanFocusFuture,
    CanFocusFuture: Future<Output = bool>,
    SetFocusable: FnOnce() -> Result<(), String>,
    Focus: FnOnce() -> FocusFuture,
    FocusFuture: Future<Output = Result<FocusRequestResult, String>>,
{
    let _focus_guard = focus_gate.lock().await;
    if can_focus().await {
        return Ok(None);
    }
    set_focusable()?;
    if can_focus().await {
        return Ok(None);
    }
    Ok(Some(focus().await?))
}

pub(crate) async fn show_capture_popup(
    state: &DesktopState,
    window: &tauri::WebviewWindow,
) -> Result<CapturePopupPresentation, FocusRequestError> {
    let _focus_guard = state.popup_focus_gate().lock().await;
    orchestrate_capture_popup_presentation(
        || super::position_capture_popup(window).map_err(|error| error.to_string()),
        || window.show().map_err(|error| error.to_string()),
        || async {
            focus_capture_popup_unlocked(state, window)
                .await
                .map_err(|error| error.message)
        },
    )
    .await
}

pub(crate) async fn orchestrate_capture_popup_presentation<Position, Show, Focus, FocusFuture>(
    position: Position,
    show: Show,
    focus: Focus,
) -> Result<CapturePopupPresentation, FocusRequestError>
where
    Position: FnOnce() -> Result<(), String>,
    Show: FnOnce() -> Result<(), String>,
    Focus: FnOnce() -> FocusFuture,
    FocusFuture: Future<Output = Result<FocusRequestResult, String>>,
{
    position().map_err(FocusRequestError::before_window_show_request)?;
    let show_started = std::time::Instant::now();
    show().map_err(FocusRequestError::before_window_show_request)?;
    let show = show_started.elapsed();
    let focus_started = std::time::Instant::now();
    let focus_result = focus()
        .await
        .map_err(FocusRequestError::after_window_show_request)?;
    Ok(CapturePopupPresentation {
        show,
        focus: focus_started.elapsed(),
        focused: focus_result.focused,
        self_active_after_request: focus_result.self_active_after_request,
    })
}

pub(crate) async fn focus_capture_popup(
    state: &DesktopState,
    window: &tauri::WebviewWindow,
) -> Result<FocusRequestResult, FocusRequestError> {
    let _focus_guard = state.popup_focus_gate().lock().await;
    focus_capture_popup_unlocked(state, window).await
}

async fn focus_capture_popup_unlocked(
    state: &DesktopState,
    window: &tauri::WebviewWindow,
) -> Result<FocusRequestResult, FocusRequestError> {
    let main = state.app.get_webview_window("main").ok_or_else(|| {
        FocusRequestError::before_window_show_request("メインウィンドウがありません".to_owned())
    })?;
    let focus_events = state.capture_popup_focus_events();
    window
        .set_focusable(true)
        .map_err(|error| FocusRequestError::before_window_show_request(error.to_string()))?;
    activate_and_focus_window(&main, window, focus_events).await
}

pub(crate) async fn activate_and_focus_window(
    main_window: &tauri::WebviewWindow,
    window: &tauri::WebviewWindow,
    focus_events: tokio::sync::watch::Receiver<bool>,
) -> Result<FocusRequestResult, FocusRequestError> {
    let main_was_focused = main_window
        .is_focused()
        .map_err(|error| FocusRequestError::before_window_show_request(error.to_string()))?;
    request_native_focus(
        focus_events,
        WINDOW_FOCUS_TIMEOUT,
        main_was_focused,
        || activate_current_application_on_main_thread(window),
        || make_window_key_and_order_front(window),
        || order_window_back(main_window),
    )
    .await
}

#[cfg(target_os = "macos")]
async fn activate_current_application_on_main_thread(
    window: &tauri::WebviewWindow,
) -> Result<bool, String> {
    run_native_window_action(window, |_| {
        crate::platform::activate_current_application().map_err(|error| error.to_string())
    })
    .await
}

#[cfg(not(target_os = "macos"))]
async fn activate_current_application_on_main_thread(
    _window: &tauri::WebviewWindow,
) -> Result<bool, String> {
    Ok(true)
}

async fn request_native_focus<
    Activate,
    ActivateFuture,
    MakeKeyAndOrderFront,
    MakeKeyAndOrderFrontFuture,
    OrderMainBack,
    OrderMainBackFuture,
>(
    mut focus_events: tokio::sync::watch::Receiver<bool>,
    timeout: Duration,
    main_was_focused: bool,
    activate: Activate,
    make_key_and_order_front: MakeKeyAndOrderFront,
    order_main_back: OrderMainBack,
) -> Result<FocusRequestResult, FocusRequestError>
where
    Activate: FnOnce() -> ActivateFuture,
    ActivateFuture: Future<Output = Result<bool, String>>,
    MakeKeyAndOrderFront: FnOnce() -> MakeKeyAndOrderFrontFuture,
    MakeKeyAndOrderFrontFuture: Future<Output = Result<(), String>>,
    OrderMainBack: FnOnce() -> OrderMainBackFuture,
    OrderMainBackFuture: Future<Output = Result<(), String>>,
{
    let self_active_after_request = activate()
        .await
        .map_err(FocusRequestError::before_window_show_request)?;
    focus_events.mark_unchanged();
    make_key_and_order_front()
        .await
        .map_err(FocusRequestError::before_window_show_request)?;
    if !main_was_focused {
        order_main_back()
            .await
            .map_err(FocusRequestError::after_window_show_request)?;
    }
    Ok(FocusRequestResult {
        focused: wait_for_focus_event(&mut focus_events, timeout).await,
        self_active_after_request,
    })
}

#[cfg(target_os = "macos")]
async fn make_window_key_and_order_front(window: &tauri::WebviewWindow) -> Result<(), String> {
    run_native_window_action(window, |native_window| {
        crate::platform::make_key_and_order_front(native_window).map_err(|error| error.to_string())
    })
    .await
}

#[cfg(not(target_os = "macos"))]
async fn make_window_key_and_order_front(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
async fn order_window_back(window: &tauri::WebviewWindow) -> Result<(), String> {
    run_native_window_action(window, |native_window| {
        crate::platform::order_window_back(native_window).map_err(|error| error.to_string())
    })
    .await
}

#[cfg(not(target_os = "macos"))]
async fn order_window_back(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
type DeferredNativeWindowAction = Box<dyn FnOnce(*mut std::ffi::c_void) + Send>;

#[cfg(any(target_os = "macos", test))]
const NATIVE_WINDOW_ACTION_PENDING: u8 = 0;
#[cfg(any(target_os = "macos", test))]
const NATIVE_WINDOW_ACTION_RUNNING: u8 = 1;
#[cfg(any(target_os = "macos", test))]
const NATIVE_WINDOW_ACTION_CANCELLED: u8 = 2;

#[cfg(target_os = "macos")]
async fn run_native_window_action<T, Action>(
    window: &tauri::WebviewWindow,
    action: Action,
) -> Result<T, String>
where
    T: Send + 'static,
    Action: FnOnce(*mut std::ffi::c_void) -> Result<T, String> + Send + 'static,
{
    run_dispatched_native_window_action(
        WINDOW_FOCUS_TIMEOUT,
        |deferred_action| {
            window
                .with_webview(move |webview| deferred_action(webview.ns_window()))
                .map_err(|error| error.to_string())
        },
        action,
    )
    .await
}

#[cfg(any(target_os = "macos", test))]
async fn run_dispatched_native_window_action<T, Dispatch, Action>(
    timeout: Duration,
    dispatch: Dispatch,
    action: Action,
) -> Result<T, String>
where
    T: Send + 'static,
    Dispatch: FnOnce(DeferredNativeWindowAction) -> Result<(), String> + Send,
    Action: FnOnce(*mut std::ffi::c_void) -> Result<T, String> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<T, String>>();
    let action_state = Arc::new(AtomicU8::new(NATIVE_WINDOW_ACTION_PENDING));
    let callback_state = action_state.clone();
    let deferred_action = Box::new(move |native_window| {
        if callback_state
            .compare_exchange(
                NATIVE_WINDOW_ACTION_PENDING,
                NATIVE_WINDOW_ACTION_RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            let result = action(native_window);
            let _ = sender.send(result);
        }
    });
    if let Err(error) = dispatch(deferred_action) {
        let _ = action_state.compare_exchange(
            NATIVE_WINDOW_ACTION_PENDING,
            NATIVE_WINDOW_ACTION_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        return Err(error);
    }
    let mut receiver = receiver;
    match tokio::time::timeout(timeout, &mut receiver).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("ネイティブウィンドウ操作が完了しませんでした".to_owned()),
        Err(_) => {
            if action_state
                .compare_exchange(
                    NATIVE_WINDOW_ACTION_PENDING,
                    NATIVE_WINDOW_ACTION_CANCELLED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Err("ネイティブウィンドウ操作がタイムアウトしました".to_owned());
            }
            match receiver.await {
                Ok(result) => result,
                Err(_) => Err("ネイティブウィンドウ操作が完了しませんでした".to_owned()),
            }
        }
    }
}

async fn wait_for_focus_event(
    focus_events: &mut tokio::sync::watch::Receiver<bool>,
    timeout: Duration,
) -> bool {
    if *focus_events.borrow() {
        return true;
    }
    tokio::time::timeout(timeout, async {
        loop {
            if focus_events.changed().await.is_err() {
                return false;
            }
            if *focus_events.borrow() {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false)
}

pub(crate) fn focus_failure_details(
    window: &tauri::WebviewWindow,
    result: FocusRequestResult,
) -> String {
    let frontmost = crate::platform::frontmost_application()
        .map(|application| {
            format!(
                "name={} bundle-id={}",
                application.name, application.bundle_id
            )
        })
        .unwrap_or_else(|| "none".to_owned());
    let key_window = match window.is_focused() {
        Ok(value) => value.to_string(),
        Err(error) => format!("error:{error}"),
    };
    format_focus_failure_details(result, &frontmost, &key_window)
}

fn format_focus_failure_details(
    result: FocusRequestResult,
    frontmost: &str,
    key_window: &str,
) -> String {
    format!(
        "self-active-after-request={} frontmost-app={frontmost} key-window={key_window}",
        result.self_active_after_request
    )
}

fn focus_failure_message(target: &str, details: &str) -> String {
    format!(
        "{target}のキーフォーカス要求に失敗しました: focus-event-timeout-ms={} {details}",
        WINDOW_FOCUS_TIMEOUT.as_millis()
    )
}

pub(crate) fn log_focus_failure(
    logger: &dyn coosenpai_core::ports::RuntimeLogger,
    target: &str,
    details: &str,
) {
    let _ = logger.write("WARN", &focus_failure_message(target, details));
}

