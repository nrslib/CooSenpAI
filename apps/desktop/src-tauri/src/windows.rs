use crate::command_guard::CommandSource;
use crate::placement_presenter::{MainWindowPlacement, PlacementEvent};
use crate::state::DesktopState;
use crate::tray_presenter::{shortcut_menu_labels_for_locale, TrayIcon, TrayView};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::RuntimeLogger;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, Submenu};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::webview::{PageLoadEvent, WebviewWindowBuilder};
use tauri::{App, AppHandle, LogicalPosition, Manager, Runtime, WindowEvent};

#[path = "windows_tray.rs"]
mod tray;
#[path = "window_focus.rs"]
pub(crate) mod window_focus;
use tray::recording_icon;
pub(crate) use window_focus::activate_and_focus_window;

struct TrayControls {
    open: MenuItem<tauri::Wry>,
    start: MenuItem<tauri::Wry>,
    stop: MenuItem<tauri::Wry>,
    settings: MenuItem<tauri::Wry>,
    reset_conversation: MenuItem<tauri::Wry>,
    shortcuts: Submenu<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
    shortcut_items: Vec<MenuItem<tauri::Wry>>,
}

pub fn configure(app: &mut App) -> tauri::Result<()> {
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let avatar = app
        .get_webview_window("avatar")
        .ok_or(tauri::Error::WindowNotFound)?;
    avatar.set_visible_on_all_workspaces(true)?;
    configure_full_screen_space_behavior(&avatar)?;
    avatar.on_window_event({
        let app = app.handle().clone();
        move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                    state.ui.input(
                        crate::ui_events::UiView::Avatar,
                        crate::ui_events::UiEvent::AvatarVisibility(Some(false)),
                    );
                }
            }
        }
    });
    let bubble = create_bubble_window(app)?;
    let bubble_focus_state = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| state.inner().clone());
    bubble.on_window_event(move |event| {
        if let WindowEvent::Focused(focused) = event {
            if let Some(state) = bubble_focus_state.as_ref() {
                state.ui.input(
                    crate::ui_events::UiView::Bubble,
                    crate::ui_events::UiEvent::BubbleFocused(*focused),
                );
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
    main.on_window_event({
        let app = app.handle().clone();
        move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                    state.ui.input(
                        crate::ui_events::UiView::Chat,
                        crate::ui_events::UiEvent::Close,
                    );
                }
            }
            WindowEvent::Focused(focused) => {
                if let Some(state) = app.try_state::<Arc<DesktopState>>() {
                    state.ui.input(
                        crate::ui_events::UiView::Chat,
                        crate::ui_events::UiEvent::MainFocused(*focused),
                    );
                }
            }
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                if let Some(path) = placement.as_ref() {
                    observe_main_window_placement(&app, path);
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
        move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Some(state) = state.clone() {
                    dispatch_capture_cancel(state);
                }
            }
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
                    state.ui.input(
                        crate::ui_events::UiView::SpeechPopup,
                        crate::ui_events::UiEvent::Voice(crate::ui_events::VoiceAction::Cancel),
                    );
                }
            }
        }
    });
    let model = app
        .get_webview_window("model-popup")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    configure_full_screen_space_behavior(&model)?;
    position_model_popup(&model, &main)?;
    let model_ui = app.state::<Arc<DesktopState>>().ui.clone();
    model.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            model_ui.input(
                crate::ui_events::UiView::ModelPicker,
                crate::ui_events::UiEvent::Close,
            );
        }
    });
    let details = app
        .get_webview_window("details")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let details_ui = app.state::<Arc<DesktopState>>().ui.clone();
    details.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            details_ui.input(
                crate::ui_events::UiView::Details,
                crate::ui_events::UiEvent::Close,
            );
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

/// 送信ポップアップの取消（Esc・閉じる要求・ウインドウ外クリック）を共通の経路で dispatch する。
pub(crate) fn dispatch_capture_cancel(state: Arc<DesktopState>) {
    state
        .capture
        .post(crate::capture::CaptureEvent::PopupCancel {
            generation: state.capture.view().generation,
            source: crate::capture::CancelSource::CloseButton,
        });
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

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendOutcome {
    Accepted,
    Failed,
    #[cfg(test)]
    Rejected,
}

/// ポップアップからの送信が受理・失敗・拒否のどれで終わっても、
/// 結果と理由を読めるようメイン画面を表示して前面に出す。
#[cfg(test)]
pub(crate) fn present_main_after_send(
    logger: &dyn RuntimeLogger,
    outcome: SendOutcome,
    present_main: impl FnOnce(),
) {
    let label = match outcome {
        SendOutcome::Accepted => "受理",
        SendOutcome::Failed => "失敗",
        #[cfg(test)]
        SendOutcome::Rejected => "拒否",
    };
    let _ = logger.write(
        "INFO",
        &format!("送信の{label}にあわせてメイン画面を前面に出します"),
    );
    present_main();
}

pub(crate) fn apply_main_hide<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("メインウィンドウがありません")?;
    window
        .hide()
        .map_err(|error| format!("メインウィンドウを閉じられません: {error}"))
}

