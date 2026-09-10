use crate::snapshot::AppSnapshot;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask};
use coosenpai_core::locale::Locale;
use coosenpai_core::ports::{ScreenCapturePermission, SpeechPermissions};
use coosenpai_core::runtime::RuntimeSnapshot;
use std::sync::{Arc, Mutex};

pub(crate) fn runtime_view_changed(previous: &RuntimeSnapshot, next: &RuntimeSnapshot) -> bool {
    let mut previous = previous.clone();
    previous.revision = next.revision;
    previous.pending_observations = next.pending_observations;
    previous != *next
}

#[derive(Debug)]
pub(crate) struct SnapshotInput {
    pub event: SnapshotEvent,
    pub config_revision: u64,
    pub work: coosenpai_core::work::WorkConfig,
}

#[derive(Debug)]
pub(crate) enum SnapshotEvent {
    RuntimeObserved {
        runtime: coosenpai_core::runtime::RuntimeSnapshot,
        initial: bool,
    },
    MetadataChanged,
    TutorialLoaded(crate::tutorial_projection::TutorialSnapshotData),
    TutorialActivated {
        config: Box<coosenpai_core::config::Config>,
        tutorial: crate::tutorial_projection::TutorialSnapshotData,
    },
    TutorialEnded {
        tutorial: crate::tutorial_projection::TutorialSnapshotData,
        runtime_error: Option<coosenpai_core::runtime::RuntimeLastError>,
    },
    TutorialPersistenceFailed(coosenpai_core::runtime::RuntimeLastError),
    UnreadRead,
    UnreadAdded(tokio::sync::oneshot::Sender<bool>),
    TemporarySelected(
        Option<coosenpai_core::companion_assertiveness::TemporaryAssertivenessSelection>,
    ),
    TemporaryExpired(chrono::DateTime<chrono::Utc>),
    TemporaryCleared {
        expires_at: chrono::DateTime<chrono::Utc>,
        cleared: bool,
    },
    Shortcut(crate::capture::ShortcutErrorEvent),
    Watch {
        generation: u64,
        event: crate::watch_presenter::WatchResult,
    },
    ConfigLoaded(coosenpai_core::config::Config),
    CompanionStopped,
    CompanionReconfigured(coosenpai_core::config::Config),
    CompanionFailed(coosenpai_core::runtime::RuntimeLastError),
    AvatarRefresh,
    AvatarLoaded {
        generation: u64,
        result: crate::avatar::AvatarLoadResult,
    },
    Hearing(crate::hearing_presenter::HearingResult),
    Speech {
        generation: u64,
        event: crate::speech_presenter::SpeechResult,
    },
    Runtime(coosenpai_core::runtime::RuntimeSnapshot),
    ConversationLoaded {
        conversation: Vec<coosenpai_core::state::ConversationEntry>,
        generations: Vec<coosenpai_core::conversation_archive::ConversationGenerationSummary>,
        selected_generation: u64,
        calls: u32,
        limit_reached: bool,
    },
    SpeechDevicesLoaded(Result<Vec<coosenpai_core::ports::SpeechInputDevice>, String>),
    DebugLoaded(coosenpai_core::debug::DebugCatalog),
    SpeechPermissionsLoaded(SpeechPermissions),
    ScreenPermissionLoaded(ScreenCapturePermission),
}

#[derive(Debug, Default)]
pub(crate) struct PublicationCount {
    pub attempted: u64,
    pub published: u64,
}

pub(crate) struct SnapshotPresenter {
    publications: std::collections::BTreeMap<&'static str, PublicationCount>,
    shortcut: crate::capture::ShortcutErrorPresenter,
    watch: crate::watch_presenter::WatchPresenter,
    avatar_generation: u64,
    speech: crate::speech_presenter::SpeechPresenter,
    runtime: Option<RuntimeSnapshot>,
    snapshot: Arc<Mutex<AppSnapshot>>,
}

impl SnapshotPresenter {
    pub(crate) fn new(
        snapshot: Arc<Mutex<AppSnapshot>>,
        lifecycle: Arc<Mutex<crate::speech_lifecycle::SpeechLifecycle>>,
        coordinator: Arc<crate::capture::ShortcutCoordinator>,
    ) -> Self {
        Self {
            publications: Default::default(),
            shortcut: crate::capture::ShortcutErrorPresenter::new(coordinator, lifecycle.clone()),
            watch: Default::default(),
            avatar_generation: 0,
            snapshot,
            speech: crate::speech_presenter::SpeechPresenter::new(lifecycle),
            runtime: None,
        }
    }

