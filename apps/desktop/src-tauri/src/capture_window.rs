use super::{CancelSource, CaptureEvent, CaptureHandle};
use crate::state::DesktopState;
use std::sync::{Arc, Mutex};
use tauri::Manager;

pub(super) type MouseCallback = Arc<dyn Fn(crate::platform::MouseClick) + Send + Sync>;

#[async_trait::async_trait]
pub(super) trait PopupBoundary: Clone + Send + Sync + 'static {
    type Monitor: Clone + Send + Sync;
    async fn prepare(&self) -> Result<(), String>;
    async fn install_mouse(&self, callback: MouseCallback) -> Result<Self::Monitor, String>;
    async fn remove_mouse(&self, monitor: Self::Monitor) -> Result<(), String>;
    async fn install_escape(&self, generation: u64) -> Result<(), String>;
    async fn remove_escape(&self, generation: u64) -> Result<(), String>;
    async fn hide(&self) -> Result<(), String>;
}

struct Registrations<M> {
    mouse: Option<M>,
    escape: Option<u64>,
}

#[derive(Clone)]
pub(super) struct PopupView<B: PopupBoundary> {
    parent: CaptureHandle,
    boundary: B,
    registrations: Arc<tokio::sync::Mutex<Registrations<B::Monitor>>>,
}

#[derive(Clone)]
pub(crate) struct CapturePopupView {
    inner: PopupView<DesktopPopupBoundary>,
}

impl CapturePopupView {
    pub(crate) fn new(state: Arc<DesktopState>) -> Self {
        Self {
            inner: PopupView::with_boundary(state.capture.clone(), DesktopPopupBoundary { state }),
        }
    }

    pub(crate) async fn prepare_show(&self, generation: u64) -> Result<(), String> {
        self.inner.prepare_show(generation).await
    }

    pub(super) async fn arm(&self, generation: u64) -> Result<(), String> {
        self.inner.arm(generation).await
    }

    pub(super) async fn hide(&self) -> Result<(), String> {
        self.inner.hide().await
    }
}

impl<B: PopupBoundary> PopupView<B> {
    pub(super) fn with_boundary(parent: CaptureHandle, boundary: B) -> Self {
        Self {
            parent,
            boundary,
            registrations: Arc::new(tokio::sync::Mutex::new(Registrations {
                mouse: None,
                escape: None,
            })),
        }
    }

    pub(crate) async fn prepare_show(&self, generation: u64) -> Result<(), String> {
        self.boundary.prepare().await?;
        self.arm(generation).await
    }

    // 非表示失敗後も同じ表示世代の明示取消を受け取れるよう、解除済みの入口だけを戻す。
    pub(super) async fn arm(&self, generation: u64) -> Result<(), String> {
        let mut registrations = self.registrations.lock().await;
        if registrations.escape.is_none() {
            self.boundary.install_escape(generation).await?;
            registrations.escape = Some(generation);
        }
        if registrations.mouse.is_none() {
            let parent = self.parent.clone();
            let callback = Arc::new(move |click| {
                parent.post(CaptureEvent::PopupCancel {
                    generation,
                    source: CancelSource::OutsideClick(click),
                });
            });
            registrations.mouse = Some(self.boundary.install_mouse(callback).await?);
        }
        Ok(())
    }

    pub(super) async fn hide(&self) -> Result<(), String> {
        let mut registrations = self.registrations.lock().await;
        let mut errors = Vec::new();
        if let Some(mouse) = registrations.mouse.clone() {
            match self.boundary.remove_mouse(mouse).await {
                Ok(()) => registrations.mouse = None,
                Err(error) => errors.push(error),
            }
        }
        if let Some(generation) = registrations.escape {
            match self.boundary.remove_escape(generation).await {
                Ok(()) => registrations.escape = None,
                Err(error) => errors.push(error),
            }
        }
        // window取得・main-thread dispatchの失敗でも、上の解除処理は既に試みている。
        if let Err(error) = self.boundary.hide().await {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[derive(Clone)]
pub(super) struct DesktopPopupBoundary {
    state: Arc<DesktopState>,
}

impl DesktopPopupBoundary {
    fn window(&self) -> Result<tauri::WebviewWindow, String> {
        self.state
            .app
            .get_webview_window("capture-popup")
            .ok_or("送信ポップアップのウィンドウがありません".into())
    }
}

#[async_trait::async_trait]
impl PopupBoundary for DesktopPopupBoundary {
    type Monitor = Arc<Mutex<Option<crate::platform::MouseDownMonitor>>>;
    async fn prepare(&self) -> Result<(), String> {
        let window = self.window()?;
        window
            .set_focusable(true)
            .map_err(|error| error.to_string())?;
        crate::windows::position_capture_popup(&window).map_err(|error| error.to_string())?;
        native_action(&window, |native| {
            crate::platform::convert_to_nonactivating_panel(native)
                .map_err(|error| error.to_string())
        })
        .await
    }
    async fn install_mouse(&self, callback: MouseCallback) -> Result<Self::Monitor, String> {
        native_action(&self.window()?, move |native| {
            crate::platform::install_mouse_down_monitor(native, move |click| callback(click))
                .map(|monitor| Arc::new(Mutex::new(Some(monitor))))
                .map_err(|error| error.to_string())
        })
        .await
    }
    async fn remove_mouse(&self, monitor: Self::Monitor) -> Result<(), String> {
        on_main_thread(&self.state.app, move || {
            if let Some(monitor) = monitor.lock().expect("popup monitor").take() {
                monitor.remove().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .await
    }
    async fn install_escape(&self, generation: u64) -> Result<(), String> {
        let state = self.state.clone();
        on_main_thread(&self.state.app, move || {
            state.shortcut_coordinator.install_popup_cancel(
                &super::shortcuts::TauriShortcutRegistrar(&state.app),
                super::shortcuts::PopupCancelTarget { generation },
            )
        })
        .await
    }
    async fn remove_escape(&self, generation: u64) -> Result<(), String> {
        let state = self.state.clone();
        on_main_thread(&self.state.app, move || {
            state.shortcut_coordinator.remove_popup_cancel(
                &super::shortcuts::TauriShortcutRegistrar(&state.app),
                generation,
            )
        })
        .await
    }
    async fn hide(&self) -> Result<(), String> {
        native_action(&self.window()?, |native| {
            crate::platform::hide_window(native).map_err(|error| error.to_string())
        })
        .await
    }
}

pub(crate) async fn on_main_thread<T: Send + 'static>(
    app: &tauri::AppHandle,
    action: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(action());
    })
    .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "ポップアップの監視操作が完了しませんでした".to_owned())?
}

async fn native_action<T: Send + 'static>(
    window: &tauri::WebviewWindow,
    action: impl FnOnce(*mut std::ffi::c_void) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    native_webview_action(window, move |native, _| action(native)).await
}

async fn native_webview_action<T: Send + 'static>(
    window: &tauri::WebviewWindow,
    action: impl FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) -> Result<T, String>
        + Send
        + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            let _ = sender.send(action(webview.ns_window(), webview.inner()));
        })
        .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "ポップアップのウィンドウ操作が完了しませんでした".to_owned())?
}

pub(crate) fn emit_capture_popup_ready<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    generation: u64,
    id: &str,
) -> tauri::Result<()> {
    use tauri::Emitter;
    app.emit_to(
        "capture-popup",
        "coosenpai:capture-popup:ready",
        serde_json::json!({ "generation": generation, "captureId": id }),
    )
}
