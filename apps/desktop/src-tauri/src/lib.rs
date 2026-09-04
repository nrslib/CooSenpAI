mod apple_termination;
mod avatar;
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
mod commands_config;
mod commands_memory;
mod commands_onboarding;
mod commands_persona;
mod commands_provider_api_keys;
mod commands_provider_models;
mod commands_speech;
mod commands_targets;
mod config_update;
mod core_runtime_port;
mod factory;
mod hearing;
mod hearing_lifecycle;
mod input_popup;
mod model_catalog;
mod own_bounds;
mod platform;
mod shortcut_startup;
mod shutdown;
mod snapshot;
mod speech;
mod speech_lifecycle;
mod speech_transcript;
mod state;
mod tutorial;
mod tutorial_notice;
mod update_check;
mod watch;
mod window_bubble;
mod windows;

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

pub fn run() -> anyhow::Result<()> {
    let signals = tauri::async_runtime::block_on(shutdown::install_early_signals())?;
    let app = tauri::Builder::default()
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
                    let pressed = shortcut.to_string();
                    let action = state.shortcut_coordinator.action(&pressed);
                    if action == Some(capture::ShortcutAction::Microphone) {
                        let pressed =
                            event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed;
                        let state = state.inner().clone();
                        let mode = state.runtime_config().speech.mode;
                        if mode != "toggle" && !pressed {
                            tauri::async_runtime::spawn(async move {
                                let handler_state = state.clone();
                                let _ = state
                                    .dispatch(
                                        CommandSource::GlobalShortcut,
                                        DesktopCommand::SpeechFinish,
                                        move |context| async move {
                                            handler_state.command_speech_finish(&context);
                                            Ok(())
                                        },
                                    )
                                    .await;
                            });
                            return;
                        }
                        if !pressed {
                            return;
                        }
                        tauri::async_runtime::spawn(async move {
                            let finishing =
                                input_popup::microphone_action(&mode, state.speech_is_recording())
                                    == input_popup::InputPopupStartAction::FinishSpeech;
                            let command = if finishing {
                                DesktopCommand::SpeechFinish
                            } else {
                                DesktopCommand::SpeechStart
                            };
                            let handler_state = state.clone();
                            let result = state
                                .dispatch(
                                    CommandSource::GlobalShortcut,
                                    command,
                                    move |context| async move {
                                        if finishing {
                                            handler_state.command_speech_finish(&context);
                                            Ok(())
                                        } else {
                                            handler_state
                                                .command_speech_begin(
                                                    &context,
                                                    speech::SpeechSource::Shortcut,
                                                )
                                                .await
                                                .map_err(DispatchError::handler)
                                        }
                                    },
                                )
                                .await
                                .map_err(|error| error.format_for_user());
                            if let Err(error) = result {
                                capture::publish_transient_shortcut_error(state, error).await;
                            }
                        });
                        return;
                    }
                    if action == Some(capture::ShortcutAction::SpeechCancel) {
                        if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                            let state = state.inner().clone();
                            tauri::async_runtime::spawn(async move {
                                let handler_state = state.clone();
                                let _ = state
                                    .dispatch(
                                        CommandSource::GlobalShortcut,
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
                        return;
                    }
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        match action {
                            Some(capture::ShortcutAction::CaptureRegion) => capture::begin(app),
                            Some(capture::ShortcutAction::SendText) => capture::begin_text(app),
                            Some(capture::ShortcutAction::TogglePanel) => windows::toggle_main(app),
                            Some(capture::ShortcutAction::ToggleWatch) => {
                                let state = state.inner().clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ = state
                                        .dispatch_watch_toggle(CommandSource::GlobalShortcut)
                                        .await;
                                });
                            }
                            Some(capture::ShortcutAction::CopyLastReply) => {
                                let state = state.inner().clone();
                                tauri::async_runtime::spawn(async move {
                                    let result =
                                        state::dispatch_copy_last_reply_shortcut(state.clone())
                                            .await;
                                    if let Err(error) = result {
                                        capture::publish_transient_shortcut_error(
                                            state,
                                            error.format_for_user(),
                                        )
                                        .await;
                                    }
                                });
                            }
                            Some(capture::ShortcutAction::Microphone)
                            | Some(capture::ShortcutAction::SpeechCancel)
                            | None => {}
                        }
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_main(app);
        }))
        .setup(|app| {
            let state =
                tauri::async_runtime::block_on(DesktopState::initialize(app.handle().clone()))
                    .map_err(|error| -> Box<dyn std::error::Error> {
                        Box::new(std::io::Error::other(error.to_string()))
                    })?;
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
        })
        .invoke_handler(tauri::generate_handler![
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
            commands::config_update,
            commands::model_popup_open,
            commands::model_popup_close,
            commands::model_popup_snapshot,
            commands::model_popup_config_update,
            commands::model_popup_companion_model_catalog,
            commands::model_popup_opencode_models_reload,
            commands::companion_assertiveness_set,
            commands::persona_list,
            commands::provider_models,
            commands_provider_api_keys::provider_api_keys_get,
            commands_provider_api_keys::provider_api_key_set,
            commands_provider_api_keys::provider_api_key_delete,
            commands::persona_select,
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
            commands::app_relaunch,
            commands::app_exit,
            commands::advice_selected,
            commands::settings_requested,
            commands::chat_input_state,
            commands::unread_read,
            commands_bubble::bubble_dismiss,
            commands_bubble::tutorial_sequence_fast_forward,
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
            commands_capture::capture_popup_cancel,
            commands_capture::capture_popup_open_accessibility_settings,
            commands_capture::attachment_read,
            commands_speech::speech_start,
            commands_speech::speech_finish,
            commands_speech::speech_cancel,
            commands_speech::speech_popup_snapshot,
            commands_speech::speech_popup_send,
            commands_speech::speech_popup_cancel,
            commands_speech::speech_open_system_settings,
            commands_onboarding::tutorial_next,
            commands_onboarding::tutorial_settings_presented,
            commands_onboarding::tutorial_finish,
            commands_onboarding::tutorial_restart,
            commands_onboarding::setup_prompt,
            commands_onboarding::setup_restart,
            commands_onboarding::conversation_reset,
        ])
        .build(app_context())?;

    let shutdown = ShutdownCoordinator::new(app.handle().clone());
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
        tauri::RunEvent::Reopen { .. } => windows::show_main(app),
        _ => {}
    });
    Ok(())
}

