use crate::command_guard::{CommandSource, DesktopCommand, DispatchError};
use crate::state::DesktopState;
use coosenpai_core::onboarding::OnboardingStore;
use coosenpai_core::persistence::atomic_write_bytes;
use coosenpai_core::ports::RuntimeLogger;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, Submenu};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::webview::{PageLoadEvent, WebviewWindowBuilder};
use tauri::{App, AppHandle, LogicalPosition, Manager, Runtime, WindowEvent};

struct TrayControls {
    start: MenuItem<tauri::Wry>,
    stop: MenuItem<tauri::Wry>,
    reset_conversation: MenuItem<tauri::Wry>,
    shortcut_items: Vec<MenuItem<tauri::Wry>>,
    setup_required: std::sync::atomic::AtomicBool,
    running: std::sync::atomic::AtomicBool,
    recording: std::sync::atomic::AtomicBool,
}

pub fn configure(app: &mut App) -> tauri::Result<()> {
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let bubble = create_bubble_window(app)?;
    let bubble_focus_state = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| state.inner().clone());
    bubble.on_window_event(move |event| {
        if let WindowEvent::Focused(focused) = event {
            if let Some(state) = bubble_focus_state.as_ref() {
                state.set_bubble_focused(*focused);
            }
        }
    });
    bubble.set_focusable(true)?;
    bubble.set_visible_on_all_workspaces(true)?;
    configure_full_screen_space_behavior(&bubble)?;
    bubble.set_ignore_cursor_events(true)?;
    crate::window_bubble::position(&bubble)?;
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let placement = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| state.paths.state.join("main-window.json"));
    if let Some(path) = placement.as_ref() {
        restore_main_window(&main, path);
    }
    let placement_revision = Arc::new(AtomicU64::new(0));
    main.on_window_event({
        let app = app.handle().clone();
        move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide_main(&app);
            }
            WindowEvent::Focused(true) => {
                if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                    state
                        .main_window_visible
                        .store(true, std::sync::atomic::Ordering::Release);
                    state
                        .main_window_focused
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                        crate::bubbles::clear_for_main_window(state.inner().as_ref()).await;
                    }
                });
            }
            WindowEvent::Focused(false) => {
                if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                    state
                        .main_window_focused
                        .store(false, std::sync::atomic::Ordering::Release);
                }
            }
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                if let Some(path) = placement.as_ref() {
                    schedule_main_window_save(
                        app.clone(),
                        path.clone(),
                        placement_revision.clone(),
                    );
                }
            }
            _ => {}
        }
    });
    let capture = app
        .get_webview_window("capture-popup")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    configure_full_screen_space_behavior(&capture)?;
    position_capture_popup(&capture)?;
    capture.on_window_event({
        let state = app
            .try_state::<Arc<DesktopState>>()
            .map(|state| state.inner().clone());
        move |event| match event {
            WindowEvent::Focused(focused) => {
                if let Some(state) = state.as_ref() {
                    state.set_capture_popup_focused(*focused);
                }
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if let Some(state) = state.clone() {
                    tauri::async_runtime::spawn(async move {
                        let handler_state = state.clone();
                        let _ = state
                            .dispatch(
                                CommandSource::IpcCapturePopup,
                                DesktopCommand::CaptureCancel,
                                move |context| async move {
                                    crate::capture::cancel(&handler_state, &context).await;
                                    Ok(())
                                },
                            )
                            .await;
                    });
                }
            }
            _ => {}
        }
    });
    let speech = app
        .get_webview_window("speech-popup")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    configure_full_screen_space_behavior(&speech)?;
    position_speech_popup(&speech)?;
    speech.on_window_event({
        let state = app
            .try_state::<Arc<DesktopState>>()
            .map(|state| state.inner().clone());
        move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Some(state) = state.clone() {
                    tauri::async_runtime::spawn(async move {
                        let handler_state = state.clone();
                        let _ = state
                            .dispatch(
                                CommandSource::IpcSpeechPopup,
                                DesktopCommand::SpeechCancel,
                                move |context| async move {
                                    handler_state
                                        .command_speech_cancel(&context)
                                        .map_err(DispatchError::handler)
                                },
                            )
                            .await;
                    });
                }
            }
        }
    });
    let model = app
        .get_webview_window("model-popup")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    configure_full_screen_space_behavior(&model)?;
    position_model_popup(&model, &main)?;
    let model_for_close = model.clone();
    model.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = model_for_close.hide();
        }
    });
    create_tray(app)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn configure_full_screen_space_behavior(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    let native_window = window.ns_window()?;
    // SAFETY: Tauri returns the live NSWindow associated with this WebviewWindow.
    let native_window: &NSWindow = unsafe { &*native_window.cast() };
    let collection_behavior = native_window.collectionBehavior()
        | NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    native_window.setCollectionBehavior(collection_behavior);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn configure_full_screen_space_behavior(_window: &tauri::WebviewWindow) -> tauri::Result<()> {
    Ok(())
}

fn create_bubble_window(app: &App) -> tauri::Result<tauri::WebviewWindow> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == "bubble")
        .cloned()
        .ok_or(tauri::Error::WindowNotFound)?;
    WebviewWindowBuilder::from_config(app.handle(), &config)?
        .on_page_load(|window, payload| {
            let phase = match payload.event() {
                PageLoadEvent::Started => "開始",
                PageLoadEvent::Finished => "完了",
            };
            if let Some(state) = window.app_handle().try_state::<Arc<DesktopState>>() {
                let _ = state.logger.write(
                    "INFO",
                    &format!(
                        "吹き出しrendererのページ読み込みを{phase}しました: url={}",
                        payload.url()
                    ),
                );
            }
        })
        .build()
}