    pub(crate) fn input_started(&mut self) {
        self.speech.input_started();
    }

    pub(crate) fn take_publication_counts(
        &mut self,
    ) -> std::collections::BTreeMap<&'static str, PublicationCount> {
        std::mem::take(&mut self.publications)
    }

    pub(crate) fn current(&self) -> AppSnapshot {
        self.snapshot.lock().expect("snapshot lock").clone()
    }

    pub(crate) fn handle(&mut self, input: SnapshotInput) -> Vec<UiEffect> {
        self.adopt(input.event, Some((input.config_revision, input.work)))
    }

    pub(crate) fn complete(&mut self, event: SnapshotEvent) -> Vec<UiEffect> {
        self.adopt(event, None)
    }

    fn adopt(
        &mut self,
        event: SnapshotEvent,
        metadata: Option<(u64, coosenpai_core::work::WorkConfig)>,
    ) -> Vec<UiEffect> {
        let count = self.publications.entry(event.source()).or_default();
        count.attempted += 1;
        let mut snapshot = self.snapshot.lock().expect("snapshot lock");
        let before = serde_json::to_value(&*snapshot).expect("snapshot serialization");
        let previous_speech = crate::speech::SpeechPopupSnapshot::from_app(&snapshot);
        let old_avatar_path = snapshot.config.ui.avatar_path.clone();
        let mut refresh_avatar = false;
        let mut effects = Vec::new();
        match event {
            SnapshotEvent::RuntimeObserved { runtime, initial } => {
                let refresh = initial
                    || self.runtime.as_ref().is_none_or(|previous| {
                        (previous.phase != runtime.phase
                            && previous.phase != coosenpai_core::runtime::RuntimePhase::Idle)
                            || previous.active_user_message_id != runtime.active_user_message_id
                            || previous.cancelled_user_message_ids
                                != runtime.cancelled_user_message_ids
                            || previous.latest_companion_decision
                                != runtime.latest_companion_decision
                            || previous.pending_deliveries != runtime.pending_deliveries
                            || previous.last_error != runtime.last_error
                    });
                let thought_changed = initial
                    || self.runtime.as_ref().is_none_or(|previous| {
                        previous.latest_companion_thought != runtime.latest_companion_thought
                            || previous.latest_companion_thought_generation
                                != runtime.latest_companion_thought_generation
                    });
                snapshot.apply_runtime(&runtime);
                self.runtime = Some(runtime.clone());
                if thought_changed {
                    effects.push(UiEffect::Deliver {
                        child: PresenterId::Bubble,
                        event: UiEvent::ThoughtObserved {
                            runtime: Box::new(runtime),
                            initial,
                        },
                    });
                }
                if refresh {
                    effects.push(UiEffect::Spawn(if snapshot.onboarding.tutorial_active {
                        UiTask::RuntimeFollowup
                    } else {
                        UiTask::RefreshConversation
                    }));
                }
            }
            SnapshotEvent::MetadataChanged => {}
            SnapshotEvent::TutorialLoaded(tutorial) => snapshot.onboarding = tutorial.view(),
            SnapshotEvent::TutorialActivated { config, tutorial } => {
                snapshot.apply_config(*config);
                snapshot.onboarding = tutorial.view();
            }
            SnapshotEvent::TutorialEnded {
                tutorial,
                runtime_error,
            } => {
                snapshot.conversation.clear();
                snapshot.onboarding = tutorial.view();
                snapshot.last_error = runtime_error;
            }
            SnapshotEvent::TutorialPersistenceFailed(error) => snapshot.last_error = Some(error),
            SnapshotEvent::UnreadRead => snapshot.unread_count = 0,
            SnapshotEvent::UnreadAdded(reply) => {
                snapshot.unread_count = snapshot.unread_count.saturating_add(1);
                let _ = reply.send(true);
            }
            SnapshotEvent::TemporarySelected(selection) => {
                if let Some(selection) = &selection {
                    effects.push(UiEffect::Spawn(UiTask::Delay {
                        duration: selection
                            .expires_at
                            .signed_duration_since(chrono::Utc::now())
                            .to_std()
                            .unwrap_or(std::time::Duration::ZERO),
                        event: UiEvent::SnapshotCompleted(Box::new(
                            SnapshotEvent::TemporaryExpired(selection.expires_at),
                        )),
                    }));
                }
                snapshot.temporary_assertiveness = selection;
            }
            SnapshotEvent::TemporaryExpired(expires_at) => {
                if snapshot
                    .temporary_assertiveness
                    .as_ref()
                    .is_some_and(|selection| selection.expires_at == expires_at)
                {
                    effects.push(UiEffect::Spawn(UiTask::ClearTemporary { expires_at }));
                }
                return effects;
            }
            SnapshotEvent::TemporaryCleared {
                expires_at,
                cleared,
            } => {
                if !cleared
                    || !snapshot
                        .temporary_assertiveness
                        .as_ref()
                        .is_some_and(|selection| selection.expires_at == expires_at)
                {
                    return effects;
                }
                snapshot.temporary_assertiveness = None;
            }
            SnapshotEvent::Shortcut(event) => {
                let Some(shortcut_effects) = self.shortcut.adopt(&mut snapshot, event) else {
                    return effects;
                };
                effects.extend(shortcut_effects);
            }
            SnapshotEvent::Watch { generation, event } => {
                let changed = self.watch.adopt(&mut snapshot, generation, event);
                effects.extend(self.watch.take_effects());
                if !changed {
                    return effects;
                }
            }
            SnapshotEvent::ConfigLoaded(config) => snapshot.apply_config(config),
            SnapshotEvent::CompanionStopped => {
                snapshot.companion.phase = crate::snapshot::CompanionViewPhase::Idle
            }
            SnapshotEvent::CompanionReconfigured(config) => {
                snapshot.apply_config(config);
                snapshot.companion.phase = crate::snapshot::CompanionViewPhase::Idle;
            }
            SnapshotEvent::CompanionFailed(error) => {
                snapshot.last_error = Some(error);
                snapshot.companion.ready = false;
                snapshot.companion.phase = crate::snapshot::CompanionViewPhase::Error;
            }
            SnapshotEvent::AvatarRefresh => refresh_avatar = true,
            SnapshotEvent::AvatarLoaded { generation, result } => {
                if generation != self.avatar_generation {
                    return effects;
                }
                snapshot.avatar_image_png = result.image_png;
                snapshot.avatar_image_load_failed = result.failed;
            }
            SnapshotEvent::Hearing(result) => {
                if !crate::hearing_presenter::adopt(&mut snapshot, result) {
                    return effects;
                }
            }
            SnapshotEvent::Speech { generation, event } => {
                let locale = Locale::from_config(&snapshot.config.ui.language);
                let Some(speech_effects) =
                    self.speech
                        .handle(&mut snapshot.speech, locale, generation, event)
                else {
                    return effects;
                };
                effects.extend(speech_effects);
            }
            SnapshotEvent::Runtime(runtime) => snapshot.apply_runtime(&runtime),
            SnapshotEvent::ConversationLoaded {
                conversation,
                generations,
                selected_generation,
                calls,
                limit_reached,
            } => {
                snapshot.conversation = conversation;
                snapshot.conversation_generations = generations;
                snapshot.selected_conversation_generation = selected_generation;
                snapshot.companion.total_calls_today = calls;
                snapshot.companion.proactive_limit_reached = limit_reached;
            }
            SnapshotEvent::SpeechDevicesLoaded(result) => match result {
                Ok(devices) => snapshot.speech.input_devices = devices,
                Err(original) => {
                    let presentation = crate::speech::support::localize_speech_error_for_locale(
                        Some("input-device-list"),
                        &original,
                        Locale::from_config(&snapshot.config.ui.language),
                    );
                    snapshot.speech.warning_kind = Some("input-device-list".to_owned());
                    snapshot.speech.message = Some(presentation.message.to_owned());
                    if presentation.log_original {
                        effects.push(UiEffect::Log(format!("音声入力エラーを表示用に変換しました: error-type=input-device-list detail={original}")));
                    }
                }
            },
            SnapshotEvent::DebugLoaded(catalog) => snapshot.debug_catalog = catalog,
            SnapshotEvent::SpeechPermissionsLoaded(permissions) => {
                snapshot.speech.microphone_permission =
                    crate::speech::permission_name(permissions.microphone);
                snapshot.speech.recognition_permission =
                    crate::speech::permission_name(permissions.recognition);
            }
            SnapshotEvent::ScreenPermissionLoaded(permission) => {
                let presentation = permission
                    .presentation_for_locale(Locale::from_config(&snapshot.config.ui.language));
                snapshot.screen_recording_status = presentation.status.to_owned();
                snapshot.screen_recording_message = presentation.message.map(str::to_owned);
                snapshot.screen_recording_restart_required = permission.requires_restart();
                snapshot.audio.screen_capture_permission =
                    if crate::platform::speaker_requires_screen_recording() {
                        presentation.status
                    } else {
                        "not-required"
                    }
                    .to_owned();
            }
        }
        if refresh_avatar || old_avatar_path != snapshot.config.ui.avatar_path {
            self.avatar_generation = self.avatar_generation.saturating_add(1);
            effects.push(UiEffect::Spawn(UiTask::LoadAvatar {
                generation: self.avatar_generation,
                path: snapshot.config.ui.avatar_path.clone(),
            }));
        }
        if let Some((revision, work)) = metadata {
            snapshot.config.work = work;
            snapshot.config_revision = revision;
        }
        if before == serde_json::to_value(&*snapshot).expect("snapshot serialization") {
            return effects;
        }
        count.published += 1;
        let voice_changed = (previous_speech.speech.phase != "idle"
            || snapshot.speech.phase != "idle")
            && previous_speech != crate::speech::SpeechPopupSnapshot::from_app(&snapshot);
        snapshot.revision = snapshot.revision.saturating_add(1);
        effects.extend(snapshot_effects(&snapshot, voice_changed));
        effects
    }

    pub(crate) fn expire_speech(&mut self, generation: u64, failure_id: u64) -> Vec<UiEffect> {
        let mut snapshot = self.snapshot.lock().expect("snapshot lock");
        if !self
            .speech
            .expire(&mut snapshot.speech, generation, failure_id)
        {
            return Vec::new();
        }
        snapshot.revision = snapshot.revision.saturating_add(1);
        snapshot_effects(&snapshot, snapshot.speech.phase != "idle")
    }
}

