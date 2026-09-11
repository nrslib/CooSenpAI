use crate::bubbles::{self, BubbleRecord, BubbleState};
pub(crate) use crate::config_update::{
    ConfigCommitError, ConfigUpdateCoordinator, ConfigUpdateTransaction,
};
use crate::factory::{bundled_persona_directory, DesktopRuntimeFactory};
use crate::own_bounds::TauriOwnWindowBounds;
use crate::snapshot::AppSnapshot;
use anyhow::{Context, Result};
use coosenpai_core::companion_storage::CompanionStorage;
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::conversation_archive::list_conversation_generations;
use coosenpai_core::logging::FileLogger;
use coosenpai_core::notification::NotificationConsumer;
use coosenpai_core::persistence::WatchLock;
use coosenpai_core::ports::{
    ClipboardReader, ClipboardWriter, ProviderApiKeyStore, RuntimeLogger, SelectedTextCopyPort,
};
use coosenpai_core::runtime::{RuntimeError, RuntimeErrorKind, RuntimeHandle, RuntimeLastError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;

#[path = "state_bubble_click.rs"]
mod bubble_click;

#[path = "state_clipboard.rs"]
mod clipboard;
pub(crate) use clipboard::dispatch_copy_last_reply_shortcut;
#[path = "state_audio.rs"]
mod audio;
#[path = "state_command_api.rs"]
mod command_api;
#[path = "state_conversation.rs"]
mod conversation_state;
#[path = "state_permission.rs"]
mod permission;
#[path = "state_persona.rs"]
mod persona;
#[path = "state_presence.rs"]
mod presence;
#[path = "state_runtime.rs"]
mod runtime_state;
#[path = "state_setup.rs"]
mod setup;
#[path = "state_startup.rs"]
mod startup;
#[path = "state_startup_context.rs"]
mod startup_context;
#[path = "state_tutorial_finish.rs"]
mod tutorial_finish;
#[path = "state_tutorial_notice_effects.rs"]
mod tutorial_notice_effects;
#[path = "state_tutorial_progress.rs"]
mod tutorial_progress;
pub(crate) use tutorial_progress::TutorialResponseStatus;
#[path = "state_tutorial_sequence.rs"]
mod tutorial_sequence;
#[path = "state_tutorial.rs"]
pub(crate) mod tutorial_state;
#[path = "state_user_input.rs"]
pub(crate) mod user_input;
#[path = "state_watch.rs"]
mod watch_state;
pub(crate) use watch_state::WatchStartIntent;
use watch_state::{WatchControl, WatchLifecycle};

pub(crate) struct DesktopState {
    pub(crate) work: Arc<crate::work::WorkController>,
    pub app: AppHandle,
    pub(crate) main_window_focused: AtomicBool,
    bubble_focus: watch::Sender<bool>,
    pub bubbles: Arc<Mutex<BubbleState>>,
    pub(crate) capture: crate::capture::CaptureHandle,
    pub(crate) ui: crate::ui_root::UiHandle,
    pub(crate) screen_capture_gate: crate::screen_capture_gate::ScreenCaptureGate,
    pub clipboard_reader: Arc<dyn coosenpai_core::ports::ClipboardReader>,
    pub selected_text_copier: Arc<dyn SelectedTextCopyPort>,
    pub(crate) clipboard_writer: Arc<dyn coosenpai_core::ports::ClipboardWriter>,
    pub(crate) speech: Arc<crate::speech::SpeechController>,
    hearing: Arc<crate::hearing::HearingController>,
    pub(crate) voice_output: Arc<crate::voice_output::VoiceOutputController>,
    pub(crate) tutorial: Arc<Mutex<crate::tutorial::TutorialController>>,
    pub shortcut_coordinator: Arc<crate::capture::ShortcutCoordinator>,
    pub(crate) command_firewall: crate::command_guard::CommandFirewall,
    pub input_active: AtomicBool,
    pub cancellation: CancellationToken,
    runtime: RuntimeHandle,
    pub factory: Arc<DesktopRuntimeFactory>,
    pub paths: ConfigPaths,
    pub logger: Arc<FileLogger>,
    pub own_bounds: Arc<TauriOwnWindowBounds>,
    conversation_sync: Arc<Mutex<()>>,
    screen_permission: Mutex<permission::ScreenPermissionCache>,
    pub(crate) snapshot: Arc<std::sync::Mutex<AppSnapshot>>,
    pub(crate) config_update: ConfigUpdateCoordinator,
    watch_control: Mutex<WatchControl>,
    pub(crate) watch_intent_lock: Mutex<()>,
    runtime_active: AtomicBool,
    shutting_down: AtomicBool,
    presence_startup_pending: AtomicBool,
    presence_inflight: Mutex<Option<String>>,
    _watch_lock: WatchLock,
}

impl DesktopState {
    pub(crate) fn core_runtime(&self) -> &dyn crate::core_runtime_port::CoreRuntimePort {
        &self.runtime
    }

    pub(crate) fn runtime_config(&self) -> Config {
        self.core_runtime().config()
    }

    pub(crate) fn runtime_snapshot(&self) -> coosenpai_core::runtime::RuntimeSnapshot {
        self.core_runtime().snapshot()
    }

    pub(crate) fn bubble_focus_events(&self) -> watch::Receiver<bool> {
        self.bubble_focus.subscribe()
    }

    pub(crate) fn set_bubble_focused(&self, focused: bool) {
        self.bubble_focus.send_replace(focused);
    }

    pub(crate) async fn input_popup_kind(&self) -> Option<crate::input_popup::InputPopupKind> {
        let capture_kind = self.capture.view().kind;
        if let Some(kind) = capture_kind {
            return Some(match kind {
                crate::capture::CaptureKind::Image => {
                    crate::input_popup::InputPopupKind::CaptureImage
                }
                crate::capture::CaptureKind::Text => {
                    crate::input_popup::InputPopupKind::CaptureText
                }
                crate::capture::CaptureKind::Voice => crate::input_popup::InputPopupKind::Speech,
            });
        }
        (self.speech_resource_phase() != crate::command_guard::ResourcePhase::Idle)
            .then_some(crate::input_popup::InputPopupKind::Speech)
    }

    pub(crate) async fn speech_popup_snapshot(&self) -> crate::speech::SpeechPopupSnapshot {
        self.speech.popup_snapshot(self).await
    }

    pub(crate) async fn refresh_speech_input_devices(&self) {
        self.speech.refresh_input_devices(self).await;
    }

    pub(crate) async fn tutorial_is_active(&self) -> bool {
        self.tutorial.lock().await.state().tutorial_active()
    }

    pub(crate) async fn tutorial_needs_setup(&self) -> bool {
        self.tutorial.lock().await.state().needs_setup()
    }

    pub(crate) async fn tutorial_current_step(
        &self,
    ) -> Option<coosenpai_core::onboarding::TutorialStep> {
        self.tutorial.lock().await.state().current_step()
    }

    pub(crate) async fn tutorial_step_response_presented(&self) -> bool {
        self.tutorial.lock().await.step_response_presented()
    }

    pub(crate) async fn tutorial_response_presentation_is_current(
        &self,
        step: coosenpai_core::onboarding::TutorialStep,
        entry_id: &str,
    ) -> bool {
        self.tutorial
            .lock()
            .await
            .response_presentation_is_current(step, entry_id)
    }

    pub(crate) async fn onboarding_policy_phase(&self) -> crate::command_guard::OnboardingPhase {
        let tutorial = self.tutorial.lock().await;
        onboarding_policy_phase_from(
            tutorial.finish_pending(),
            tutorial.state().needs_setup(),
            tutorial.state().tutorial_active(),
            tutorial.state().current_step(),
            tutorial.chat_input_enabled(),
        )
    }

    pub(crate) fn speech_resource_phase(&self) -> crate::command_guard::ResourcePhase {
        self.speech.resource_phase()
    }

    pub async fn initialize(app: AppHandle) -> Result<Arc<Self>> {
        Self::initialize_with_clipboards(
            app,
            crate::platform::clipboard_reader(),
            crate::platform::clipboard_writer(),
            crate::platform::selected_text_copier(),
        )
        .await
    }

    pub(crate) async fn initialize_with_clipboards(
        app: AppHandle,
        clipboard_reader: Arc<dyn ClipboardReader>,
        clipboard_writer: Arc<dyn ClipboardWriter>,
        selected_text_copier: Arc<dyn SelectedTextCopyPort>,
    ) -> Result<Arc<Self>> {
        Self::initialize_with_ports(
            app,
            clipboard_reader,
            clipboard_writer,
            selected_text_copier,
            crate::platform::provider_api_key_store(),
            crate::platform::voice_output_provider(),
        )
        .await
    }

    pub(crate) async fn initialize_with_ports(
        app: AppHandle,
        clipboard_reader: Arc<dyn ClipboardReader>,
        clipboard_writer: Arc<dyn ClipboardWriter>,
        selected_text_copier: Arc<dyn SelectedTextCopyPort>,
        keychain: Arc<dyn ProviderApiKeyStore>,
        voice_output_provider: Arc<dyn coosenpai_core::voice_output::VoiceOutputProviderFactory>,
    ) -> Result<Arc<Self>> {
        let paths = crate::desktop_startup::prepare_paths(
            std::env::var_os("HOME"),
            std::env::var_os("COOSENPAI_HOME"),
        )?;
        let resource_dir = startup::resource_directory(&app)?;
        let paths = paths
            .with_builtin_personas(bundled_persona_directory(resource_dir.clone()))
            .with_builtin_tutorial(resource_dir.join("tutorial/tutorial.md"))
            .with_builtin_tutorial_en(resource_dir.join("tutorial/tutorial.en.md"));
        let logger = Arc::new(FileLogger::new(paths.log.clone())?);
        logger.write("INFO", "CooSenpAI desktop runtimeを初期化しました。")?;
        if let Err(error) = crate::avatar::cleanup_stale_files(&paths) {
            let _ = logger.write(
                "WARN",
                &format!("アバター旧ファイルの起動時 cleanup に失敗しました: {error}"),
            );
        }
        let watch_lock = WatchLock::acquire(&paths.watch_lock)
            .context("別の coosenpai watch が起動しています")?;
        let cancellation = CancellationToken::new();
        let factory = Arc::new(
            DesktopRuntimeFactory::new_with_keychain(
                paths.clone(),
                logger.clone(),
                cancellation.clone(),
                keychain,
            )
            .map_err(anyhow::Error::msg)?,
        );
        let startup = startup_context::StartupContext::load(&paths, factory.as_ref());
        startup.log_status(logger.as_ref())?;
        let runtime_active = startup.is_runtime_active();
        let startup_context::StartupContext {
            config,
            mut tutorial,
            onboarding: onboarding_view,
            conversation_generation,
            ready: startup_ready,
            error: runtime_error,
        } = startup;
        factory.work.approvals.set_mode(config.work.approval_mode);
        factory.work.set_roots(config.work.allowed_roots.clone());
        let runtime = startup::startup_runtime(
            &config,
            runtime_error.clone(),
            &onboarding_view,
            &mut tutorial,
            factory.clone(),
            logger.clone(),
            cancellation.clone(),
        )
        .await?;
        let storage = CompanionStorage::from_paths(&paths, config.retention.conversation_days);
        let conversation = storage.load_conversation().unwrap_or_default();
        let conversation_generations = match list_conversation_generations(&paths) {
            Ok(generations) => generations,
            Err(error) => {
                let _ = logger.write(
                    "WARN",
                    &format!("会話世代一覧の読込に失敗しました: {error}"),
                );
                Vec::new()
            }
        };
        let observer_calls = coosenpai_core::usage::today_observer_usage(&paths.usage)
            .map(|usage| usage.ai_calls)
            .unwrap_or(0);
        let (companion_calls, companion_limit_reached) =
            companion_usage_summary(&paths, config.companion.daily_proactive_limit);
        let permission = crate::platform::screen_capture_permission();
        let audio_permissions = permission::current_audio_permissions(logger.as_ref());
        let speech = Arc::new(crate::speech::SpeechController::new(&paths));
        let hearing = Arc::new(crate::hearing::HearingController::new(
            &paths,
            logger.clone(),
        ));
        let speech_input_devices = speech.input_devices();
        let initial_avatar =
            crate::avatar::load_with_status(&paths, config.ui.avatar_path.as_deref());
        let config_revision = config.revision;
        let (ui, start_ui_root) = crate::ui_root::channel();
        let (capture, capture_presenter) = crate::capture::channel(ui.clone());
        let (bubble_focus, _) = watch::channel(false);
        logger.write(
            "INFO",
            &format!("画面収録権限: {}", permission.presentation().status),
        )?;
        let screen_capture_gate = crate::screen_capture_gate::ScreenCaptureGate::default();
        let state = Arc::new(Self {
            work: factory.work.clone(),
            own_bounds: Arc::new(TauriOwnWindowBounds::new(app.clone())),
            conversation_sync: Arc::new(Mutex::new(())),
            screen_permission: Mutex::new(permission::ScreenPermissionCache::new(
                permission,
                Instant::now(),
            )),
            app,
            main_window_focused: AtomicBool::new(false),
            bubble_focus,
            bubbles: Arc::new(Mutex::new(BubbleState::for_conversation_generation(
                conversation_generation,
            ))),
            capture,
            ui,
            screen_capture_gate,
            clipboard_reader,
            selected_text_copier,
            clipboard_writer,
            speech,
            hearing,
            voice_output: Arc::new(crate::voice_output::VoiceOutputController::new(
                voice_output_provider,
            )),
            tutorial: Arc::new(Mutex::new(tutorial)),
            shortcut_coordinator: Arc::new(crate::capture::ShortcutCoordinator::default()),
            command_firewall: crate::command_guard::CommandFirewall::default(),
            input_active: AtomicBool::new(false),
            cancellation,
            factory,
            snapshot: Arc::new(std::sync::Mutex::new({
                let mut snapshot = AppSnapshot::initial(
                    config,
                    conversation,
                    permission,
                    audio_permissions,
                    observer_calls,
                    companion_calls,
                    signed_build(),
                );
                snapshot.companion.proactive_limit_reached = companion_limit_reached;
                snapshot.avatar_image_png = initial_avatar.image_png;
                snapshot.avatar_image_load_failed = initial_avatar.failed;
                snapshot.last_error = runtime_error;
                snapshot.onboarding = onboarding_view.clone();
                snapshot.speech.input_devices = speech_input_devices;
                snapshot.conversation_generations = conversation_generations;
                snapshot.selected_conversation_generation = conversation_generation;
                snapshot
            })),
            config_update: ConfigUpdateCoordinator::new(config_revision),
            watch_control: Mutex::new(WatchControl {
                lifecycle: WatchLifecycle::Stopped,
                generation: 0,
                resume_after_power: false,
                #[cfg(test)]
                start_commit_barrier: None,
            }),
            watch_intent_lock: Mutex::new(()),
            runtime_active: AtomicBool::new(runtime_active),
            shutting_down: AtomicBool::new(false),
            presence_startup_pending: AtomicBool::new(true),
            presence_inflight: Mutex::new(None),
            runtime,
            paths,
            logger,
            _watch_lock: watch_lock,
        });
        state.app.manage(state.ui.clone());
        start_ui_root(state.clone(), state.snapshot().await, capture_presenter);
        Self::spawn_runtime_monitor(state.clone());
        tauri::async_runtime::spawn(
            state
                .work
                .clone()
                .notify_changes(state.ui.clone(), state.cancellation.clone()),
        );
        Self::spawn_notification_monitor(state.clone());
        Self::spawn_power_monitor(state.clone());
        Self::spawn_presence_monitor(state.clone());
        if let Err(error) = state.sync_launch_at_login(state.runtime_config().app.launch_at_login) {
            let _ = state.logger.write(
                "WARN",
                &format!("ログイン時起動の同期に失敗しました: {error}"),
            );
        }
        if onboarding_view.tutorial_active
            && !onboarding_view.setup_required
            && startup_ready
            && state.tutorial.lock().await.finish_pending()
        {
            let handler_state = state.clone();
            let _ = state
                .dispatch(
                    crate::command_guard::CommandSource::TutorialAutomation,
                    crate::command_guard::DesktopCommand::TutorialFinish,
                    move |context| async move {
                        handler_state
                            .command_finish_tutorial(
                                &context,
                                tutorial_state::TutorialFinishEntry::Automatic,
                            )
                            .await
                            .map_err(crate::command_guard::DispatchError::handler)
                    },
                )
                .await;
        }
        state.refresh_debug().await;
        state.voice_output.start(&state);
        state.sync_audio();
        Ok(state)
    }

    pub async fn snapshot(&self) -> AppSnapshot {
        self.snapshot.lock().expect("snapshot lock").clone()
    }

    pub(crate) async fn refresh_avatar_image(&self) {
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::AvatarRefresh)
            .await;
    }

    pub(crate) fn sync_launch_at_login(&self, enabled: bool) -> Result<(), String> {
        let manager = self
            .app
            .try_state::<tauri_plugin_autostart::AutoLaunchManager>()
            .ok_or_else(|| "autostart plugin が初期化されていません".to_owned())?;
        if enabled {
            manager.enable()
        } else {
            manager.disable()
        }
        .map_err(|error| error.to_string())
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub(crate) fn is_runtime_active(&self) -> bool {
        self.runtime_active.load(Ordering::Acquire)
    }

    pub(crate) async fn publish_event(
        &self,
        event: crate::snapshot_presenter::SnapshotEvent,
    ) -> AppSnapshot {
        let input = crate::snapshot_presenter::SnapshotInput {
            event,
            config_revision: self.config_update.current_revision(),
            work: coosenpai_core::work::WorkConfig {
                approval_mode: self.work.approvals.mode(),
                allowed_roots: self.work.roots(),
            },
        };
        if let Err(error) = self
            .ui
            .request(
                crate::ui_events::UiView::Application,
                crate::ui_events::UiEvent::SnapshotResult(Box::new(input)),
            )
            .await
        {
            let _ = self.logger.write("WARN", &error);
        }
        self.snapshot().await
    }

    pub async fn refresh_conversation(&self) {
        let _conversation_sync = self.conversation_sync.lock().await;
        let config = self.runtime.config();
        let storage = CompanionStorage::from_paths(&self.paths, config.retention.conversation_days);
        let (calls, limit_reached) =
            companion_usage_summary(&self.paths, config.companion.daily_proactive_limit);
        let conversation = storage.load_conversation();
        let generations = list_conversation_generations(&self.paths);
        let selected_generation = storage.conversation_generation();
        if let (Ok(conversation), Ok(generations), Ok(selected_generation)) =
            (conversation, generations, selected_generation)
        {
            let snapshot = self.snapshot().await;
            if snapshot.conversation != conversation
                || snapshot.conversation_generations != generations
                || snapshot.selected_conversation_generation != selected_generation
                || snapshot.companion.total_calls_today != calls
                || snapshot.companion.proactive_limit_reached != limit_reached
            {
                self.publish_event(
                    crate::snapshot_presenter::SnapshotEvent::ConversationLoaded {
                        conversation,
                        generations,
                        selected_generation,
                        calls,
                        limit_reached,
                    },
                )
                .await;
            }
        }
        self.refresh_debug().await;
    }

    pub async fn refresh_debug(&self) {
        let catalog = if self.runtime.config().debug.enabled {
            match coosenpai_core::debug::DebugStore::from_paths(&self.paths).load_catalog() {
                Ok(value) => value,
                Err(_) => {
                    let _ = self.logger.write(
                        "WARN",
                        "デバッグ詳細の読込に失敗しました: error-type=debug-persistence",
                    );
                    coosenpai_core::debug::DebugCatalog::default()
                }
            }
        } else {
            coosenpai_core::debug::DebugCatalog::default()
        };
        if self.snapshot().await.debug_catalog != catalog {
            self.publish_event(crate::snapshot_presenter::SnapshotEvent::DebugLoaded(
                catalog,
            ))
            .await;
        }
    }

    pub(crate) async fn finish_capture_cleanup(&self) {
        self.cancellation.cancel();
        self.capture.shutdown().await;
    }

    pub async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancellation.cancel();
        self.capture.shutdown().await;
        self.work.shutdown().await;
        self.voice_output.stop().await;
        self.cancel_audio_and_wait().await;
        let _ = self.stop_watch_internal_and_wait(true).await;
        let cleanup = self.runtime.shutdown();
        if tokio::time::timeout(Duration::from_secs(10), cleanup)
            .await
            .is_err()
        {
            coosenpai_core::process::force_kill_provider_processes();
        }
        coosenpai_core::process::force_kill_provider_processes();
        let _ = self
            .logger
            .write("INFO", "CooSenpAI desktop runtimeを停止しました。");
    }
}