pub fn show_main(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = show_main_now(&app).await;
    });
}

pub(crate) fn main_is_visible(app: &AppHandle) -> bool {
    app.try_state::<Arc<DesktopState>>().is_some_and(|state| {
        state
            .main_window_visible
            .load(std::sync::atomic::Ordering::Acquire)
    })
}

pub(crate) fn main_is_focused(app: &AppHandle) -> bool {
    app.try_state::<Arc<DesktopState>>().is_some_and(|state| {
        state
            .main_window_focused
            .load(std::sync::atomic::Ordering::Acquire)
    })
}

pub(crate) fn hide_main(app: &AppHandle) {
    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
        state
            .main_window_visible
            .store(false, std::sync::atomic::Ordering::Release);
        state
            .main_window_focused
            .store(false, std::sync::atomic::Ordering::Release);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    if let Some(window) = app.get_webview_window("model-popup") {
        let _ = window.hide();
    }
}

pub(crate) async fn show_main_now(app: &AppHandle) -> Result<(), String> {
    let Some(state) = app.try_state::<Arc<DesktopState>>() else {
        return Err("desktop state がありません".to_owned());
    };
    let state = state.inner().clone();
    let onboarding_phase = state.onboarding_policy_phase().await;
    if main_open_action(onboarding_phase) == MainOpenAction::SetupPrompt {
        hide_main(&state.app);
        let handler_state = state.clone();
        state
            .dispatch(
                CommandSource::TutorialAutomation,
                DesktopCommand::SetupPrompt,
                move |context| async move {
                    handler_state
                        .command_announce_initial_onboarding(&context)
                        .await
                        .map_err(DispatchError::handler)
                },
            )
            .await
            .map_err(|error| error.format_for_user())?;
        return Ok(());
    }
    let window = state
        .app
        .get_webview_window("main")
        .ok_or_else(|| "メインウィンドウがありません".to_owned())?;
    window
        .show()
        .map_err(|error| format!("メインウィンドウを表示できません: {error}"))?;
    state
        .main_window_visible
        .store(true, std::sync::atomic::Ordering::Release);
    window
        .set_focus()
        .map_err(|error| format!("メインウィンドウを前面にできません: {error}"))?;
    state
        .main_window_focused
        .store(true, std::sync::atomic::Ordering::Release);
    crate::bubbles::clear_for_main_window(state.as_ref()).await;
    if should_notify_tutorial_main_opened(onboarding_phase) {
        let handler_state = state.clone();
        let _ = state
            .dispatch(
                CommandSource::TutorialAutomation,
                DesktopCommand::TutorialAdvance,
                move |context| async move {
                    handler_state.command_tutorial_main_opened(&context).await;
                    Ok(())
                },
            )
            .await;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainOpenAction {
    SetupPrompt,
    ShowMain,
}

fn main_open_action(phase: crate::command_guard::OnboardingPhase) -> MainOpenAction {
    match phase {
        crate::command_guard::OnboardingPhase::Setup => MainOpenAction::SetupPrompt,
        crate::command_guard::OnboardingPhase::TutorialFinishing
        | crate::command_guard::OnboardingPhase::Tutorial { .. }
        | crate::command_guard::OnboardingPhase::Normal => MainOpenAction::ShowMain,
    }
}

fn should_notify_tutorial_main_opened(phase: crate::command_guard::OnboardingPhase) -> bool {
    matches!(
        phase,
        crate::command_guard::OnboardingPhase::Tutorial {
            step: coosenpai_core::onboarding::TutorialStep::Chat,
            ..
        }
    )
}

pub fn toggle_main(app: &AppHandle) {
    if main_is_visible(app) {
        hide_main(app);
    } else {
        show_main(app);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MainWindowPlacement {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn restore_main_window(window: &tauri::WebviewWindow, path: &PathBuf) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let Ok(placement) = serde_json::from_slice::<MainWindowPlacement>(&bytes) else {
        return;
    };
    if placement.width == 0 || placement.height == 0 {
        return;
    }
    let _ = window.set_size(tauri::PhysicalSize::new(placement.width, placement.height));
    let _ = window.set_position(tauri::PhysicalPosition::new(placement.x, placement.y));
}

fn schedule_main_window_save(app: AppHandle, path: PathBuf, revision: Arc<AtomicU64>) {
    let expected = revision.fetch_add(1, Ordering::AcqRel).saturating_add(1);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if revision.load(Ordering::Acquire) != expected {
            return;
        }
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        let placement = match (window.outer_position(), window.outer_size()) {
            (Ok(position), Ok(size)) => MainWindowPlacement {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
            },
            _ => return,
        };
        let result = tokio::task::spawn_blocking(move || {
            let bytes = serde_json::to_vec_pretty(&placement)?;
            atomic_write_bytes(&path, &bytes).map_err(serde_json::Error::io)
        })
        .await;
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => format!("error-type=persistence detail={error}"),
            Err(error) => format!("error-type=join detail={error}"),
        };
        if let Some(state) = app.try_state::<Arc<DesktopState>>() {
            let _ = state.logger.write(
                "WARN",
                &format!("ウィンドウ位置の保存に失敗しました: {error}"),
            );
        }
    });
}

fn create_tray(app: &App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "CooSenpAI を開く", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "start", "見る", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "休憩する", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "設定", true, None::<&str>)?;
    let reset_conversation = MenuItem::with_id(
        app,
        "reset-conversation",
        "会話をリセット",
        true,
        None::<&str>,
    )?;
    let onboarding = app
        .try_state::<Arc<DesktopState>>()
        .and_then(|state| {
            OnboardingStore::new(state.paths.onboarding.clone())
                .load()
                .ok()
        })
        .unwrap_or_default();
    let tutorial_active = onboarding.tutorial_active();
    let setup_required = onboarding.needs_setup();
    let quit = MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?;
    let config = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| state.runtime_config());
    let shortcut_labels = config
        .as_ref()
        .map(shortcut_menu_labels)
        .unwrap_or_default();
    let shortcut_items = shortcut_labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            MenuItem::with_id(
                app,
                format!("shortcut-info:{index}"),
                label,
                false,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let shortcut_refs = shortcut_items
        .iter()
        .map(|item| item as &dyn IsMenuItem<tauri::Wry>)
        .collect::<Vec<_>>();
    let shortcuts = Submenu::with_items(app, "ショートカット", true, &shortcut_refs)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &start,
            &stop,
            &reset_conversation,
            &settings,
            &shortcuts,
            &quit,
        ],
    )?;
    let icon = Image::from_bytes(include_bytes!("../icons/trayTemplate@2x.png"))?;
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("CooSenpAI")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::Click { .. }) {
                show_main(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                "open" => show_main(app),
                "start" => {
                    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                        let state = state.inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = state.dispatch_watch_start(CommandSource::Tray).await;
                        });
                    }
                }
                "stop" => {
                    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                        let state = state.inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let handler_state = state.clone();
                            let _ = state
                                .dispatch(
                                    CommandSource::Tray,
                                    DesktopCommand::WatchStop,
                                    move |context| async move {
                                        handler_state
                                            .command_stop_watch(&context)
                                            .await
                                            .map(|_| ())
                                            .map_err(DispatchError::handler)
                                    },
                                )
                                .await;
                        });
                    }
                }
                "reset-conversation" => {
                    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                        let state = state.inner().clone();
                        tauri::async_runtime::spawn(async move {
                            crate::bubble_conversation::show_reset_prompt(state).await;
                        });
                    }
                }
                "settings" => {
                    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                        let state = state.inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let handler_state = state.clone();
                            let _ = state
                                .dispatch(
                                    CommandSource::Tray,
                                    DesktopCommand::SettingsOpen,
                                    move |context| async move {
                                        handler_state
                                            .command_tutorial_settings_opened(&context)
                                            .await
                                            .map_err(DispatchError::handler)?;
                                        show_main(&handler_state.app);
                                        let _ = tauri::Emitter::emit(
                                            &handler_state.app,
                                            "coosenpai:settings:requested",
                                            (),
                                        );
                                        Ok(())
                                    },
                                )
                                .await;
                        });
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .build(app)?;
    stop.set_enabled(false)?;
    start.set_enabled(!setup_required)?;
    reset_conversation.set_enabled(!setup_required && !tutorial_active)?;
    app.manage(TrayControls {
        start,
        stop,
        reset_conversation,
        shortcut_items,
        setup_required: std::sync::atomic::AtomicBool::new(setup_required),
        running: std::sync::atomic::AtomicBool::new(false),
        recording: std::sync::atomic::AtomicBool::new(false),
    });
    refresh_tray_tooltip(app.handle());
    Ok(())
}

