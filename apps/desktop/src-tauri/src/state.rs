use crate::bubbles::{self, BubbleRecord, BubbleState, BubbleWindowSyncState};
use crate::capture::CapturePopupState;
pub(crate) use crate::config_update::{
    ConfigCommitError, ConfigUpdateCoordinator, ConfigUpdateTransaction,
};
use crate::factory::{bundled_persona_directory, DesktopRuntimeFactory};
use crate::own_bounds::TauriOwnWindowBounds;
use crate::snapshot::{AppSnapshot, SnapshotEvent};
use anyhow::{Context, Result};
use coosenpai_core::companion_storage::CompanionStorage;
use coosenpai_core::config::{ensure_layout, load_config};
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::logging::FileLogger;
use coosenpai_core::notification::NotificationConsumer;
use coosenpai_core::onboarding::OnboardingStore;
use coosenpai_core::persistence::WatchLock;
use coosenpai_core::ports::{
    ClipboardReader, ClipboardWriter, NotificationPort, ProviderApiKeyStore, RuntimeLogger,
    SelectedTextCopyPort,
};
use coosenpai_core::runtime::{
    RuntimeError, RuntimeErrorKind, RuntimeHandle, RuntimeLastError, RuntimeSnapshot,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;

const THOUGHT_BUBBLE_COOLDOWN: Duration = Duration::from_millis(1_500);

#[derive(Debug, Default)]
struct ThoughtBubbleScheduler {
    last_presented_at: Option<Instant>,
    pending: Option<String>,
    flush_scheduled: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum ThoughtBubbleAction {
    Show(String),
    Wait(Duration),
    None,
}

impl ThoughtBubbleScheduler {
    fn queue(&mut self, now: Instant, thought: String) -> ThoughtBubbleAction {
        self.pending = Some(thought);
        let Some(last_presented_at) = self.last_presented_at else {
            self.last_presented_at = Some(now);
            return ThoughtBubbleAction::Show(self.pending.take().expect("thought is queued"));
        };
        let elapsed = now.saturating_duration_since(last_presented_at);
        if elapsed >= THOUGHT_BUBBLE_COOLDOWN && !self.flush_scheduled {
            self.last_presented_at = Some(now);
            return ThoughtBubbleAction::Show(self.pending.take().expect("thought is queued"));
        }
        if self.flush_scheduled {
            ThoughtBubbleAction::None
        } else {
            self.flush_scheduled = true;
            ThoughtBubbleAction::Wait(THOUGHT_BUBBLE_COOLDOWN.saturating_sub(elapsed))
        }
    }

    fn flush(&mut self, now: Instant) -> ThoughtBubbleAction {
        let Some(last_presented_at) = self.last_presented_at else {
            self.flush_scheduled = false;
            return ThoughtBubbleAction::None;
        };
        let elapsed = now.saturating_duration_since(last_presented_at);
        if elapsed < THOUGHT_BUBBLE_COOLDOWN {
            return ThoughtBubbleAction::Wait(THOUGHT_BUBBLE_COOLDOWN - elapsed);
        }
        self.flush_scheduled = false;
        self.last_presented_at = Some(now);
        self.pending
            .take()
            .map_or(ThoughtBubbleAction::None, ThoughtBubbleAction::Show)
    }

    fn clear_pending(&mut self) {
        self.pending = None;
    }
}

#[path = "state_clipboard.rs"]
mod clipboard;
pub(crate) use clipboard::dispatch_copy_last_reply_shortcut;
#[path = "state_audio.rs"]
mod audio;
#[path = "state_command_api.rs"]
mod command_api;
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
    pub app: AppHandle,
    pub(crate) main_window_visible: AtomicBool,
    pub(crate) main_window_focused: AtomicBool,
    capture_popup_focus: watch::Sender<bool>,
    bubble_focus: watch::Sender<bool>,
    pub bubbles: Mutex<BubbleState>,
    pub(crate) bubble_window_sync: Mutex<BubbleWindowSyncState>,
    thought_bubble: Mutex<ThoughtBubbleScheduler>,
    capture_popup: Mutex<CapturePopupState>,
    text_capture_serial: Mutex<()>,
    pub(crate) input_popup_gate: Mutex<()>,
    pub clipboard_reader: Arc<dyn coosenpai_core::ports::ClipboardReader>,
    pub selected_text_copier: Arc<dyn SelectedTextCopyPort>,
    pub(crate) clipboard_writer: Arc<dyn coosenpai_core::ports::ClipboardWriter>,
    speech: Arc<crate::speech::SpeechController>,
    hearing: Arc<crate::hearing::HearingController>,
    tutorial: Mutex<crate::tutorial::TutorialController>,
    tutorial_sequence: Mutex<tutorial_sequence::TutorialSequenceControl>,
    pub shortcut_coordinator: crate::capture::ShortcutCoordinator,
    pub(crate) command_firewall: crate::command_guard::CommandFirewall,
    pub input_active: AtomicBool,
    pub cancellation: CancellationToken,
    runtime: RuntimeHandle,
    pub factory: Arc<DesktopRuntimeFactory>,
    pub paths: ConfigPaths,
    pub logger: Arc<FileLogger>,
    pub own_bounds: Arc<TauriOwnWindowBounds>,
    conversation_sync: Mutex<()>,
    screen_permission: Mutex<coosenpai_core::ports::ScreenCapturePermission>,
    #[cfg(test)]
    screen_permission_override: Mutex<Option<coosenpai_core::ports::ScreenCapturePermission>>,
    snapshot: Mutex<AppSnapshot>,
    pub(crate) config_update: ConfigUpdateCoordinator,
    bubble_delivery_log_state: Mutex<BubbleDeliveryLogState>,
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

    pub(crate) async fn capture_popup_read(
        &self,
    ) -> tokio::sync::MutexGuard<'_, CapturePopupState> {
        self.capture_popup.lock().await
    }

    pub(crate) async fn capture_popup_for_command(
        &self,
        _permit: &crate::command_guard::CommandContext,
    ) -> tokio::sync::MutexGuard<'_, CapturePopupState> {
        self.capture_popup.lock().await
    }

    pub(crate) async fn capture_popup_for_event(
        &self,
        generation: crate::command_guard::GenerationStamp,
    ) -> Result<tokio::sync::MutexGuard<'_, CapturePopupState>, crate::command_guard::DispatchError>
    {
        self.ensure_command_generation(generation)?;
        let capture = self.capture_popup.lock().await;
        self.ensure_command_generation(generation)?;
        Ok(capture)
    }

    pub(crate) async fn text_capture_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.text_capture_serial.lock().await
    }

    pub(crate) fn capture_popup_focus_events(&self) -> watch::Receiver<bool> {
        self.capture_popup_focus.subscribe()
    }

    pub(crate) fn set_capture_popup_focused(&self, focused: bool) {
        self.capture_popup_focus.send_replace(focused);
    }

    pub(crate) fn bubble_focus_events(&self) -> watch::Receiver<bool> {
        self.bubble_focus.subscribe()
    }

    pub(crate) fn set_bubble_focused(&self, focused: bool) {
        self.bubble_focus.send_replace(focused);
    }

    pub(crate) async fn input_popup_kind(&self) -> Option<crate::input_popup::InputPopupKind> {
        let capture_kind = self.capture_popup_read().await.kind();
        if let Some(kind) = capture_kind {
            return Some(match kind {
                crate::capture::CaptureKind::Image => {
                    crate::input_popup::InputPopupKind::CaptureImage
                }
                crate::capture::CaptureKind::Text => {
                    crate::input_popup::InputPopupKind::CaptureText
                }
            });
        }
        (self.speech_resource_phase() != crate::command_guard::ResourcePhase::Idle)
            .then_some(crate::input_popup::InputPopupKind::Speech)
    }

    pub(crate) fn speech_is_recording(&self) -> bool {
        self.speech.is_recording()
    }

    pub(crate) fn speech_confirming_generation(&self) -> Option<u64> {
        self.speech.confirming_generation()
    }

    pub(crate) fn speech_accepts_transient_shortcut_error(&self, generation: u64) -> bool {
        self.speech.accepts_transient_shortcut_error(generation)
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

    pub(crate) async fn cancel_speech_and_wait(&self) {
        self.speech.cancel_and_wait(self).await;
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
        Self::initialize_with_clipboards_and_keychain(
            app,
            clipboard_reader,
            clipboard_writer,
            selected_text_copier,
            crate::platform::provider_api_key_store(),
        )
        .await
    }

    pub(crate) async fn initialize_with_clipboards_and_keychain(
        app: AppHandle,
        clipboard_reader: Arc<dyn ClipboardReader>,
        clipboard_writer: Arc<dyn ClipboardWriter>,
        selected_text_copier: Arc<dyn SelectedTextCopyPort>,
        keychain: Arc<dyn ProviderApiKeyStore>,
    ) -> Result<Arc<Self>> {
        let user_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("ホームディレクトリを取得できません")?;
        let resource_dir = startup::resource_directory(&app)?;
        let paths = ConfigPaths::for_home(&user_home)
            .with_builtin_personas(bundled_persona_directory(resource_dir.clone()))
            .with_builtin_tutorial(resource_dir.join("tutorial/tutorial.md"));
        ensure_layout(&paths)?;
        let (config, startup_config_error) = startup::startup_config(load_config(&paths));
        let logger = Arc::new(FileLogger::new(paths.log.clone())?);
        logger.write("INFO", "CooSenpAI desktop runtimeを初期化しました。")?;
        if let Err(error) = crate::avatar::cleanup_stale_backups(&paths) {
            let _ = logger.write(
                "WARN",
                &format!("アバター旧ファイルの起動時 cleanup に失敗しました: {error}"),
            );
        }
        let (mut tutorial, onboarding_persistence_error) =
            startup::startup_tutorial(OnboardingStore::new(paths.onboarding.clone()));
        let onboarding_view = crate::snapshot::OnboardingView::from_state(tutorial.state());
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
        let setup_provider_error = if onboarding_view.setup_required {
            match factory.tutorial_provider(tutorial_state::tutorial_placeholders(&config)) {
                Ok(provider) => {
                    tutorial.attach_setup_provider(provider);
                    None
                }
                Err(error) => Some(startup::factory_runtime_error(error.issue)),
            }
        } else {
            None
        };
        let (current_conversation_generation, generation_error) =
            startup::conversation_generation(&paths);
        let should_initialize_conversation = startup::should_initialize_conversation_on_startup(
            startup_config_error.is_none(),
            onboarding_persistence_error.is_none(),
            generation_error.is_none(),
            onboarding_view.setup_required,
            onboarding_view.tutorial_active,
        );
        let (conversation_generation, startup_conversation_error) =
            startup::initialize_conversation_before_runtime(
                &paths,
                &config,
                current_conversation_generation,
                generation_error.is_some(),
                should_initialize_conversation,
            );
        let onboarding_error = startup::onboarding_runtime_error(tutorial.state());
        let startup_ready = startup_config_error.is_none()
            && generation_error.is_none()
            && onboarding_persistence_error.is_none()
            && startup_conversation_error.is_none();
        let runtime_error = startup_config_error
            .clone()
            .or(generation_error)
            .or(startup_conversation_error)
            .or(onboarding_persistence_error)
            .or(setup_provider_error)
            .or(onboarding_error);
        let runtime_active = runtime_error.is_none();
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
        let observer_calls = coosenpai_core::usage::today_observer_usage(&paths.usage)
            .map(|usage| usage.ai_calls)
            .unwrap_or(0);
        let companion_calls = coosenpai_core::usage::load_companion(
            &paths.companion_usage,
            &coosenpai_core::config::local_date(),
        )
        .map(|usage| usage.total_calls)
        .unwrap_or(0);
        let permission = crate::platform::screen_capture_permission();
        let speech_permissions = permission::current_speech_permissions(logger.as_ref());
        let speech = Arc::new(crate::speech::SpeechController::new(&paths));
        let hearing = Arc::new(crate::hearing::HearingController::new(
            &paths,
            logger.clone(),
        ));
        let speech_input_devices = speech.input_devices();
        let initial_avatar =
            crate::avatar::load_with_status(&paths, config.ui.avatar_path.as_deref());
        let (capture_popup_focus, _) = watch::channel(false);
        let (bubble_focus, _) = watch::channel(false);
        logger.write(
            "INFO",
            &format!("画面収録権限: {}", permission.presentation().status),
        )?;
        let state = Arc::new(Self {
            own_bounds: Arc::new(TauriOwnWindowBounds::new(app.clone())),
            conversation_sync: Mutex::new(()),
            screen_permission: Mutex::new(permission),
            #[cfg(test)]
            screen_permission_override: Mutex::new(None),
            app,
            main_window_visible: AtomicBool::new(false),
            main_window_focused: AtomicBool::new(false),
            capture_popup_focus,
            bubble_focus,
            bubbles: Mutex::new(BubbleState::for_conversation_generation(
                conversation_generation,
            )),
            bubble_window_sync: Mutex::new(BubbleWindowSyncState::default()),
            thought_bubble: Mutex::new(ThoughtBubbleScheduler::default()),
            capture_popup: Mutex::new(CapturePopupState::default()),
            text_capture_serial: Mutex::new(()),
            input_popup_gate: Mutex::new(()),
            clipboard_reader,
            selected_text_copier,
            clipboard_writer,
            speech,
            hearing,
            tutorial: Mutex::new(tutorial),
            tutorial_sequence: Mutex::new(tutorial_sequence::TutorialSequenceControl::default()),
            shortcut_coordinator: crate::capture::ShortcutCoordinator::default(),
            command_firewall: crate::command_guard::CommandFirewall::default(),
            input_active: AtomicBool::new(false),
            cancellation,
            factory,
            snapshot: Mutex::new({
                let mut snapshot = AppSnapshot::initial(
                    config,
                    conversation,
                    permission,
                    speech_permissions,
                    observer_calls,
                    companion_calls,
                    signed_build(),
                );
                snapshot.avatar_image_png = initial_avatar.image_png;
                snapshot.avatar_image_load_failed = initial_avatar.failed;
                snapshot.last_error = runtime_error;
                snapshot.onboarding = onboarding_view.clone();
                snapshot.speech.input_devices = speech_input_devices;
                snapshot
            }),
            config_update: ConfigUpdateCoordinator::default(),
            bubble_delivery_log_state: Mutex::new(BubbleDeliveryLogState::default()),
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
        state.own_bounds.request_refresh()?;
        Self::spawn_own_bounds_monitor(state.clone());
        Self::spawn_runtime_monitor(state.clone());
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
        state.sync_audio();
        Ok(state)
    }

    pub async fn snapshot(&self) -> AppSnapshot {
        self.snapshot.lock().await.clone()
    }

    pub(crate) async fn refresh_avatar_image(&self) {
        let avatar = crate::avatar::load_with_status(
            &self.paths,
            self.runtime_config().ui.avatar_path.as_deref(),
        );
        self.publish(|snapshot| apply_avatar_load_result(snapshot, avatar))
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

    pub async fn publish<F>(&self, update: F) -> AppSnapshot
    where
        F: FnOnce(&mut AppSnapshot),
    {
        let mut snapshot = self.snapshot.lock().await;
        let avatar_path = snapshot.config.ui.avatar_path.clone();
        update(&mut snapshot);
        if snapshot.config.ui.avatar_path != avatar_path {
            refresh_avatar_snapshot(&mut snapshot, &self.paths);
        }
        snapshot.config_revision = self.config_update.current_revision();
        snapshot.revision = snapshot.revision.saturating_add(1);
        let result = snapshot.clone();
        let _ = self.app.emit(
            "coosenpai:snapshot:updated",
            SnapshotEvent {
                revision: result.revision,
                snapshot: result.clone(),
            },
        );
        result
    }

    pub async fn refresh_conversation(&self) {
        let _conversation_sync = self.conversation_sync.lock().await;
        let config = self.runtime.config();
        let storage = CompanionStorage::from_paths(&self.paths, config.retention.conversation_days);
        if let Ok(conversation) = storage.load_conversation() {
            let calls = coosenpai_core::usage::load_companion(
                &self.paths.companion_usage,
                &coosenpai_core::config::local_date(),
            )
            .map(|usage| usage.total_calls)
            .unwrap_or(0);
            self.publish(|snapshot| {
                snapshot.conversation = conversation;
                snapshot.companion.total_calls_today = calls;
            })
            .await;
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
        self.publish(|snapshot| snapshot.debug_catalog = catalog)
            .await;
    }

    pub async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancellation.cancel();
        self.speech.cancel_and_wait(self).await;
        self.cancel_audio_and_wait().await;
        let _ = self.stop_watch_internal(true).await;
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

fn refresh_avatar_snapshot(snapshot: &mut AppSnapshot, paths: &ConfigPaths) {
    let avatar = crate::avatar::load_with_status(paths, snapshot.config.ui.avatar_path.as_deref());
    apply_avatar_load_result(snapshot, avatar);
}

fn apply_avatar_load_result(snapshot: &mut AppSnapshot, avatar: crate::avatar::AvatarLoadResult) {
    snapshot.avatar_image_png = avatar.image_png;
    snapshot.avatar_image_load_failed = avatar.failed;
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

#[derive(Clone, Copy)]
enum NotificationTarget {
    Bubble,
    Os,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BubbleDeliveryDecision {
    Show,
    SuppressRead,
    SuppressUnread,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BubbleDeliveryLogKey {
    decision: BubbleDeliveryDecision,
    main_focused: bool,
    input_active: bool,
}

#[derive(Default)]
struct BubbleDeliveryLogState {
    last_by_notification: HashMap<String, BubbleDeliveryLogKey>,
}

impl BubbleDeliveryLogState {
    fn should_log(&mut self, notification_id: &str, key: BubbleDeliveryLogKey) -> bool {
        if self.last_by_notification.get(notification_id).copied() == Some(key) {
            return false;
        }
        self.last_by_notification
            .insert(notification_id.to_owned(), key);
        true
    }

    fn clear(&mut self, notification_id: &str) {
        self.last_by_notification.remove(notification_id);
    }
}

impl BubbleDeliveryDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Show => "show",
            Self::SuppressRead => "suppress-read",
            Self::SuppressUnread => "suppress-unread",
        }
    }
}

fn bubble_delivery_log(
    message_kind: &str,
    decision: BubbleDeliveryDecision,
    main_focused: bool,
    input_active: bool,
) -> String {
    format!(
        "吹き出し配達判定: kind={message_kind} decision={} main-focused={main_focused} input-active={input_active}",
        decision.as_str()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BubblePresentationStyle {
    tutorial: bool,
    persistent: bool,
}

fn bubble_presentation_style(
    message_kind: &str,
    tutorial_active: bool,
    keep_latest: bool,
) -> BubblePresentationStyle {
    if tutorial_active && message_kind == "chat" {
        BubblePresentationStyle {
            tutorial: true,
            persistent: true,
        }
    } else {
        BubblePresentationStyle {
            tutorial: false,
            persistent: keep_latest,
        }
    }
}

fn bubble_delivery_decision(
    message_kind: &str,
    input_active: bool,
    main_focused: bool,
) -> BubbleDeliveryDecision {
    if message_kind != "chat" && input_active {
        BubbleDeliveryDecision::SuppressUnread
    } else if main_focused {
        BubbleDeliveryDecision::SuppressRead
    } else {
        BubbleDeliveryDecision::Show
    }
}

fn should_show_thought_bubble(enabled: bool, input_active: bool, main_focused: bool) -> bool {
    enabled && !input_active && !main_focused
}

pub fn signed_build() -> bool {
    option_env!("APPLE_SIGNING_IDENTITY")
        == Some("Developer ID Application: Masanobu Naruse (MUFAV5XYJD)")
}

