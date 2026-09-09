use std::future::Future;
#[cfg(any(target_os = "macos", test))]
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;

const WINDOW_FOCUS_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FocusRequestResult {
    pub(crate) focused: bool,
    pub(crate) self_active_after_request: bool,
    pub(crate) preempted: bool,
}

#[derive(Debug)]
pub(crate) struct FocusRequestError {
    pub(crate) message: String,
}

impl FocusRequestError {
    pub(crate) fn new(message: String) -> Self {
        Self { message }
    }
}

fn popup_focus_requested(popup_focus_requests: &tokio::sync::watch::Receiver<u64>) -> bool {
    matches!(popup_focus_requests.has_changed(), Ok(true))
}

pub(crate) async fn activate_and_focus_window(
    main_window: &tauri::WebviewWindow,
    window: &tauri::WebviewWindow,
    focus_events: tokio::sync::watch::Receiver<bool>,
    popup_focus_requests: Option<tokio::sync::watch::Receiver<u64>>,
) -> Result<FocusRequestResult, FocusRequestError> {
    let main_was_focused = main_window
        .is_focused()
        .map_err(|error| FocusRequestError::new(error.to_string()))?;
    request_native_focus(
        focus_events,
        WINDOW_FOCUS_TIMEOUT,
        main_was_focused,
        popup_focus_requests,
        || activate_and_make_key_on_main_thread(window),
        || order_window_back(main_window),
    )
    .await
}

#[cfg(target_os = "macos")]
async fn activate_and_make_key_on_main_thread(
    window: &tauri::WebviewWindow,
) -> Result<bool, String> {
    // activate と key 化を 1 回のメインスレッド dispatch にまとめ、混雑時の往復待ちを減らす。
    run_native_window_action(window, |native_window| {
        let self_active = crate::platform::activate_current_application_unless_active()
            .map_err(|error| error.to_string())?;
        crate::platform::make_key_and_order_front(native_window)
            .map_err(|error| error.to_string())?;
        Ok(self_active)
    })
    .await
}

pub(crate) async fn present_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    run_native_window_action(window, |native_window| {
        crate::platform::show_window_without_activation(native_window)
            .map_err(|error| error.to_string())
    })
    .await
}

#[cfg(not(target_os = "macos"))]
async fn activate_and_make_key_on_main_thread(
    _window: &tauri::WebviewWindow,
) -> Result<bool, String> {
    Ok(true)
}

async fn request_native_focus<NativeFocus, NativeFocusFuture, OrderMainBack, OrderMainBackFuture>(
    mut focus_events: tokio::sync::watch::Receiver<bool>,
    timeout: Duration,
    main_was_focused: bool,
    popup_focus_requests: Option<tokio::sync::watch::Receiver<u64>>,
    native_focus: NativeFocus,
    order_main_back: OrderMainBack,
) -> Result<FocusRequestResult, FocusRequestError>
where
    NativeFocus: FnOnce() -> NativeFocusFuture,
    NativeFocusFuture: Future<Output = Result<bool, String>>,
    OrderMainBack: FnOnce() -> OrderMainBackFuture,
    OrderMainBackFuture: Future<Output = Result<(), String>>,
{
    let mut popup_focus_requests = popup_focus_requests;
    if popup_focus_requests
        .as_ref()
        .is_some_and(popup_focus_requested)
    {
        return Ok(FocusRequestResult {
            focused: false,
            self_active_after_request: false,
            preempted: true,
        });
    }
    focus_events.mark_unchanged();
    let self_active_after_request = native_focus().await.map_err(FocusRequestError::new)?;
    if !main_was_focused {
        order_main_back().await.map_err(FocusRequestError::new)?;
    }
    let outcome =
        wait_for_focus_event(&mut focus_events, popup_focus_requests.as_mut(), timeout).await;
    Ok(FocusRequestResult {
        focused: matches!(outcome, FocusWaitOutcome::Focused),
        self_active_after_request,
        preempted: matches!(outcome, FocusWaitOutcome::Preempted),
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusWaitOutcome {
    Focused,
    TimedOut,
    Preempted,
}

async fn wait_for_focus_event(
    focus_events: &mut tokio::sync::watch::Receiver<bool>,
    popup_focus_requests: Option<&mut tokio::sync::watch::Receiver<u64>>,
    timeout: Duration,
) -> FocusWaitOutcome {
    if *focus_events.borrow() {
        return FocusWaitOutcome::Focused;
    }
    let preempted = async move {
        match popup_focus_requests {
            Some(receiver) => {
                if receiver.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(preempted);
    let focused = async {
        loop {
            if focus_events.changed().await.is_err() {
                return false;
            }
            if *focus_events.borrow() {
                return true;
            }
        }
    };
    tokio::pin!(focused);
    tokio::select! {
        biased;
        focused = &mut focused => {
            if focused {
                FocusWaitOutcome::Focused
            } else {
                FocusWaitOutcome::TimedOut
            }
        }
        _ = &mut preempted => FocusWaitOutcome::Preempted,
        _ = tokio::time::sleep(timeout) => FocusWaitOutcome::TimedOut,
    }
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn log_focus_failure(
    logger: &dyn coosenpai_core::ports::RuntimeLogger,
    target: &str,
    details: &str,
) {
    let _ = logger.write("WARN", &focus_failure_message(target, details));
}