pub fn sync_onboarding(app: &AppHandle, setup_required: bool, active: bool) {
    if let Some(controls) = app.try_state::<TrayControls>() {
        controls
            .setup_required
            .store(setup_required, Ordering::Release);
        let intent_active = controls.running.load(Ordering::Acquire);
        let (start_enabled, stop_enabled, reset_enabled) =
            tray_availability(setup_required, intent_active);
        let _ = controls
            .reset_conversation
            .set_enabled(reset_enabled && !active);
        let _ = controls.start.set_enabled(start_enabled);
        let _ = controls.stop.set_enabled(stop_enabled);
    }
}

pub fn sync_tutorial(app: &AppHandle, active: bool) {
    sync_onboarding(app, false, active);
}

pub fn sync_persona(app: &AppHandle, _selected: &str) {
    refresh_tray_tooltip(app);
}

pub fn position_capture_popup(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let scale = window.scale_factor()?;
    let size = monitor.size().to_logical::<f64>(scale);
    let position = monitor.position().to_logical::<f64>(scale);
    window.set_position(LogicalPosition::new(
        position.x + (size.width - 380.0 - 24.0).max(0.0),
        position.y + 36.0,
    ))
}

pub(crate) fn show_model_popup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let window = app
        .get_webview_window("model-popup")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    position_model_popup(&window, &main)?;
    window.set_focusable(true)?;
    window.show()?;
    window.set_focus()
}

