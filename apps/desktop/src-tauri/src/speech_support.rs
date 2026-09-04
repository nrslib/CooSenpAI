use crate::command_guard::{CommandContext, CommandSource, DesktopCommand, DispatchError};
use crate::speech::SpeechController;
use crate::state::DesktopState;
use coosenpai_core::ports::{RuntimeLogger, SpeechKeyStatePort, SpeechPermissionKind};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::Manager;

pub(super) async fn send_chat(
    state: &Arc<DesktopState>,
    context: &CommandContext,
    text: String,
) -> Result<String, String> {
    state
        .command_enqueue_user_message(
            context,
            text,
            Vec::new(),
            crate::state::user_input::UserMessageAttachment::None,
        )
        .await
}

pub(super) async fn send_chat_from_callback(
    controller: &SpeechController,
    state: &Arc<DesktopState>,
    generation: u64,
    text: String,
) -> Result<String, String> {
    let handler_state = state.clone();
    let result = state
        .dispatch_with_fence(
            CommandSource::SpeechCallback,
            DesktopCommand::SpeechConfirm,
            crate::command_guard::GenerationStamp {
                resource: crate::command_guard::GenerationResource::Speech,
                value: generation,
            },
            move |context| async move {
                if !controller.lifecycle_generation_is_sending(generation) {
                    return Err(DispatchError::Rejected(
                        crate::command_guard::RejectReason::StaleGeneration,
                    ));
                }
                send_chat(&handler_state, &context, text)
                    .await
                    .map_err(DispatchError::handler)
            },
        )
        .await;
    if matches!(
        result,
        Err(DispatchError::Rejected(
            crate::command_guard::RejectReason::StaleGeneration
        ))
    ) {
        controller
            .complete_stale_send(state.as_ref(), generation)
            .await;
    }
    result.map_err(|error| error.format_for_user())
}

pub(super) fn apply_speech_completion_foreground(sent_to_chat: bool, show_main: impl FnOnce()) {
    if sent_to_chat {
        show_main();
    }
}

pub(super) async fn wait_for_cancel(control: Option<coosenpai_core::ports::SpeechSessionControl>) {
    if let Some(control) = control {
        let _ = control.cancel().await;
    }
}

pub(super) fn show_popup(state: &DesktopState, focus: bool) {
    if let Some(window) = state.app.get_webview_window("speech-popup") {
        let _ = window.set_focusable(focus);
        let _ = crate::windows::position_speech_popup(&window);
        let _ = window.show();
        if focus {
            let _ = window.set_focus();
        }
    }
}

pub(super) fn hide_popup(state: &DesktopState) {
    if let Some(window) = state.app.get_webview_window("speech-popup") {
        let _ = window.hide();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyReleaseOutcome {
    Finish,
    Inactive,
}

pub(super) fn apply_warning(
    view: &mut crate::snapshot::SpeechView,
    kind: impl Into<String>,
    message: impl Into<String>,
) {
    view.warning_kind = Some(kind.into());
    view.message = Some(message.into());
}

pub(super) fn apply_failure(view: &mut crate::snapshot::SpeechView, message: impl Into<String>) {
    view.phase = "idle".to_owned();
    view.partial.clear();
    view.warning_kind = None;
    view.message = Some(message.into());
    view.source = None;
}

pub(super) fn apply_confirmation_failure(
    view: &mut crate::snapshot::SpeechView,
    generation: u64,
    message: &str,
) -> bool {
    if view.generation != generation {
        return false;
    }
    view.phase = "confirming".to_owned();
    view.message = Some(message.to_owned());
    true
}

const SPEECH_FAILURE_DISPLAY_DURATION: Duration = Duration::from_secs(3);
pub(super) const NO_SPEECH_FAILURE_MESSAGE: &str = "音声を聞き取れませんでした";
const GENERIC_SPEECH_FAILURE_MESSAGE: &str = "音声入力に失敗しました";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SpeechErrorPresentation {
    pub(super) message: &'static str,
    pub(super) log_original: bool,
}

pub(super) fn localize_speech_error(kind: Option<&str>, original: &str) -> SpeechErrorPresentation {
    if kind == Some("no-speech") || is_no_speech_error(original) {
        return SpeechErrorPresentation {
            message: NO_SPEECH_FAILURE_MESSAGE,
            log_original: false,
        };
    }

    let (message, log_original) = match kind {
        Some("permission-microphone") => ("マイクの使用が許可されていません", false),
        Some("permission-speech") => ("音声認識の使用が許可されていません", false),
        Some("locale-unavailable") => ("指定したロケールの音声認識は利用できません", false),
        Some("on-device-unsupported") => (
            "このロケールではオンデバイス音声認識を利用できません",
            false,
        ),
        Some("input-device") => ("音声入力デバイスを利用できません", false),
        Some("input-device-list") => ("マイク一覧を取得できません", true),
        Some("key-state") => ("マイクキーの状態を確認できないため録音を終了します", true),
        _ => (GENERIC_SPEECH_FAILURE_MESSAGE, true),
    };
    SpeechErrorPresentation {
        message,
        log_original,
    }
}

pub(super) fn present_speech_error(
    state: &DesktopState,
    kind: Option<&str>,
    original: &str,
) -> SpeechErrorPresentation {
    let presentation = localize_speech_error(kind, original);
    if presentation.log_original {
        let error_type = kind.unwrap_or("unknown");
        let _ = state.logger.write(
            "WARN",
            &format!(
                "音声入力エラーを表示用に変換しました: error-type={error_type} detail={original}"
            ),
        );
    }
    presentation
}

fn is_no_speech_error(original: &str) -> bool {
    let normalized = original.to_ascii_lowercase();
    [
        "no speech detected",
        "no speech was detected",
        "speech not detected",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
        || (normalized.contains("kafassistanterrordomain") && normalized.contains("1110"))
        || [
            "音声を検出できません",
            "音声が検出されません",
            "話し声を検出できません",
        ]
        .iter()
        .any(|needle| original.contains(needle))
}

pub(super) fn schedule_speech_failure_clear(
    state: Arc<DesktopState>,
    generation: u64,
    message: String,
    failure_ids: Arc<AtomicU64>,
    failure_id: u64,
) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SPEECH_FAILURE_DISPLAY_DURATION).await;
        let snapshot = state.snapshot().await;
        if failure_ids.load(Ordering::Acquire) != failure_id
            || snapshot.speech.generation != generation
            || snapshot.speech.message.as_deref() != Some(message.as_str())
        {
            return;
        }
        state
            .publish(|snapshot| {
                clear_speech_failure_message(
                    &mut snapshot.speech,
                    generation,
                    &message,
                    failure_ids.load(Ordering::Acquire),
                    failure_id,
                );
            })
            .await;
    });
}

