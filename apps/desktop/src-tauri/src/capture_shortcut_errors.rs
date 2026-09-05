use crate::command_guard::GenerationStamp;
use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_SHORTCUT_ERROR_ID: AtomicU64 = AtomicU64::new(1);

pub(super) async fn publish_shortcut_error(state: &DesktopState, error: String) -> AppSnapshot {
    let error_id = next_shortcut_error_id();
    state
        .publish(|snapshot| {
            set_shortcut_error(snapshot, error_id, Some(error), None);
        })
        .await
}

pub(super) async fn publish_speech_shortcut_error(
    state: &DesktopState,
    lifecycle_revision: u64,
    error: String,
) -> AppSnapshot {
    let error_id = next_shortcut_error_id();
    state
        .publish(|snapshot| {
            if state
                .shortcut_coordinator
                .accepts_speech_revision(lifecycle_revision)
            {
                set_shortcut_error(snapshot, error_id, Some(error), None);
            }
        })
        .await
}

pub(super) async fn clear_shortcut_error_if_current(
    state: &DesktopState,
    expected: ShortcutErrorToken,
) {
    state
        .publish(|snapshot| {
            if can_clear_shortcut_error(snapshot, expected) {
                set_shortcut_error(snapshot, next_shortcut_error_id(), None, None);
            }
        })
        .await;
}

pub(super) async fn clear_speech_shortcut_error_if_current(
    state: &DesktopState,
    lifecycle_revision: u64,
    expected: ShortcutErrorToken,
) {
    state
        .publish(|snapshot| {
            if state
                .shortcut_coordinator
                .accepts_speech_revision(lifecycle_revision)
                && can_clear_shortcut_error(snapshot, expected)
            {
                set_shortcut_error(snapshot, next_shortcut_error_id(), None, None);
            }
        })
        .await;
}

pub(crate) async fn publish_transient_shortcut_error(state: Arc<DesktopState>, message: String) {
    let error_id = next_shortcut_error_id();
    state
        .publish(|snapshot| {
            set_shortcut_error(snapshot, error_id, Some(message.clone()), None);
        })
        .await;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        clear_transient_shortcut_error(&state, error_id, None, &message).await;
    });
}

pub(super) async fn publish_capture_transient_shortcut_error(
    state: Arc<DesktopState>,
    generation: GenerationStamp,
    message: String,
) {
    let error_id = next_shortcut_error_id();
    let snapshot = state
        .publish(|snapshot| {
            if state.ensure_command_generation(generation).is_ok() {
                set_shortcut_error(snapshot, error_id, Some(message.clone()), None);
            }
        })
        .await;
    if snapshot.capture_shortcut_error_id != error_id {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        clear_transient_shortcut_error(&state, error_id, None, &message).await;
    });
}

pub(crate) async fn publish_speech_transient_shortcut_error(
    state: Arc<DesktopState>,
    generation: u64,
    message: String,
) {
    let error_id = next_shortcut_error_id();
    let snapshot = state
        .publish(|snapshot| {
            if state.speech_accepts_transient_shortcut_error(generation)
                && snapshot.speech.generation == generation
            {
                set_shortcut_error(snapshot, error_id, Some(message.clone()), Some(generation));
            }
        })
        .await;
    if snapshot.capture_shortcut_error_id != error_id {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        clear_transient_shortcut_error(&state, error_id, Some(generation), &message).await;
    });
}

fn next_shortcut_error_id() -> u64 {
    NEXT_SHORTCUT_ERROR_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ShortcutErrorToken {
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

async fn clear_transient_shortcut_error(
    state: &DesktopState,
    error_id: u64,
    speech_generation: Option<u64>,
    message: &str,
) {
    state
        .publish(|snapshot| {
            if is_current_shortcut_error(
                snapshot.capture_shortcut_error_id,
                error_id,
                snapshot.capture_shortcut_error_speech_generation,
                speech_generation,
                snapshot.capture_shortcut_error.as_deref(),
                message,
            ) {
                set_shortcut_error(snapshot, next_shortcut_error_id(), None, None);
            }
        })
        .await;
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
        && current_message == Some(expected_message)
}

pub(super) async fn fail_if_current(
    state: &DesktopState,
    generation: GenerationStamp,
    message: &str,
) {
    if reset_if_current(state, generation).await {
        if state.ensure_command_generation(generation).is_ok() {
            publish_shortcut_error(state, message.to_owned()).await;
        }
        if state.ensure_command_generation(generation).is_ok() {
            crate::windows::show_main(&state.app);
        }
    }
}

pub(super) async fn reset_if_current(state: &DesktopState, generation: GenerationStamp) -> bool {
    let Ok(mut capture) = state.capture_popup_for_event(generation).await else {
        return false;
    };
    capture.reset_capturing_if_current(generation)
}