pub(crate) fn position_model_popup<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    main: &tauri::WebviewWindow<R>,
) -> tauri::Result<()> {
    let main_position = main.outer_position()?;
    let main_size = main.outer_size()?;
    let popup_size = window.outer_size()?;
    let horizontal_offset = main_size
        .width
        .saturating_sub(popup_size.width.saturating_add(12));
    window.set_position(tauri::PhysicalPosition::new(
        main_position.x + horizontal_offset as i32,
        main_position.y + 68,
    ))
}

const WINDOW_FOCUS_TIMEOUT: Duration = Duration::from_millis(500);

pub(crate) async fn show_capture_popup(
    state: &DesktopState,
    window: &tauri::WebviewWindow,
) -> Result<bool, String> {
    position_capture_popup(window).map_err(|error| error.to_string())?;
    focus_capture_popup(state, window).await
}

pub(crate) async fn focus_capture_popup(
    state: &DesktopState,
    window: &tauri::WebviewWindow,
) -> Result<bool, String> {
    let main = state
        .app
        .get_webview_window("main")
        .ok_or_else(|| "メインウィンドウがありません".to_owned())?;
    let focus_events = state.capture_popup_focus_events();
    window
        .set_focusable(true)
        .map_err(|error| error.to_string())?;
    activate_and_focus_window(&main, window, focus_events).await
}

