use coosenpai_core::config::Config;
use coosenpai_core::debug::DebugCatalog;
use coosenpai_core::locale::{
    localize_audio_message, localize_capture_disposition, localize_error_message,
    localize_screen_permission_message, localize_shortcut_message, localize_speech_message, Locale,
    TextKey,
};
use coosenpai_core::memory::MemoryStatus;
use coosenpai_core::runtime::{RuntimeLastError, RuntimeSnapshot};
use coosenpai_core::state::{
    AudioObservation, AudioObservationSource, ConversationEntry, ObservationRecord,
    VisualObservation,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub revision: u64,
    pub config_revision: u64,
    pub config: Config,
    pub observer_running: bool,
    pub watch_intent_active: bool,
    pub observer: ObserverView,
    pub companion: CompanionView,
    pub notify: NotificationView,
    pub observer_provider_label: String,
    pub companion_provider_label: String,
    pub companion_display_name: String,
    #[serde(default)]
    pub companion_emotions: coosenpai_core::emotion::EmotionState,
    #[serde(skip)]
    companion_emotions_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_assertiveness:
        Option<coosenpai_core::companion_assertiveness::TemporaryAssertivenessSelection>,
    pub conversation: Vec<ConversationEntry>,
    pub conversation_generations:
        Vec<coosenpai_core::conversation_archive::ConversationGenerationSummary>,
    pub selected_conversation_generation: u64,
    pub unread_count: usize,
    pub screen_recording_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_recording_message: Option<String>,
    pub screen_recording_restart_required: bool,
    pub signed_build: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<RuntimeLastError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub companion_retry_in_seconds: Option<u64>,
    pub pending_deliveries: usize,
    pub delivery_outbox_blocked: bool,
    pub memory_status: MemoryStatus,
    pub debug_catalog: DebugCatalog,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_shortcut_error: Option<String>,
    #[serde(skip)]
    pub(crate) capture_shortcut_error_id: u64,
    #[serde(skip)]
    pub(crate) capture_shortcut_error_speech_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_user_message_id: Option<String>,
    pub user_work_pending: bool,
    pub cancelled_user_message_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub companion_draft: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_companion_thought: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_companion_decision: Option<coosenpai_core::runtime::CompanionDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_user_interruption: Option<coosenpai_core::runtime::UserInterruption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_image_png: Option<Vec<u8>>,
    pub avatar_image_load_failed: bool,
    pub provider_usage: coosenpai_core::provider::ProviderUsage,
    pub speech: SpeechView,
    pub audio: AudioView,
    pub onboarding: OnboardingView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingView {
    pub setup_required: bool,
    pub tutorial_active: bool,
    pub finish_pending: bool,
    pub resume_pending: bool,
    pub chat_input_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_highlight: Option<String>,
    #[serde(default)]
    pub highlighted_settings: Vec<String>,
}

impl OnboardingView {
    pub fn from_state(state: &coosenpai_core::onboarding::OnboardingState) -> Self {
        Self::from_state_and_resume(state, state.tutorial_active())
    }

    pub fn from_state_and_resume(
        state: &coosenpai_core::onboarding::OnboardingState,
        resume_pending: bool,
    ) -> Self {
        let setup_required = state.needs_setup();
        let tutorial_active = !setup_required && state.tutorial_active();
        Self {
            setup_required,
            tutorial_active,
            finish_pending: tutorial_active && state.tutorial_finish_pending(),
            resume_pending: tutorial_active && !state.tutorial_finish_pending() && resume_pending,
            chat_input_enabled: false,
            current_step: tutorial_active
                .then(|| state.current_step())
                .flatten()
                .map(|step| step.id().to_owned()),
            skip_hint: None,
            settings_highlight: None,
            highlighted_settings: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeechView {
    pub generation: u64,
    pub phase: String,
    pub partial: String,
    pub microphone_permission: String,
    pub recognition_permission: String,
    pub input_devices: Vec<coosenpai_core::ports::SpeechInputDevice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioView {
    pub generation: u64,
    pub phase: String,
    pub microphone_permission: String,
    pub recognition_permission: String,
    pub screen_capture_permission: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_observation: Option<AudioObservationView>,
    #[serde(default)]
    pub recent_events: Vec<AudioLogEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "stage", rename_all = "kebab-case")]
pub enum AudioLogStage {
    Recognizing,
    NoSpeech,
    Confirmed { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioLogEvent {
    pub id: String,
    pub created_at: String,
    pub source: AudioObservationSource,
    #[serde(flatten)]
    pub stage: AudioLogStage,
}

impl AudioView {
    pub fn push_event(&mut self, event: AudioLogEvent) {
        const RECENT_AUDIO_EVENT_LIMIT: usize = 100;
        if self
            .recent_events
            .iter()
            .any(|existing| existing.id == event.id)
        {
            return;
        }
        self.recent_events.push(event);
        if self.recent_events.len() > RECENT_AUDIO_EVENT_LIMIT {
            self.recent_events.remove(0);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioObservationView {
    pub id: String,
    pub created_at: String,
    pub source: String,
    pub text: String,
}

impl AudioObservationView {
    pub fn from_observation(observation: &AudioObservation) -> Self {
        Self {
            id: observation.id.clone(),
            created_at: observation.created_at.clone(),
            source: match observation.source {
                AudioObservationSource::Microphone => "microphone",
                AudioObservationSource::Speaker => "speaker",
            }
            .to_owned(),
            text: observation.text.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObserverView {
    pub phase: ObserverViewPhase,
    pub ai_calls_today: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_captured_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_trigger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_capture_disposition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub front_app: Option<String>,
    pub pending_frame_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_send_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observation: Option<ObservationRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_visual_observation: Option<VisualObservation>,
    pub ocr_gate_enabled: bool,
    pub activity_signals_enabled: bool,
    pub battery_multiplier: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub targets: Vec<WatchTargetView>,
}

impl ObserverView {
    pub(crate) fn record_error(&mut self, message: String) {
        self.phase = ObserverViewPhase::Error;
        self.pending_frame_count = 0;
        self.next_send_at = None;
        self.error_message = Some(message);
    }

    pub(crate) fn record_recoverable_error(&mut self, message: String) {
        self.phase = ObserverViewPhase::Idle;
        self.error_message = Some(message);
    }

    pub(crate) fn clear_error(&mut self) {
        self.error_message = None;
    }

    pub(crate) fn record_observation(&mut self, observation: ObservationRecord) {
        if let ObservationRecord::Visual(value) = &observation {
            self.last_visual_observation = Some(value.clone());
        }
        self.last_observation = Some(observation);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchTargetView {
    pub target: String,
    pub name: String,
    pub enabled: bool,
    pub foreground: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_captured_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_trigger: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ObserverViewPhase {
    Stopped,
    Idle,
    Capturing,
    Thinking,
    Suspended,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionView {
    pub phase: CompanionViewPhase,
    pub ready: bool,
    pub total_calls_today: u32,
    #[serde(default)]
    pub proactive_limit_reached: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CompanionViewPhase {
    Idle,
    Thinking,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationView {
    pub mode: String,
    pub minimum_priority: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEvent {
    pub revision: u64,
    pub snapshot: AppSnapshot,
}

impl AppSnapshot {
    pub(crate) fn finish_watch(&mut self, failed: bool) {
        self.observer_running = false;
        self.watch_intent_active = false;
        self.observer.phase = if failed {
            ObserverViewPhase::Error
        } else {
            ObserverViewPhase::Stopped
        };
        self.observer.pending_frame_count = 0;
        self.observer.next_send_at = None;
    }

    pub fn initial(
        config: Config,
        conversation: Vec<ConversationEntry>,
        permission: coosenpai_core::ports::ScreenCapturePermission,
        speech_permissions: coosenpai_core::ports::SpeechPermissions,
        observer_calls: u32,
        companion_calls: u32,
        signed_build: bool,
    ) -> Self {
        let config_revision = config.revision;
        let locale = coosenpai_core::locale::Locale::from_config(&config.ui.language);
        let permission_presentation = permission.presentation_for_locale(locale);
        let memory_status = MemoryStatus {
            enabled: config.memory.enabled,
            provider_consent: config.memory.provider_consent,
            ..MemoryStatus::default()
        };
        Self {
            revision: 1,
            config_revision,
            observer_running: false,
            watch_intent_active: false,
            observer: ObserverView {
                phase: ObserverViewPhase::Stopped,
                ai_calls_today: observer_calls,
                last_captured_at: None,
                last_trigger: None,
                last_capture_disposition: None,
                front_app: None,
                pending_frame_count: 0,
                next_send_at: None,
                last_observation: None,
                last_visual_observation: None,
                ocr_gate_enabled: config.watch.ocr_gate.enabled,
                activity_signals_enabled: true,
                battery_multiplier: 1.0,
                error_message: None,
                targets: target_views(&config),
            },
            companion: CompanionView {
                phase: CompanionViewPhase::Idle,
                ready: true,
                total_calls_today: companion_calls,
                proactive_limit_reached: false,
            },
            notify: NotificationView {
                mode: config.notification.mode.clone(),
                minimum_priority: config.notification.min_priority.clone(),
            },
            observer_provider_label: provider_label(&config.observer.provider),
            companion_provider_label: provider_label(&config.companion.provider),
            companion_display_name: config.companion.display_name.clone(),
            temporary_assertiveness: None,
            config,
            conversation,
            conversation_generations: Vec::new(),
            selected_conversation_generation: 0,
            unread_count: 0,
            screen_recording_status: permission_presentation.status.to_owned(),
            screen_recording_message: permission_presentation.message.map(str::to_owned),
            screen_recording_restart_required: permission.requires_restart(),
            signed_build,
            last_error: None,
            companion_retry_in_seconds: None,
            pending_deliveries: 0,
            delivery_outbox_blocked: false,
            memory_status,
            debug_catalog: DebugCatalog::default(),
            capture_shortcut_error: None,
            capture_shortcut_error_id: 0,
            capture_shortcut_error_speech_generation: None,
            active_user_message_id: None,
            user_work_pending: false,
            cancelled_user_message_ids: Vec::new(),
            companion_draft: None,
            latest_companion_thought: None,
            latest_companion_decision: None,
            latest_user_interruption: None,
            companion_emotions: Default::default(),
            companion_emotions_revision: 0,
            avatar_image_png: None,
            avatar_image_load_failed: false,
            provider_usage: Default::default(),
            speech: SpeechView {
                generation: 0,
                phase: "idle".to_owned(),
                partial: String::new(),
                microphone_permission: speech_permission_name(speech_permissions.microphone),
                recognition_permission: speech_permission_name(speech_permissions.recognition),
                input_devices: Vec::new(),
                warning_kind: None,
                message: None,
                source: None,
            },
            audio: AudioView {
                generation: 0,
                phase: "off".to_owned(),
                microphone_permission: speech_permission_name(speech_permissions.microphone),
                recognition_permission: speech_permission_name(speech_permissions.recognition),
                screen_capture_permission:
                    if crate::platform::speaker_requires_screen_recording() {
                        permission_presentation.status
                    } else {
                        "not-required"
                    }
                    .to_owned(),
                warning_kind: None,
                message: None,
                latest_observation: None,
                recent_events: Vec::new(),
            },
            onboarding: OnboardingView::from_state(
                &coosenpai_core::onboarding::OnboardingState::default(),
            ),
        }
    }

    pub fn apply_runtime(&mut self, runtime: &RuntimeSnapshot) {
        let startup_config_error = self
            .last_error
            .as_ref()
            .filter(|error| error.kind == coosenpai_core::runtime::RuntimeErrorKind::Config)
            .cloned();
        self.last_error = startup_config_error.or_else(|| runtime.last_error.clone());
        self.companion_retry_in_seconds = runtime.companion_retry_in_seconds;
        self.pending_deliveries = runtime.pending_deliveries;
        self.delivery_outbox_blocked = runtime.delivery_outbox_blocked;
        self.memory_status = runtime.memory_status.clone();
        self.companion_display_name = runtime.companion_display_name.clone();
        self.companion.proactive_limit_reached = runtime.proactive_limit_reached;
        self.active_user_message_id = runtime.active_user_message_id.clone();
        self.user_work_pending = runtime.user_work_pending;
        self.cancelled_user_message_ids = runtime.cancelled_user_message_ids.clone();
        self.companion_draft = runtime.companion_draft.clone();
        self.latest_companion_thought = runtime.latest_companion_thought.clone();
        self.latest_companion_decision = runtime.latest_companion_decision.clone();
        self.latest_user_interruption = runtime.latest_user_interruption.clone();
        // リセット応答と監視通知の到着順が逆でも、古い感情へ戻さない。
        if runtime.revision >= self.companion_emotions_revision {
            self.companion_emotions = runtime.companion_emotions;
            self.companion_emotions_revision = runtime.revision;
        }
        self.provider_usage = runtime.provider_usage.clone();
        self.companion.ready = self.last_error.is_none();
        self.companion.phase = if self.last_error.is_some() {
            CompanionViewPhase::Error
        } else if matches!(
            runtime.phase,
            coosenpai_core::runtime::RuntimePhase::Companion
        ) {
            CompanionViewPhase::Thinking
        } else {
            CompanionViewPhase::Idle
        };
        if matches!(
            runtime.phase,
            coosenpai_core::runtime::RuntimePhase::Observing
        ) {
            self.observer.phase = ObserverViewPhase::Thinking;
        } else if self.observer_running && self.observer.phase == ObserverViewPhase::Thinking {
            self.observer.phase = ObserverViewPhase::Idle;
        }
    }

    pub fn apply_config(&mut self, config: Config) {
        let locale = Locale::from_config(&config.ui.language);
        if self
            .last_error
            .as_ref()
            .is_some_and(|error| error.kind == coosenpai_core::runtime::RuntimeErrorKind::Config)
        {
            self.last_error = None;
        }
        self.observer_provider_label = provider_label(&config.observer.provider);
        self.companion_provider_label = provider_label(&config.companion.provider);
        self.companion_display_name = config.companion.display_name.clone();
        self.observer.ocr_gate_enabled = config.watch.ocr_gate.enabled;
        self.observer.targets = merge_target_views(&self.observer.targets, &config);
        self.notify.mode = config.notification.mode.clone();
        self.notify.minimum_priority = config.notification.min_priority.clone();
        self.screen_recording_message = self
            .screen_recording_message
            .take()
            .map(|message| localize_screen_permission_message(&message, locale));
        self.capture_shortcut_error = self
            .capture_shortcut_error
            .take()
            .map(|message| localize_shortcut_message(&message, locale));
        self.observer.last_capture_disposition = self
            .observer
            .last_capture_disposition
            .take()
            .map(|message| localize_capture_disposition(&message, locale));
        self.observer.error_message =
            self.observer.error_message.take().map(|message| {
                localize_error_message(&message, TextKey::WatchOperationFailed, locale)
            });
        let speech_warning_kind = self.speech.warning_kind.clone();
        self.speech.message = self.speech.message.take().map(|message| {
            speech_warning_kind.as_deref().map_or_else(
                || localize_speech_message(&message, locale),
                |kind| localize_audio_message(kind, &message, locale),
            )
        });
        let audio_warning_kind = self.audio.warning_kind.as_deref().unwrap_or_default();
        self.audio.message = self
            .audio
            .message
            .take()
            .map(|message| localize_audio_message(audio_warning_kind, &message, locale));
        self.config = config;
    }
}

fn speech_permission_name(permission: coosenpai_core::ports::SpeechPermissionKind) -> String {
    match permission {
        coosenpai_core::ports::SpeechPermissionKind::NotDetermined => "not-determined",
        coosenpai_core::ports::SpeechPermissionKind::Granted => "granted",
        coosenpai_core::ports::SpeechPermissionKind::Denied => "denied",
        coosenpai_core::ports::SpeechPermissionKind::Restricted => "restricted",
        coosenpai_core::ports::SpeechPermissionKind::Unavailable => "unavailable",
    }
    .to_owned()
}

fn target_views(config: &Config) -> Vec<WatchTargetView> {
    let locale = coosenpai_core::locale::Locale::from_config(&config.ui.language);
    let mut targets = vec![WatchTargetView {
        target: "fullscreen".to_owned(),
        name: coosenpai_core::locale::text(
            coosenpai_core::locale::TextKey::WatchFullscreenTarget,
            locale,
        )
        .to_owned(),
        enabled: config.watch.fullscreen,
        foreground: true,
        last_captured_at: None,
        last_trigger: None,
    }];
    targets.extend(config.watch.apps.iter().map(|application| WatchTargetView {
        target: format!("app:{}", application.bundle_id),
        name: application.name.clone(),
        enabled: application.enabled,
        foreground: false,
        last_captured_at: None,
        last_trigger: None,
    }));
    targets
}

fn merge_target_views(previous: &[WatchTargetView], config: &Config) -> Vec<WatchTargetView> {
    target_views(config)
        .into_iter()
        .map(|mut target| {
            if let Some(old) = previous.iter().find(|old| old.target == target.target) {
                target.foreground = old.foreground;
                target.last_captured_at.clone_from(&old.last_captured_at);
                target.last_trigger.clone_from(&old.last_trigger);
            }
            target
        })
        .collect()
}

fn provider_label(value: &str) -> String {
    match value {
        "codex" => "Codex CLI",
        "claude" => "Claude Code",
        "opencode" => "OpenCode",
        other => other,
    }
    .to_owned()
}

