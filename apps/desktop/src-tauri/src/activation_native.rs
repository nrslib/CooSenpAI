use super::runtime::{ActivationLease, ActivationPort};
use super::OriginObservation;
use crate::capture::window::{on_main_thread, CapturePopupView};
use crate::capture::{CaptureOrigin, ReadyCapture};
use crate::state::DesktopState;
use coosenpai_core::ports::{ForegroundApplication, ForegroundApplicationPort, RuntimeLogger};
use std::sync::{Arc, Mutex};
use tauri::Manager;

pub(super) struct NativeActivationPort {
    state: Arc<DesktopState>,
    popup: CapturePopupView,
    _applications: crate::platform::ApplicationActivationMonitor,
    key_monitor: Arc<Mutex<Option<crate::platform::WindowKeyMonitor>>>,
    activation_changes: tokio::sync::watch::Receiver<()>,
}

impl NativeActivationPort {
    pub(super) fn new(state: Arc<DesktopState>, popup: CapturePopupView) -> Self {
        let ui = state.ui.clone();
        let other_ui = ui.clone();
        let (activation_changed, activation_changes) = tokio::sync::watch::channel(());
        let other_activated = activation_changed.clone();
        Self {
            _applications: crate::platform::ApplicationActivationMonitor::new(
                move || {
                    activation_changed.send_replace(());
                    ui.input(
                        crate::ui_events::UiView::Application,
                        crate::ui_events::UiEvent::ApplicationDeactivated,
                    );
                },
                move || {
                    other_activated.send_replace(());
                    other_ui.input(
                        crate::ui_events::UiView::Application,
                        crate::ui_events::UiEvent::OtherApplicationActivated,
                    );
                },
            ),
            state,
            popup,
            key_monitor: Arc::new(Mutex::new(None)),
            activation_changes,
        }
    }

    async fn make_popup_key(&self, lease: ActivationLease, generation: u64) -> Result<(), String> {
        let window = self
            .state
            .app
            .get_webview_window("capture-popup")
            .ok_or("送信ポップアップのウィンドウがありません")?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let monitor = self.key_monitor.clone();
        let ui = self.state.ui.clone();
        window
            .with_webview(move |webview| {
                let result = lease.execute(|| {
                    let subscription =
                        crate::platform::WindowKeyMonitor::new(webview.ns_window(), move || {
                            ui.input(
                                crate::ui_events::UiView::Application,
                                crate::ui_events::UiEvent::Activation(
                                    super::ActivationInput::PopupKeyLost(generation),
                                ),
                            );
                        })
                        .map_err(|error| error.to_string())?;
                    *monitor.lock().expect("popup key monitor") = Some(subscription);
                    crate::platform::show_nonactivating_webview_panel(
                        webview.ns_window(),
                        webview.inner(),
                    )
                    .map_err(|error| error.to_string())
                });
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        receiver
            .await
            .map_err(|_| "ポップアップの key 操作が完了しませんでした".to_owned())?
    }

    fn focus_popup(&self) -> Result<(), String> {
        crate::webview_event::emit_to(
            &self.state.app,
            "capture-popup",
            "coosenpai:capture-popup:focus",
            &(),
        )
        .map_err(|error| error.to_string())
    }
}

#[async_trait::async_trait]
impl ActivationPort for NativeActivationPort {
    fn log(&self, message: &str) {
        let _ = self.state.logger.write("INFO", message);
    }

    fn log_error(&self, message: &str) {
        let _ = self.state.logger.write("ERROR", message);
    }

    async fn observe_origin(
        &self,
        lease: ActivationLease,
        main_origin: Option<ForegroundApplication>,
    ) -> Result<OriginObservation, String> {
        on_main_thread(&self.state.app, move || {
            lease.execute(|| {
                Ok(OriginObservation {
                    self_active: crate::platform::current_application_is_active()
                        .map_err(|error| error.to_string())?,
                    self_pid: std::process::id() as i32,
                    frontmost: crate::platform::frontmost_application_identity()
                        .filter(crate::platform::application_is_running),
                    main_origin: main_origin.filter(crate::platform::application_is_running),
                })
            })
        })
        .await
    }

    async fn return_to_origin(
        &self,
        lease: ActivationLease,
        origin: ForegroundApplication,
        expires_at: Option<tokio::time::Instant>,
    ) -> Result<(), String> {
        let mut changes = self.activation_changes.clone();
        let target = origin.clone();
        let activation = lease.clone();
        let returned = on_main_thread(&self.state.app, move || {
            activation.execute_before(
                expires_at,
                "撮影前のアクティブ化が2秒以内に完了しませんでした",
                || {
                    if !crate::platform::application_is_running(&target) {
                        return Ok(false);
                    }
                    crate::platform::return_activation_to_application(&target)
                        .map_err(|error| error.to_string())?;
                    Ok(true)
                },
            )
        })
        .await?;
        if !returned {
            return Ok(());
        }
        // activate の受付だけでキーを送ると、遅れて完了する切替が popup の key を奪う。
        super::runtime::confirm_origin_returned(&mut changes, || {
            let target_pid = origin.process_id;
            let observation = lease.clone();
            let origin = origin.clone();
            on_main_thread(&self.state.app, move || {
                observation.execute(|| {
                    let active = crate::platform::current_application_is_active()
                        .map_err(|error| error.to_string())?;
                    Ok(!crate::platform::application_is_running(&origin)
                        || (!active
                            && crate::platform::frontmost_application_identity()
                                .is_some_and(|front| front.process_id == target_pid)))
                })
            })
        })
        .await
    }

    async fn show_popup(
        &self,
        lease: ActivationLease,
        generation: u64,
        content: Arc<ReadyCapture>,
    ) -> Result<(), String> {
        self.popup.prepare_show(generation).await?;
        self.make_popup_key(lease, generation).await?;
        crate::capture::window::emit_capture_popup_ready(&self.state.app, generation, &content.id)
            .map_err(|error| error.to_string())?;
        self.focus_popup()?;
        let _ = self.state.logger.write(
            "INFO",
            &format!("capture: popup shown generation={generation} key=true webview-focus=true"),
        );
        Ok(())
    }

    async fn reacquire_popup_key(
        &self,
        lease: ActivationLease,
        _generation: u64,
        expires_at: Option<tokio::time::Instant>,
    ) -> Result<bool, String> {
        let window = self
            .state
            .app
            .get_webview_window("capture-popup")
            .ok_or("送信ポップアップのウィンドウがありません")?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        window
            .with_webview(move |webview| {
                let result = lease.execute_key_before(expires_at, || {
                    crate::platform::reacquire_nonactivating_webview_panel(
                        webview.ns_window(),
                        webview.inner(),
                    )
                    .map_err(|error| error.to_string())
                });
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        let key = receiver
            .await
            .map_err(|_| "ポップアップの key 操作が完了しませんでした".to_owned())??;
        if key {
            self.focus_popup()?;
        }
        Ok(key)
    }

    async fn restore_origin(
        &self,
        lease: ActivationLease,
        origin: CaptureOrigin,
    ) -> Result<(), String> {
        let Some(application) = origin.frontmost_application else {
            return Ok(());
        };
        on_main_thread(&self.state.app, move || {
            lease.execute(|| {
                if !crate::platform::application_is_running(&application) {
                    return Ok(());
                }
                crate::platform::MacForegroundApplications
                    .activate_application(&application)
                    .map_err(|error| error.to_string())
            })
        })
        .await
    }
}