pub(crate) async fn activate_and_focus_window(
    main_window: &tauri::WebviewWindow,
    window: &tauri::WebviewWindow,
    focus_events: tokio::sync::watch::Receiver<bool>,
) -> Result<bool, String> {
    let main_was_focused = main_window
        .is_focused()
        .map_err(|error| error.to_string())?;
    request_native_focus(
        focus_events,
        WINDOW_FOCUS_TIMEOUT,
        main_was_focused,
        || crate::platform::activate_current_application().map_err(|error| error.to_string()),
        || make_window_key_and_order_front(window),
        || order_window_back(main_window),
    )
    .await
}

async fn request_native_focus<
    Activate,
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
) -> Result<bool, String>
where
    Activate: FnOnce() -> Result<(), String>,
    MakeKeyAndOrderFront: FnOnce() -> MakeKeyAndOrderFrontFuture,
    MakeKeyAndOrderFrontFuture: Future<Output = Result<(), String>>,
    OrderMainBack: FnOnce() -> OrderMainBackFuture,
    OrderMainBackFuture: Future<Output = Result<(), String>>,
{
    activate()?;
    focus_events.mark_unchanged();
    make_key_and_order_front().await?;
    if !main_was_focused {
        order_main_back().await?;
    }
    Ok(wait_for_focus_event(&mut focus_events, timeout).await)
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

#[cfg(target_os = "macos")]
async fn run_native_window_action(
    window: &tauri::WebviewWindow,
    action: fn(*mut std::ffi::c_void) -> Result<(), String>,
) -> Result<(), String> {
    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), String>>();
    window
        .with_webview(move |webview| {
            let result = action(webview.ns_window());
            let _ = sender.send(result);
        })
        .map_err(|error| error.to_string())?;
    tokio::time::timeout(WINDOW_FOCUS_TIMEOUT, receiver)
        .await
        .map_err(|_| "ネイティブウィンドウ操作がタイムアウトしました".to_owned())?
        .map_err(|_| "ネイティブウィンドウ操作が完了しませんでした".to_owned())?
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

pub(crate) fn focus_failure_details(window: &tauri::WebviewWindow) -> String {
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
    format!("frontmost-app={frontmost} key-window={key_window}")
}

pub(crate) fn focus_failure_message(target: &str, details: &str) -> String {
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

pub fn position_speech_popup(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let scale = window.scale_factor()?;
    let size = monitor.size().to_logical::<f64>(scale);
    let position = monitor.position().to_logical::<f64>(scale);
    window.set_position(LogicalPosition::new(
        position.x + (size.width - 360.0 - 24.0).max(0.0),
        position.y + 36.0,
    ))
}

fn tray_watch_actions(intent_active: bool) -> (bool, bool) {
    (!intent_active, intent_active)
}

fn tray_availability(setup_required: bool, intent_active: bool) -> (bool, bool, bool) {
    let (start, stop) = tray_watch_actions(intent_active);
    (
        !setup_required && start,
        !setup_required && stop,
        !setup_required,
    )
}

pub fn sync_tray(app: &AppHandle, intent_active: bool) {
    if let Some(controls) = app.try_state::<TrayControls>() {
        let setup_required = controls.setup_required.load(Ordering::Acquire);
        let (start_enabled, stop_enabled, _) = tray_availability(setup_required, intent_active);
        let _ = controls.start.set_enabled(start_enabled);
        let _ = controls.stop.set_enabled(stop_enabled);
        controls
            .running
            .store(intent_active, std::sync::atomic::Ordering::Release);
    }
    update_tray_icon(app);
    refresh_tray_tooltip(app);
}

fn shortcut_menu_labels(config: &coosenpai_core::config::Config) -> Vec<String> {
    [
        ("文章を渡す", config.keymap.send_text.as_deref()),
        ("画面を渡す", config.keymap.capture_region.as_deref()),
        ("声で話す", config.keymap.microphone.as_deref()),
        ("パネルを開く", config.keymap.toggle_panel.as_deref()),
        ("見る / 休憩する", config.keymap.toggle_watch.as_deref()),
        (
            "直近の返事をコピー",
            config.keymap.copy_last_reply.as_deref(),
        ),
    ]
    .into_iter()
    .map(|(label, shortcut)| format!("{label}: {}", shortcut.unwrap_or("未設定")))
    .collect()
}

pub fn sync_shortcut_menu(app: &AppHandle, config: &coosenpai_core::config::Config) {
    let Some(controls) = app.try_state::<TrayControls>() else {
        return;
    };
    for (item, label) in controls
        .shortcut_items
        .iter()
        .zip(shortcut_menu_labels(config))
    {
        let _ = item.set_text(label);
    }
}

pub fn sync_recording(app: &AppHandle, recording: bool) {
    if let Some(controls) = app.try_state::<TrayControls>() {
        controls
            .recording
            .store(recording, std::sync::atomic::Ordering::Release);
    }
    update_tray_icon(app);
    refresh_tray_tooltip(app);
}

fn update_tray_icon(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return;
    };
    let (running, recording) = app
        .try_state::<TrayControls>()
        .map(|controls| {
            (
                controls.running.load(std::sync::atomic::Ordering::Acquire),
                controls
                    .recording
                    .load(std::sync::atomic::Ordering::Acquire),
            )
        })
        .unwrap_or_default();
    let icon = if recording {
        Ok(recording_icon())
    } else if running {
        Image::from_bytes(include_bytes!("../icons/trayWatching@2x.png"))
    } else {
        Image::from_bytes(include_bytes!("../icons/trayTemplate@2x.png"))
    };
    if let Ok(icon) = icon {
        let _ = tray.set_icon_with_as_template(Some(icon), !recording);
    }
}

fn recording_icon() -> Image<'static> {
    const SIZE: u32 = 36;
    let mut pixels = vec![0_u8; (SIZE * SIZE * 4) as usize];
    let center = (SIZE as f64 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let distance =
                ((f64::from(x) - center).powi(2) + (f64::from(y) - center).powi(2)).sqrt();
            if distance <= 11.5 {
                let offset = ((y * SIZE + x) * 4) as usize;
                pixels[offset..offset + 4].copy_from_slice(&[224, 70, 70, 255]);
            }
        }
    }
    Image::new_owned(pixels, SIZE, SIZE)
}

fn refresh_tray_tooltip(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return;
    };
    let running = app
        .try_state::<TrayControls>()
        .is_some_and(|controls| controls.running.load(std::sync::atomic::Ordering::Acquire));
    let recording = app.try_state::<TrayControls>().is_some_and(|controls| {
        controls
            .recording
            .load(std::sync::atomic::Ordering::Acquire)
    });
    let display_name = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| state.runtime_snapshot().companion_display_name)
        .unwrap_or_else(|| "CooSenpAI".to_owned());
    let state = if recording {
        "録音中"
    } else if running {
        "見ています"
    } else {
        "休憩中"
    };
    let _ = tray.set_tooltip(Some(format!("{display_name}: {state}")));
}

