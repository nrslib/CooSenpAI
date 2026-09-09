use crate::speech::SpeechController;
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::{RuntimeLogger, SpeechKeyStatePort, SpeechPermissionKind};
use std::sync::Arc;
use std::time::Duration;

pub(super) async fn send_chat_from_callback(
    controller: &SpeechController,
    state: &Arc<DesktopState>,
    generation: u64,
    text: String,
) -> Result<String, String> {
    if !controller.lifecycle_generation_is_sending(generation) {
        controller.complete_stale_send(state, generation).await;
        return Err("音声入力の世代が更新されています".to_owned());
    }
    state
        .ui
        .request(
            crate::ui_events::UiView::SpeechPopup,
            crate::ui_events::UiEvent::SubmitInput(crate::ui_events::ChatInput::Voice {
                generation,
                text,
            }),
        )
        .await?
        .ok_or_else(|| "送信の受理結果がありません".to_owned())
}

pub(super) async fn complete_callback_send<S, SFut, C, CFut>(
    send: S,
    settle: C,
) -> Result<String, String>
where
    S: FnOnce() -> SFut,
    SFut: std::future::Future<Output = Result<String, String>>,
    C: FnOnce(Result<String, String>) -> CFut,
    CFut: std::future::Future<Output = ()>,
{
    let result = send().await;
    settle(result.clone()).await;
    result
}

pub(super) async fn wait_for_cancel(control: Option<coosenpai_core::ports::SpeechSessionControl>) {
    if let Some(control) = control {
        let _ = control.cancel().await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyReleaseOutcome {
    Finish,
    Inactive,
}

pub(crate) fn apply_warning(
    view: &mut crate::snapshot::SpeechView,
    kind: impl Into<String>,
    message: impl Into<String>,
) {
    view.warning_kind = Some(kind.into());
    view.message = Some(message.into());
}

pub(crate) fn apply_failure(view: &mut crate::snapshot::SpeechView, message: impl Into<String>) {
    view.phase = "idle".to_owned();
    view.partial.clear();
    view.warning_kind = None;
    view.message = Some(message.into());
    view.source = None;
}

pub(crate) fn apply_confirmation_failure(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpeechErrorPresentation {
    pub(crate) message: &'static str,
    pub(crate) log_original: bool,
}

pub(crate) fn localize_speech_error_for_locale(
    kind: Option<&str>,
    original: &str,
    locale: Locale,
) -> SpeechErrorPresentation {
    if kind == Some("no-speech") || is_no_speech_error(original) {
        return SpeechErrorPresentation {
            message: text(TextKey::SpeechNoSpeech, locale),
            log_original: false,
        };
    }

    let (key, log_original) = match kind {
        Some("permission-microphone") => (TextKey::SpeechMicrophoneDenied, false),
        Some("permission-speech") => (TextKey::SpeechRecognitionDenied, false),
        Some("locale-unavailable") => (TextKey::SpeechLocaleUnavailable, false),
        Some("on-device-unsupported") => (TextKey::SpeechOnDeviceUnsupported, false),
        Some("input-device") => (TextKey::SpeechInputDeviceUnavailable, false),
        Some("input-device-list") => (TextKey::SpeechInputDeviceListFailed, true),
        Some("key-state") => (TextKey::SpeechKeyStateFailed, true),
        _ => (TextKey::SpeechGenericFailure, true),
    };
    SpeechErrorPresentation {
        message: text(key, locale),
        log_original,
    }
}

pub(super) fn present_speech_error(
    state: &DesktopState,
    kind: Option<&str>,
    original: &str,
) -> SpeechErrorPresentation {
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let presentation = localize_speech_error_for_locale(kind, original, locale);
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

pub(crate) fn clear_speech_failure_message(
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

#[cfg(test)]
pub(super) fn denied_permission_message(
    permissions: coosenpai_core::ports::SpeechPermissions,
) -> Option<&'static str> {
    denied_permission_message_for_locale(permissions, Locale::Ja)
}

pub(super) fn denied_permission_message_for_locale(
    permissions: coosenpai_core::ports::SpeechPermissions,
    locale: Locale,
) -> Option<&'static str> {
    match permissions.microphone {
        SpeechPermissionKind::Granted => {}
        SpeechPermissionKind::Denied => return Some(text(TextKey::SpeechMicrophoneDenied, locale)),
        SpeechPermissionKind::Restricted => {
            return Some(text(TextKey::SpeechMicrophoneRestricted, locale))
        }
        SpeechPermissionKind::NotDetermined | SpeechPermissionKind::Unavailable => {
            return Some(text(TextKey::SpeechMicrophoneUnavailable, locale))
        }
    }
    match permissions.recognition {
        SpeechPermissionKind::Granted => None,
        SpeechPermissionKind::Denied => Some(text(TextKey::SpeechRecognitionDenied, locale)),
        SpeechPermissionKind::Restricted => {
            Some(text(TextKey::SpeechRecognitionRestricted, locale))
        }
        SpeechPermissionKind::NotDetermined | SpeechPermissionKind::Unavailable => {
            Some(text(TextKey::SpeechRecognitionUnavailable, locale))
        }
    }
}

