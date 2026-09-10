mod activation_policy;
mod app_presenter;
mod app_update;
mod apple_termination;
mod avatar;
mod avatar_presenter;
mod avatar_scene_presenter;
mod avatar_view;
mod avatar_window;
mod bubble_click;
mod bubble_controls_presenter;
mod bubble_conversation;
mod bubbles;
mod capture;
mod capture_notice;
mod command_dispatcher;
mod command_guard;
mod command_policy;
mod command_source_policy;
mod command_types;
mod commands;
mod commands_bubble;
mod commands_capture;
mod commands_chat_view;
mod commands_config;
mod commands_conversation;
mod commands_details;
mod commands_license;
mod commands_memory;
mod commands_onboarding;
mod commands_persona;
mod commands_provider_api_keys;
mod commands_provider_models;
mod commands_speech;
mod commands_targets;
mod commands_ui;
mod commands_voice_output;
mod commands_work;
mod composer_presenter;
mod config_update;
mod conversation_presenter;
mod core_runtime_port;
mod e2e_logs;
mod factory;
mod hearing;
mod hearing_lifecycle;
mod input_popup;
mod model_catalog;
mod model_picker_presenter;
mod motion_settings_presenter;
mod own_bounds;
mod platform;
mod presentation;
mod selection_session;
mod shortcut_startup;
mod shutdown;
mod snapshot;
mod speech;
mod speech_lifecycle;
mod speech_transcript;
mod speech_view;
mod state;
mod status_presenter;
mod tutorial;
mod tutorial_notice;
mod tutorial_ui;
mod ui_command_effects;
mod ui_commands;
mod ui_events;
mod ui_load;
mod ui_port;
mod ui_presenters;
mod ui_root;
mod update_archive;
mod update_check;
mod update_format;
mod update_install;
mod update_system;
mod update_transport;
mod voice_output;
mod watch;
mod webview_event;
mod window_bubble;
mod windows;
mod work;
mod work_approval_presenter;

use crate::command_guard::{CommandSource, DesktopCommand, DispatchError};
use crate::shutdown::ShutdownCoordinator;
use crate::state::DesktopState;

use coosenpai_core::ports::RuntimeLogger;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

fn app_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

fn startup_onboarding_command(needs_setup: bool, tutorial_active: bool) -> Option<DesktopCommand> {
    match (needs_setup, tutorial_active) {
        (true, _) => Some(DesktopCommand::SetupPrompt),
        (false, true) => Some(DesktopCommand::TutorialResume),
        (false, false) => None,
    }
}

async fn announce_startup_onboarding(state: Arc<DesktopState>) {
    let mut retry_delay = Duration::from_millis(250);
    let mut attempt = 1_u64;
    loop {
        let needs_setup = state.tutorial_needs_setup().await;
        let tutorial_active = state.tutorial_is_active().await;
        let Some(command) = startup_onboarding_command(needs_setup, tutorial_active) else {
            return;
        };
        let handler_state = state.clone();
        let result = state
            .dispatch(
                CommandSource::TutorialAutomation,
                command,
                move |context| async move {
                    handler_state
                        .command_announce_initial_onboarding(&context)
                        .await
                        .map_err(DispatchError::handler)
                },
            )
            .await;
        let error = match result {
            Ok(()) => return,
            Err(error) => error,
        };
        let _ = state.logger.write(
            "WARN",
            &format!("初回セットアップの表示に失敗しました: {error}"),
        );
        attempt = attempt.saturating_add(1);
        let _ = state.logger.write(
            "INFO",
            &format!(
                "初回セットアップ吹き出しの表示を再試行します: attempt={attempt} delay-ms={}",
                retry_delay.as_millis()
            ),
        );
        tokio::select! {
            () = state.cancellation.cancelled() => return,
            () = tokio::time::sleep(retry_delay) => {}
        }
        retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(5));
    }
}

fn should_restore_watch_on_startup(
    watch_enabled: bool,
    screen_recording_granted: bool,
    setup_required: bool,
    tutorial_active: bool,
    runtime_active: bool,
) -> bool {
    watch_enabled
        && screen_recording_granted
        && !setup_required
        && !tutorial_active
        && runtime_active
}

