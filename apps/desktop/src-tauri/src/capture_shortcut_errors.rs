use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
use coosenpai_core::locale::{localize_capture_message, localize_shortcut_message, Locale};
use std::sync::Arc;

pub(super) async fn publish_shortcut_error(state: &DesktopState, error: String) -> AppSnapshot {
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Shortcut(
            ShortcutErrorEvent::RegistrationFailed {
                lifecycle_revision: None,
                error,
            },
        ))
        .await
}

pub(super) async fn publish_speech_shortcut_error(
    state: &DesktopState,
    lifecycle_revision: u64,
    error: String,
) -> AppSnapshot {
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Shortcut(
            ShortcutErrorEvent::RegistrationFailed {
                lifecycle_revision: Some(lifecycle_revision),
                error,
            },
        ))
        .await
}

pub(super) async fn clear_shortcut_error_if_current(
    state: &DesktopState,
    expected: ShortcutErrorToken,
) {
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Shortcut(
            ShortcutErrorEvent::RegistrationSucceeded {
                lifecycle_revision: None,
                expected,
            },
        ))
        .await;
}

pub(super) async fn clear_speech_shortcut_error_if_current(
    state: &DesktopState,
    lifecycle_revision: u64,
    expected: ShortcutErrorToken,
) {
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Shortcut(
            ShortcutErrorEvent::RegistrationSucceeded {
                lifecycle_revision: Some(lifecycle_revision),
                expected,
            },
        ))
        .await;
}

pub(crate) async fn publish_transient_shortcut_error(state: Arc<DesktopState>, message: String) {
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Shortcut(
            ShortcutErrorEvent::Transient {
                speech_generation: None,
                message,
            },
        ))
        .await;
}

pub(crate) async fn publish_speech_transient_shortcut_error(
    state: Arc<DesktopState>,
    generation: u64,
    message: String,
) {
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Shortcut(
            ShortcutErrorEvent::Transient {
                speech_generation: Some(generation),
                message,
            },
        ))
        .await;
}

#[derive(Debug)]
pub(crate) enum ShortcutErrorEvent {
    RegistrationFailed {
        lifecycle_revision: Option<u64>,
        error: String,
    },
    RegistrationSucceeded {
        lifecycle_revision: Option<u64>,
        expected: ShortcutErrorToken,
    },
    Transient {
        speech_generation: Option<u64>,
        message: String,
    },
    Expired {
        token: ShortcutErrorToken,
        message: String,
    },
}

pub(crate) struct ShortcutErrorPresenter {
    coordinator: Arc<super::ShortcutCoordinator>,
    speech: Arc<std::sync::Mutex<crate::speech_lifecycle::SpeechLifecycle>>,
}

impl ShortcutErrorPresenter {
    pub(crate) fn new(
        coordinator: Arc<super::ShortcutCoordinator>,
        speech: Arc<std::sync::Mutex<crate::speech_lifecycle::SpeechLifecycle>>,
    ) -> Self {
        Self {
            coordinator,
            speech,
        }
    }

    pub(crate) fn adopt(
        &mut self,
        snapshot: &mut AppSnapshot,
        event: ShortcutErrorEvent,
    ) -> Option<Vec<crate::ui_events::UiEffect>> {
        use crate::ui_events::{UiEffect, UiEvent, UiTask};
        let error_id = snapshot.capture_shortcut_error_id.saturating_add(1);
        let locale = Locale::from_config(&snapshot.config.ui.language);
        let mut effects = Vec::new();
        match event {
            ShortcutErrorEvent::RegistrationFailed {
                lifecycle_revision,
                error,
            } => {
                if lifecycle_revision
                    .is_some_and(|revision| !self.coordinator.accepts_speech_revision(revision))
                {
                    return None;
                }
                let error =
                    localize_shortcut_message(&localize_capture_message(&error, locale), locale);
                set_shortcut_error(snapshot, error_id, Some(error), None);
            }
            ShortcutErrorEvent::RegistrationSucceeded {
                lifecycle_revision,
                expected,
            } => {
                if lifecycle_revision
                    .is_some_and(|revision| !self.coordinator.accepts_speech_revision(revision))
                    || !can_clear_shortcut_error(snapshot, expected)
                {
                    return None;
                }
                set_shortcut_error(snapshot, error_id, None, None);
            }
            ShortcutErrorEvent::Transient {
                speech_generation,
                message,
            } => {
                if let Some(generation) = speech_generation {
                    let speech = self
                        .speech
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !speech.is_current(generation)
                        || speech.can_apply_cleanup(generation)
                        || snapshot.speech.generation != generation
                    {
                        return None;
                    }
                }
                let message =
                    localize_shortcut_message(&localize_capture_message(&message, locale), locale);
                set_shortcut_error(snapshot, error_id, Some(message.clone()), speech_generation);
                let token = shortcut_error_token(snapshot);
                effects.push(UiEffect::Spawn(UiTask::Delay {
                    duration: std::time::Duration::from_secs(3),
                    event: UiEvent::SnapshotCompleted(Box::new(
                        crate::snapshot_presenter::SnapshotEvent::Shortcut(
                            ShortcutErrorEvent::Expired { token, message },
                        ),
                    )),
                }));
            }
            ShortcutErrorEvent::Expired { token, message } => {
                if !is_current_shortcut_error(
                    snapshot.capture_shortcut_error_id,
                    token.id,
                    snapshot.capture_shortcut_error_speech_generation,
                    token.speech_generation,
                    snapshot.capture_shortcut_error.as_deref(),
                    &message,
                ) {
                    return None;
                }
                set_shortcut_error(snapshot, error_id, None, None);
            }
        }
        Some(effects)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShortcutErrorToken {
    id: u64,
    speech_generation: Option<u64>,
}

fn shortcut_error_token(snapshot: &AppSnapshot) -> ShortcutErrorToken {
    ShortcutErrorToken {
        id: snapshot.capture_shortcut_error_id,
        speech_generation: snapshot.capture_shortcut_error_speech_generation,
    }
}

fn can_clear_shortcut_error(snapshot: &AppSnapshot, expected: ShortcutErrorToken) -> bool {
    can_clear_shortcut_error_token(
        shortcut_error_token(snapshot),
        snapshot.capture_shortcut_error.is_some(),
        expected,
    )
}

fn can_clear_shortcut_error_token(
    current: ShortcutErrorToken,
    has_error: bool,
    expected: ShortcutErrorToken,
) -> bool {
    has_error && current == expected
}

pub(super) async fn current_shortcut_error_token(state: &DesktopState) -> ShortcutErrorToken {
    shortcut_error_token(&state.snapshot().await)
}

fn set_shortcut_error(
    snapshot: &mut AppSnapshot,
    error_id: u64,
    error: Option<String>,
    speech_generation: Option<u64>,
) {
    snapshot.capture_shortcut_error_id = error_id;
    snapshot.capture_shortcut_error = error;
    snapshot.capture_shortcut_error_speech_generation = speech_generation;
}

fn is_current_shortcut_error(
    current_id: u64,
    expected_id: u64,
    current_generation: Option<u64>,
    expected_generation: Option<u64>,
    current_message: Option<&str>,
    expected_message: &str,
) -> bool {
    current_id == expected_id
        && current_generation == expected_generation
        && current_message.is_some_and(|message| {
            message == expected_message
                || localize_capture_message(message, Locale::Ja)
                    == localize_capture_message(expected_message, Locale::Ja)
        })
}