pub(super) fn clear_speech_failure_message(
    view: &mut crate::snapshot::SpeechView,
    generation: u64,
    message: &str,
    current_failure_id: u64,
    expected_failure_id: u64,
) -> bool {
    if current_failure_id == expected_failure_id
        && view.generation == generation
        && view.message.as_deref() == Some(message)
    {
        view.message = None;
        true
    } else {
        false
    }
}

pub(super) fn should_finish_push_to_talk(
    key_state: &dyn SpeechKeyStatePort,
    shortcut: &str,
    maximum_elapsed: bool,
) -> Result<bool, coosenpai_core::ports::PortError> {
    if maximum_elapsed {
        return Ok(true);
    }
    key_state
        .primary_key_pressed(shortcut)
        .map(|pressed| !pressed)
}

pub(super) async fn wait_for_push_to_talk_end(
    key_state: &dyn SpeechKeyStatePort,
    shortcut: &str,
    poll_interval: Duration,
    maximum_duration: Duration,
    mut active: impl FnMut() -> bool,
) -> Result<KeyReleaseOutcome, coosenpai_core::ports::PortError> {
    let deadline = tokio::time::Instant::now() + maximum_duration;
    let mut interval = tokio::time::interval(poll_interval);
    interval.tick().await;
    loop {
        interval.tick().await;
        if !active() {
            return Ok(KeyReleaseOutcome::Inactive);
        }
        if should_finish_push_to_talk(key_state, shortcut, tokio::time::Instant::now() >= deadline)?
        {
            return Ok(KeyReleaseOutcome::Finish);
        }
    }
}

pub(crate) fn permission_name(permission: SpeechPermissionKind) -> String {
    match permission {
        SpeechPermissionKind::NotDetermined => "not-determined",
        SpeechPermissionKind::Granted => "granted",
        SpeechPermissionKind::Denied => "denied",
        SpeechPermissionKind::Restricted => "restricted",
        SpeechPermissionKind::Unavailable => "unavailable",
    }
    .to_owned()
}

pub(super) fn denied_permission_message(
    permissions: coosenpai_core::ports::SpeechPermissions,
) -> Option<&'static str> {
    match permissions.microphone {
        SpeechPermissionKind::Granted => {}
        SpeechPermissionKind::Denied => return Some("マイクの使用が許可されていません"),
        SpeechPermissionKind::Restricted => return Some("マイクの使用が制限されています"),
        SpeechPermissionKind::NotDetermined | SpeechPermissionKind::Unavailable => {
            return Some("マイクを利用できません")
        }
    }
    match permissions.recognition {
        SpeechPermissionKind::Granted => None,
        SpeechPermissionKind::Denied => Some("音声認識の使用が許可されていません"),
        SpeechPermissionKind::Restricted => Some("音声認識の使用が制限されています"),
        SpeechPermissionKind::NotDetermined | SpeechPermissionKind::Unavailable => {
            Some("音声認識を利用できません")
        }
    }
}