async fn restore_watch_on_startup(state: Arc<DesktopState>) {
    let _watch_intent = state.watch_intent_lock.lock().await;
    let config = state.runtime_config();
    let setup_required = state.tutorial_needs_setup().await;
    let tutorial_active = state.tutorial_is_active().await;
    let snapshot = state.snapshot().await;
    let screen_recording_granted = snapshot.screen_recording_status == "granted";

    if should_restore_watch_on_startup(
        config.watch.enabled,
        screen_recording_granted,
        setup_required,
        tutorial_active,
        state.is_runtime_active(),
    ) {
        match state.dispatch_watch_restore().await {
            Ok(snapshot) if snapshot.watch_intent_active => {
                let _ = state.logger.write("INFO", "前回の見守り状態を復元しました");
            }
            Ok(_) => {}
            Err(error) => {
                let _ = state.logger.write(
                    "WARN",
                    &format!("前回の見守り状態の復元に失敗しました: {error}"),
                );
            }
        }
    } else if config.watch.enabled
        && !screen_recording_granted
        && !setup_required
        && !tutorial_active
        && state.is_runtime_active()
    {
        state
            .show_watch_start_rejection(
                snapshot
                    .screen_recording_message
                    .as_deref()
                    .unwrap_or("画面収録の許可が必要です"),
            )
            .await;
    }
}

mod desktop_startup;

fn setup_desktop(app: &mut tauri::App) -> anyhow::Result<()> {
    let state = tauri::async_runtime::block_on(DesktopState::initialize(app.handle().clone()))?;
    app.manage(bubble_click::BubbleClickState(state.clone()));
    app.manage(state);
    windows::configure(app)?;
    if let Some(state) = app.try_state::<Arc<DesktopState>>() {
        let state = state.inner().clone();
        update_check::start(state.clone());
        model_catalog::start(state.clone());
        tauri::async_runtime::spawn(async move {
            let config = state.runtime_config();
            let bindings = capture::ShortcutBindings::from_config(&config);
            shortcut_startup::sync(&state, bindings, 0).await;
            announce_startup_onboarding(state.clone()).await;
            restore_watch_on_startup(state).await;
        });
    }
    eprintln!("CooSenpAI desktop ready");
    Ok(())
}