fn snapshot_effects(snapshot: &AppSnapshot, speech_changed: bool) -> Vec<UiEffect> {
    let result = Arc::new(snapshot.clone());
    let mut effects = Vec::new();
    if speech_changed {
        effects.push(UiEffect::Deliver {
            child: PresenterId::Capture,
            event: UiEvent::CaptureCompleted(Box::new(
                crate::capture::CaptureEvent::VoiceProgress(Arc::new(
                    crate::speech::SpeechPopupSnapshot::from_app(&result),
                )),
            )),
        });
    }
    effects.push(UiEffect::Deliver {
        child: PresenterId::Root,
        event: UiEvent::SnapshotUpdated(result),
    });
    effects
}

impl SnapshotEvent {
    pub(crate) fn source(&self) -> &'static str {
        match self {
            Self::RuntimeObserved { .. } => "RuntimeObserved",
            Self::Runtime(_) => "Runtime",
            Self::ConversationLoaded { .. } => "ConversationLoaded",
            Self::DebugLoaded(_) => "DebugLoaded",
            Self::Hearing(_) => "Hearing",
            Self::Speech { .. } => "Speech",
            Self::Watch { .. } => "Watch",
            Self::MetadataChanged => "MetadataChanged",
            Self::TutorialLoaded(_)
            | Self::TutorialActivated { .. }
            | Self::TutorialEnded { .. }
            | Self::TutorialPersistenceFailed(_) => "Tutorial",
            Self::UnreadRead | Self::UnreadAdded(_) => "Unread",
            Self::TemporarySelected(_)
            | Self::TemporaryExpired(_)
            | Self::TemporaryCleared { .. } => "TemporaryAssertiveness",
            Self::Shortcut(_) => "Shortcut",
            Self::ConfigLoaded(_) | Self::CompanionReconfigured(_) => "Config",
            Self::CompanionStopped | Self::CompanionFailed(_) => "Companion",
            Self::AvatarRefresh | Self::AvatarLoaded { .. } => "Avatar",
            Self::SpeechDevicesLoaded(_) => "SpeechDevices",
            Self::SpeechPermissionsLoaded(_) => "SpeechPermissions",
            Self::ScreenPermissionLoaded(_) => "ScreenPermission",
        }
    }
}