pub(crate) async fn apply_main_show(app: &AppHandle) -> Result<(), String> {
    let Some(state) = app.try_state::<Arc<DesktopState>>() else {
        return Err(text(TextKey::WindowStateUnavailable, Locale::Ja).to_owned());
    };
    let state = state.inner().clone();
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let window = state
        .app
        .get_webview_window("main")
        .ok_or_else(|| text(TextKey::MainWindowMissing, locale).to_owned())?;
    window.show().map_err(|error| {
        text(TextKey::MainWindowShowFailed, locale).replace("{error}", &error.to_string())
    })?;
    window.set_focus().map_err(|error| {
        text(TextKey::MainWindowFocusFailed, locale).replace("{error}", &error.to_string())
    })?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
enum MainOpenAction {
    SetupPrompt,
    ShowMain,
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

fn observe_main_window_placement(app: &AppHandle, path: &std::path::Path) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) {
        crate::ui_root::application_input(
            app,
            crate::ui_events::UiEvent::Placement(PlacementEvent::Changed {
                path: path.to_owned(),
                placement: MainWindowPlacement {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                },
            }),
        );
    }
}

fn create_tray(app: &App) -> tauri::Result<()> {
    let locale = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| Locale::from_config(&state.runtime_config().ui.language))
        .unwrap_or(Locale::Ja);
    let open = MenuItem::with_id(
        app,
        "open",
        text(TextKey::WindowOpen, locale),
        true,
        None::<&str>,
    )?;
    let start = MenuItem::with_id(
        app,
        "start",
        text(TextKey::WindowWatchStart, locale),
        true,
        None::<&str>,
    )?;
    let stop = MenuItem::with_id(
        app,
        "stop",
        text(TextKey::WindowWatchStop, locale),
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(
        app,
        "settings",
        text(TextKey::WindowSettings, locale),
        true,
        None::<&str>,
    )?;
    let reset_conversation = MenuItem::with_id(
        app,
        "reset-conversation",
        text(TextKey::WindowResetConversation, locale),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        text(TextKey::WindowQuit, locale),
        true,
        None::<&str>,
    )?;
    let config = app
        .try_state::<Arc<DesktopState>>()
        .map(|state| state.runtime_config());
    let shortcut_labels = config
        .as_ref()
        .map(|config| shortcut_menu_labels_for_locale(config, locale))
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
    let shortcuts = Submenu::with_items(
        app,
        text(TextKey::WindowShortcuts, locale),
        true,
        &shortcut_refs,
    )?;
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
                crate::ui_root::application_input(
                    tray.app_handle(),
                    crate::ui_events::UiEvent::OpenMain,
                );
            }
        })
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if let Some(event) = tray_ui_event(id) {
                crate::ui_root::application_input(app, event);
            }
        })
        .build(app)?;
    stop.set_enabled(false)?;
    start.set_enabled(false)?;
    reset_conversation.set_enabled(false)?;
    app.manage(TrayControls {
        open,
        start,
        stop,
        settings,
        reset_conversation,
        shortcuts,
        quit,
        shortcut_items,
    });
    crate::ui_root::application_input(app.handle(), crate::ui_events::UiEvent::TrayMounted);
    Ok(())
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

pub(crate) fn show_details<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let window = app
        .get_webview_window("details")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
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

pub(crate) fn render_tray(app: &AppHandle, view: &TrayView) -> Result<(), String> {
    let controls = app
        .try_state::<TrayControls>()
        .ok_or("トレイのメニューがありません")?;
    apply_tray(&controls, app, view).map_err(|error| format!("トレイの更新に失敗しました: {error}"))
}

fn apply_tray(controls: &TrayControls, app: &AppHandle, view: &TrayView) -> tauri::Result<()> {
    controls.start.set_enabled(view.start_enabled)?;
    controls.stop.set_enabled(view.stop_enabled)?;
    controls
        .reset_conversation
        .set_enabled(view.reset_enabled)?;
    for (item, title) in [
        &controls.open,
        &controls.start,
        &controls.stop,
        &controls.settings,
        &controls.reset_conversation,
    ]
    .into_iter()
    .zip(&view.titles)
    {
        item.set_text(title)?;
    }
    controls.shortcuts.set_text(&view.titles[5])?;
    controls.quit.set_text(&view.titles[6])?;
    for (item, label) in controls.shortcut_items.iter().zip(&view.shortcuts) {
        item.set_text(label)?;
    }
    let tray = app
        .tray_by_id("main-tray")
        .ok_or(tauri::Error::WindowNotFound)?;
    let icon = match view.icon {
        TrayIcon::Recording => recording_icon(),
        TrayIcon::Watching => Image::from_bytes(include_bytes!("../icons/trayWatching@2x.png"))?,
        TrayIcon::Paused => Image::from_bytes(include_bytes!("../icons/trayTemplate@2x.png"))?,
    };
    tray.set_icon_with_as_template(Some(icon), view.icon != TrayIcon::Recording)?;
    tray.set_tooltip(Some(&view.tooltip))?;
    Ok(())
}

pub(crate) fn tray_ui_event(id: &str) -> Option<crate::ui_events::UiEvent> {
    use crate::ui_events::UiEvent;
    match id {
        "start" | "stop" => {
            let (reply, _) = tokio::sync::oneshot::channel();
            Some(UiEvent::UserCommand(if id == "start" {
                crate::ui_commands::UserCommand::WatchStart {
                    source: CommandSource::Tray,
                    reply,
                }
            } else {
                crate::ui_commands::UserCommand::WatchStop {
                    source: CommandSource::Tray,
                    reply,
                }
            }))
        }
        "open" => Some(UiEvent::OpenMain),
        "reset-conversation" => Some(UiEvent::ResetPromptRequested),
        "settings" => Some(UiEvent::OpenSettings),
        "quit" => Some(UiEvent::Shutdown),
        _ => None,
    }
}

