use crate::snapshot::SpeechView;
use crate::speech::support::{
    apply_confirmation_failure, apply_failure, apply_warning, clear_speech_failure_message,
};
use crate::speech::SpeechSource;
use crate::speech_lifecycle::SpeechLifecycle;
use crate::ui_events::{UiEffect, UiEvent, UiTask};
use coosenpai_core::locale::{localize_audio_message, Locale};
use coosenpai_core::ports::{SpeechPermissionKind, SpeechPermissions};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug)]
pub(crate) enum SpeechResult {
    FinalResolved {
        source: SpeechSource,
        confirm_before_send: bool,
        reply: tokio::sync::oneshot::Sender<Option<crate::speech_lifecycle::FinalOutcome>>,
    },
    CallbackSent(Result<String, String>),
    Started(SpeechSource),
    Permissions(SpeechPermissions),
    DeviceFallback(String),
    FinishRequested,
    SubmitRequested,
    SessionStarted {
        microphone: SpeechPermissionKind,
        recognition: SpeechPermissionKind,
    },
    Partial(String),
    Warning {
        kind: String,
        message: String,
    },
    PermissionError(String),
    Confirmed(String),
    AutoSubmitStarted,
    ConfirmationFailed(String),
    Failed(String),
    Cleaned,
    KeyStateFailed(String),
}

pub(crate) struct SpeechPresenter {
    lifecycle: Arc<Mutex<SpeechLifecycle>>,
    failure_id: u64,
    failure_message: String,
}

impl SpeechPresenter {
    pub(crate) fn new(lifecycle: Arc<Mutex<SpeechLifecycle>>) -> Self {
        Self {
            lifecycle,
            failure_id: 0,
            failure_message: String::new(),
        }
    }

    pub(crate) fn input_started(&mut self) {
        self.lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .input_started();
    }

    pub(crate) fn handle(
        &mut self,
        view: &mut SpeechView,
        locale: Locale,
        generation: u64,
        event: SpeechResult,
    ) -> Option<Vec<UiEffect>> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut effects = Vec::new();
        match event {
            SpeechResult::FinalResolved {
                source,
                confirm_before_send,
                reply,
            } => {
                let plan = lifecycle
                    .accepts_session_events(generation)
                    .then(|| final_outcome(source, confirm_before_send));
                let _ = reply.send(plan);
                return Some(Vec::new());
            }
            SpeechResult::CallbackSent(result)
                if lifecycle.accept_callback_completion(generation) =>
            {
                let label = if result.is_ok() { "受理" } else { "失敗" };
                return Some(vec![
                    UiEffect::Log(format!("送信の{label}にあわせてメイン画面を前面に出します")),
                    UiEffect::Deliver {
                        child: crate::ui_events::PresenterId::Capture,
                        event: UiEvent::CaptureCompleted(Box::new(
                            crate::capture::CaptureEvent::Ui(UiEvent::OpenMain),
                        )),
                    },
                ]);
            }

            SpeechResult::Started(source) if lifecycle.is_current(generation) => {
                view.phase = "starting".to_owned();
                view.generation = generation;
                view.partial.clear();
                view.warning_kind = None;
                view.message = None;
                view.source = Some(source.as_str().to_owned());
            }
            SpeechResult::Permissions(permissions)
                if lifecycle.is_current(generation) && view.generation == generation =>
            {
                view.microphone_permission = crate::speech::permission_name(permissions.microphone);
                view.recognition_permission =
                    crate::speech::permission_name(permissions.recognition);
            }
            SpeechResult::DeviceFallback(message) if lifecycle.is_current(generation) => {
                apply_warning(view, "input-device-fallback", message);
            }
            SpeechResult::FinishRequested
                if lifecycle.is_finalizing(generation) && view.generation == generation =>
            {
                view.phase = "finalizing".to_owned()
            }
            SpeechResult::SubmitRequested
                if lifecycle.is_sending(generation) && view.generation == generation =>
            {
                view.phase = "sending".to_owned();
                view.message = None;
            }
            SpeechResult::SessionStarted {
                microphone,
                recognition,
            } if lifecycle.accepts_session_events(generation)
                && view.generation == generation
                && view.phase != "idle" =>
            {
                view.phase = "recording".to_owned();
                view.microphone_permission = crate::speech::permission_name(microphone);
                view.recognition_permission = crate::speech::permission_name(recognition);
            }
            SpeechResult::Partial(text)
                if lifecycle.accepts_session_events(generation)
                    && view.generation == generation
                    && view.phase != "idle" =>
            {
                view.partial = text
            }
            SpeechResult::Warning { kind, message }
                if lifecycle.accepts_session_events(generation) =>
            {
                view.message = Some(localize_audio_message(&kind, &message, locale));
                view.warning_kind = Some(kind);
            }
            SpeechResult::PermissionError(kind) if lifecycle.accepts_session_events(generation) => {
                match kind.as_str() {
                    "permission-microphone" => view.microphone_permission = "denied".to_owned(),
                    "permission-speech" => view.recognition_permission = "denied".to_owned(),
                    _ => return None,
                }
            }
            SpeechResult::Confirmed(text) if lifecycle.is_confirming(generation) => {
                view.phase = "confirming".to_owned();
                view.partial = text;
            }
            SpeechResult::AutoSubmitStarted
                if lifecycle.is_sending(generation) && view.generation == generation =>
            {
                view.phase = "sending".to_owned()
            }
            SpeechResult::ConfirmationFailed(message)
                if lifecycle.is_confirming(generation) && view.generation == generation =>
            {
                apply_confirmation_failure(view, generation, &message);
                self.failure_message = message;
                self.failure_id = self.failure_id.saturating_add(1);
                effects.push(failure_timeout(generation, self.failure_id));
            }
            SpeechResult::Failed(message) if lifecycle.can_apply_cleanup(generation) => {
                apply_failure(view, &message);
                self.failure_message = message;
                self.failure_id = self.failure_id.saturating_add(1);
                effects.push(failure_timeout(generation, self.failure_id));
            }
            SpeechResult::Cleaned
                if lifecycle.can_apply_cleanup(generation) && view.generation == generation =>
            {
                view.phase = "idle".to_owned();
                view.partial.clear();
                view.warning_kind = None;
                view.message = None;
                view.source = None;
                self.failure_id = self.failure_id.saturating_add(1);
            }
            SpeechResult::KeyStateFailed(message) if lifecycle.is_current(generation) => {
                view.warning_kind = Some("key-state".to_owned());
                view.message = Some(message);
            }
            _ => return None,
        }
        Some(effects)
    }

    pub(crate) fn expire(&self, view: &mut SpeechView, generation: u64, failure_id: u64) -> bool {
        clear_speech_failure_message(
            view,
            generation,
            &self.failure_message,
            self.failure_id,
            failure_id,
        )
    }
}

fn failure_timeout(generation: u64, failure_id: u64) -> UiEffect {
    UiEffect::Spawn(UiTask::Delay {
        duration: Duration::from_secs(3),
        event: UiEvent::SpeechFailureExpired {
            generation,
            failure_id,
        },
    })
}

pub(crate) fn final_outcome(
    source: SpeechSource,
    confirm_before_send: bool,
) -> crate::speech_lifecycle::FinalOutcome {
    use crate::speech_lifecycle::FinalOutcome;
    match source {
        SpeechSource::Composer => FinalOutcome::Composer,
        SpeechSource::Shortcut if confirm_before_send => FinalOutcome::Confirm,
        SpeechSource::Shortcut => FinalOutcome::Send,
    }
}