pub fn run() -> anyhow::Result<()> {
    let signals = tauri::async_runtime::block_on(shutdown::install_early_signals())?;
    let app = tauri::Builder::default()
        .manage(app_update::AppUpdater::default())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let Some(state) = app.try_state::<Arc<DesktopState>>() else {
                        return;
                    };
                    state.ui.input(
                        ui_events::UiView::GlobalShortcut,
                        ui_events::UiEvent::GlobalShortcut {
                            shortcut: shortcut.to_string(),
                            pressed: event.state()
                                == tauri_plugin_global_shortcut::ShortcutState::Pressed,
                        },
                    );
                })
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            ui_root::application_input(app, ui_events::UiEvent::OpenMain);
        }))
        .setup(|app| desktop_startup::complete(setup_desktop(app), app))
        .invoke_handler(tauri::generate_handler![
            commands_ui::ui_view_mounted,
            commands_ui::settings_close,
            commands_ui::ui_panel_event,
            avatar_window::avatar_state_get,
            avatar_window::avatar_show,
            avatar_window::avatar_hide,
            avatar_window::avatar_toggle,
            avatar_window::avatar_model_changed,
            commands_voice_output::voice_output_status,
            commands_voice_output::voice_output_voices,
            commands_voice_output::voice_output_open_link,
            commands_voice_output::voice_output_stop,
            commands_voice_output::voice_output_test,
            commands_work::work_status,
            commands_work::work_stop,
            commands_work::work_approve,
            app_update::app_update_status,
            app_update::app_update_check,
            app_update::app_update_install,
            commands::snapshot_get,
            commands::watch_start,
            commands::watch_stop,
            commands_targets::running_apps_list,
            commands_targets::watch_target_add,
            commands_targets::watch_target_remove,
            commands_targets::watch_target_set_enabled,
            commands::chat_send,
            commands::chat_cancel,
            commands::chat_retry,
            commands::config_get,
            commands_config::config_get_persisted,
            commands::config_update,
            commands::model_popup_open,
            commands::model_popup_close,
            commands::model_popup_snapshot,
            commands::model_popup_config_update,
            commands::model_popup_companion_model_catalog,
            commands::model_popup_opencode_models_reload,
            commands_details::details_open,
            commands_details::details_snapshot,
            commands_details::details_dataflow_log,
            commands::companion_assertiveness_set,
            commands::persona_list,
            commands::provider_models,
            commands_provider_api_keys::provider_api_keys_get,
            commands_provider_api_keys::provider_api_key_set,
            commands_provider_api_keys::provider_api_key_delete,
            commands::persona_select,
            commands::persona_select_setup,
            commands_persona::persona_reload,
            commands_persona::persona_get,
            commands_persona::persona_save,
            commands_persona::persona_delete,
            commands_persona::persona_restore,
            commands_memory::memory_list,
            commands_memory::memory_confirm,
            commands_memory::memory_reject,
            commands_memory::memory_confirm_update,
            commands_memory::memory_reject_update,
            commands_memory::memory_delete,
            commands_memory::memory_consolidate,
            commands::panel_open_system_settings,
            commands_license::license_document_open,
            commands::app_relaunch,
            commands::app_exit,
            commands::advice_selected,
            commands::settings_requested,
            commands::chat_input_state,
            commands::unread_read,
            commands_bubble::bubble_dismiss,
            commands_bubble::bubble_fast_forward,
            commands_bubble::bubble_navigate,
            commands_bubble::settings_appearance_preview,
            commands::bubble_click,
            commands::bubble_hover,
            commands::bubble_focus,
            commands_bubble::bubble_snapshot,
            commands_bubble::bubble_renderer_ready,
            commands_bubble::bubble_ack,
            commands::bubble_passthrough,
            commands::bubble_resize,
            commands_onboarding::bubble_interact,
            commands_capture::capture_popup_snapshot,
            commands_capture::capture_popup_send,
            commands_capture::capture_popup_edit,
            commands_capture::capture_popup_key,
            commands_capture::capture_popup_action,
            commands_capture::capture_popup_retry,
            commands::model_picker_input,
            commands_chat_view::composer_input,
            commands_chat_view::work_approval_input,
            commands_chat_view::app_view_input,
            commands_chat_view::app_select_persona,
            commands_chat_view::avatar_scene_input,
            commands_chat_view::motion_settings_input,
            commands_chat_view::conversation_input,
            commands_bubble::bubble_view_input,
            commands_capture::capture_popup_cancel,
            commands_capture::capture_popup_open_accessibility_settings,
            commands_capture::attachment_read,
            commands_speech::speech_start,
            commands_speech::speech_finish,
            commands_speech::speech_cancel,
            commands_speech::speech_popup_snapshot,
            commands_speech::speech_popup_send,
            commands_speech::speech_popup_edit,
            commands_speech::speech_popup_key,
            commands_speech::speech_popup_cancel,
            commands_speech::speech_open_system_settings,
            commands_onboarding::tutorial_next,
            commands_onboarding::tutorial_settings_presented,
            commands_onboarding::tutorial_finish,
            commands_onboarding::tutorial_restart,
            commands_onboarding::setup_prompt,
            commands_onboarding::setup_restart,
            commands_onboarding::conversation_reset,
            commands_conversation::conversation_select,
            commands::companion_emotions_reset,
        ])
        .build(app_context())?;

    let shutdown = ShutdownCoordinator::new(app.handle().clone());
    app.manage(shutdown.clone());
    shutdown.attach_signals(signals);
    let apple_shutdown = shutdown.clone();
    apple_termination::install(Arc::new(move || apple_shutdown.handle_apple_event()))
        .map_err(anyhow::Error::msg)?;

    let event_shutdown = shutdown;
    app.run(move |app, event| match event {
        tauri::RunEvent::ExitRequested { api, code, .. } => {
            let restart = code == Some(tauri::RESTART_EXIT_CODE);
            if event_shutdown.handle_exit_requested(restart) {
                api.prevent_exit();
            }
        }
        tauri::RunEvent::Reopen { .. } => {
            ui_root::application_input(app, ui_events::UiEvent::OpenMain)
        }
        _ => {}
    });
    Ok(())
}

mod screen_capture_gate;

mod placement_presenter;

mod snapshot_presenter;

mod speech_presenter;

mod hearing_presenter;

mod watch_presenter;

mod tray_presenter;

mod tutorial_projection;

mod thought_presenter;

mod presence_presenter;

mod tutorial_card_presenter;
mod tutorial_effects;
mod tutorial_events;

mod tutorial_sequence_presenter;

mod tutorial_notice_presenter;

mod tutorial_progress_presenter;

mod tutorial_response_presenter;

mod tutorial_lifecycle_presenter;

mod tutorial_progress_events;

mod panels;

mod settings_preview_presenter;