fn companion_usage_summary(paths: &ConfigPaths, proactive_limit: Option<u32>) -> (u32, bool) {
    coosenpai_core::usage::load_companion(
        &paths.companion_usage,
        &coosenpai_core::config::local_date(),
    )
    .map(|usage| {
        (
            usage.total_calls,
            coosenpai_core::usage::is_proactive_limit_reached(
                usage.proactive_calls,
                proactive_limit,
            ),
        )
    })
    .unwrap_or((0, false))
}

fn onboarding_policy_phase_from(
    finish_pending: bool,
    needs_setup: bool,
    tutorial_active: bool,
    current_step: Option<coosenpai_core::onboarding::TutorialStep>,
    chat_input_enabled: bool,
) -> crate::command_guard::OnboardingPhase {
    if finish_pending {
        crate::command_guard::OnboardingPhase::TutorialFinishing
    } else if needs_setup {
        crate::command_guard::OnboardingPhase::Setup
    } else if tutorial_active {
        current_step
            .map(|step| crate::command_guard::OnboardingPhase::Tutorial {
                step,
                chat_input_enabled,
            })
            .unwrap_or(crate::command_guard::OnboardingPhase::TutorialFinishing)
    } else {
        crate::command_guard::OnboardingPhase::Normal
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum NotificationTarget {
    Bubble,
    Os,
}

impl DesktopState {
    pub(crate) async fn voice_conversation_generation(&self) -> u64 {
        self.bubbles.lock().await.conversation_generation()
    }
}

pub fn signed_build() -> bool {
    option_env!("APPLE_SIGNING_IDENTITY")
        == Some("Developer ID Application: Masanobu Naruse (MUFAV5XYJD)")
}

#[path = "state_tutorial_progress_effects.rs"]
mod tutorial_progress_effects;

#[path = "state_tutorial_response_effects.rs"]
mod tutorial_response_effects;

#[path = "state_tutorial_lifecycle_effects.rs"]
mod tutorial_lifecycle_effects;
