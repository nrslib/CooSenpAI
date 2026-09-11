use crate::snapshot::SpeechView;
pub(crate) use crate::speech_lifecycle::SpeechSource;
use crate::speech_lifecycle::{FinalOutcome, SessionOutcome, SpeechLifecycle, StartOutcome};
use crate::speech_presenter::SpeechResult;
use crate::speech_transcript::SpeechTranscript;
use crate::state::DesktopState;
use coosenpai_core::config::ConfigPaths;
use coosenpai_core::locale::{localize_audio_message, text as locale_text, Locale, TextKey};
use coosenpai_core::ports::{
    SpeechEvent, SpeechInputDevice, SpeechInputDevicePort, SpeechKeyStatePort,
    SpeechPermissionPort, SpeechPort,
};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[path = "speech_state.rs"]
mod controller_state;
#[path = "speech_devices.rs"]
mod devices;
#[path = "speech_diagnostics.rs"]
pub(crate) mod diagnostics;
#[path = "speech_helper.rs"]
mod helper;
#[path = "speech_runtime.rs"]
mod runtime;
#[path = "speech_support.rs"]
pub(crate) mod support;
use diagnostics::{log_stage, SpeechStage};
pub(crate) use support::permission_name;
use support::{
    complete_callback_send, denied_permission_message_for_locale, present_speech_error,
    send_chat_from_callback, wait_for_cancel,
};

pub struct SpeechController {
    speech_port: Mutex<Option<Arc<dyn SpeechPort>>>,
    permission_port: Mutex<Arc<dyn SpeechPermissionPort>>,
    key_state: Arc<dyn SpeechKeyStatePort>,
    input_devices: Arc<dyn SpeechInputDevicePort>,
    pub(crate) lifecycle: Arc<Mutex<SpeechLifecycle>>,
    transcript: Mutex<SpeechTranscript>,
    projection: tokio::sync::Mutex<()>,
    cancel_completed: Notify,
    #[cfg(test)]
    shortcut_refresh_disabled: std::sync::atomic::AtomicBool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeechPopupSnapshot {
    pub revision: u64,
    pub companion_display_name: String,
    pub speech: SpeechView,
    pub theme: String,
    pub font: String,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_image_png: Option<Vec<u8>>,
}

impl SpeechPopupSnapshot {
    pub(crate) fn from_app(snapshot: &crate::snapshot::AppSnapshot) -> Self {
        Self {
            revision: snapshot.revision,
            companion_display_name: snapshot.companion_display_name.clone(),
            speech: snapshot.speech.clone(),
            theme: snapshot.config.ui.theme.clone(),
            font: snapshot.config.ui.font.clone(),
            language: snapshot.config.ui.language.clone(),
            avatar_color: snapshot.config.ui.avatar_color.clone(),
            avatar_image_png: snapshot.avatar_image_png.clone(),
        }
    }
}

impl SpeechController {
    pub fn new(paths: &ConfigPaths) -> Self {
        let executable_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(ToOwned::to_owned));
        let helper = match helper::resolve(executable_dir.as_deref(), &paths.root) {
            Ok(helper) => helper,
            Err(error) => {
                eprintln!("音声 E2E helper を拒否しました: {error}");
                None
            }
        };
        Self {
            speech_port: Mutex::new(helper.map(crate::platform::speech_port)),
            permission_port: Mutex::new(Arc::new(crate::platform::MacSpeechPermissions)),
            key_state: crate::platform::speech_key_state(),
            input_devices: crate::platform::speech_input_devices(),
            lifecycle: Arc::new(Mutex::new(SpeechLifecycle::default())),
            transcript: Mutex::new(SpeechTranscript::default()),
            projection: tokio::sync::Mutex::new(()),
            cancel_completed: Notify::new(),
            #[cfg(test)]
            shortcut_refresh_disabled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn with_ports(
        key_state: Arc<dyn SpeechKeyStatePort>,
        input_devices: Arc<dyn SpeechInputDevicePort>,
    ) -> Self {
        Self {
            speech_port: Mutex::new(None),
            permission_port: Mutex::new(Arc::new(crate::platform::MacSpeechPermissions)),
            key_state,
            input_devices,
            lifecycle: Arc::new(Mutex::new(SpeechLifecycle::default())),
            transcript: Mutex::new(SpeechTranscript::default()),
            projection: tokio::sync::Mutex::new(()),
            cancel_completed: Notify::new(),
            #[cfg(test)]
            shortcut_refresh_disabled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn install_ports_for_test(
        &self,
        speech_port: Arc<dyn SpeechPort>,
        permission_port: Arc<dyn SpeechPermissionPort>,
    ) {
        *self.speech_port.lock().expect("speech port") = Some(speech_port);
        *self.permission_port.lock().expect("permission port") = permission_port;
    }

    pub(super) async fn begin(
        self: &Arc<Self>,
        state: Arc<DesktopState>,
        permit: &crate::command_guard::CommandContext,
        source: SpeechSource,
    ) -> Result<(), String> {
        let voice_output = state.voice_output.clone();
        let _voice_start = voice_output.start_gate.lock().await;
        voice_output.stop().await;
        let locale = Locale::from_config(&state.runtime_config().ui.language);
        let phase = self.lifecycle().phase();
        if phase != "idle" {
            return match phase {
                "cancelling" | "cleaning" => {
                    Err(locale_text(TextKey::SpeechEnding, locale).to_owned())
                }
                _ => Ok(()),
            };
        }
        let cancellation = state.cancellation.child_token();
        let command_generation = permit
            .fence(crate::command_guard::GenerationResource::Speech)
            .ok_or_else(|| locale_text(TextKey::SpeechGenerationMissing, locale).to_owned())?;
        let Some(generation) = self
            .lifecycle()
            .start_with_generation(cancellation.clone(), command_generation.value)
        else {
            return Err(locale_text(TextKey::SpeechStartFailed, locale).to_owned());
        };
        self.transcript().begin(generation);
        log_stage(
            state.logger.as_ref(),
            generation,
            SpeechStage::Starting(source),
        );
        if source == SpeechSource::Shortcut {
            let config = state.runtime_config();
            if config.speech.mode == "pushToTalk" {
                if let Some(shortcut) = config.keymap.microphone {
                    let controller = self.clone();
                    let poll_state = state.clone();
                    tauri::async_runtime::spawn(async move {
                        controller
                            .monitor_key_release(poll_state, generation, shortcut)
                            .await;
                    });
                }
            }
        }
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            controller
                .start_after_transition(state, source, generation, cancellation)
                .await;
        });
        Ok(())
    }

    async fn start_after_transition(
        self: Arc<Self>,
        state: Arc<DesktopState>,
        source: SpeechSource,
        generation: u64,
        cancellation: CancellationToken,
    ) {
        let locale = Locale::from_config(&state.runtime_config().ui.language);
        if !self.continue_start(&state, generation).await {
            return;
        }
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                generation,
                event: SpeechResult::Started(source),
            })
            .await;
        self.refresh_cancel_shortcut(&state).await;
        if !self.continue_start(&state, generation).await {
            return;
        }
        let permission_port = self
            .permission_port
            .lock()
            .expect("permission port")
            .clone();
        log_stage(
            state.logger.as_ref(),
            generation,
            SpeechStage::PermissionRequest,
        );
        let permissions = match permission_port.request(cancellation.clone()).await {
            Ok(permissions) => permissions,
            Err(error) => {
                if self.lifecycle().is_cancelling(generation) {
                    self.complete_cancel_owner(&state, generation).await;
                } else {
                    let original = error.to_string();
                    self.fail_external_error(&state, generation, None, &original)
                        .await;
                }
                return;
            }
        };
        log_stage(
            state.logger.as_ref(),
            generation,
            SpeechStage::Permissions(permissions),
        );
        if !self.continue_start(&state, generation).await {
            return;
        }
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                generation,
                event: SpeechResult::Permissions(permissions),
            })
            .await;
        if let Some(message) = denied_permission_message_for_locale(permissions, locale) {
            self.fail(&state, generation, message).await;
            return;
        }
        let speech = self.speech_port.lock().expect("speech port").clone();
        let Some(speech) = speech else {
            self.fail(
                &state,
                generation,
                locale_text(TextKey::SpeechHelperMissing, locale),
            )
            .await;
            return;
        };
        let config = state.runtime_config();
        let speech_locale = config.speech.locale;
        let (input_device, device_warning) =
            self.resolve_input_device_for_locale(&config.speech.input_device, locale);
        log_stage(state.logger.as_ref(), generation, SpeechStage::HelperStart);
        let mut session = match speech
            .start(&speech_locale, &input_device, cancellation.clone())
            .await
        {
            Ok(session) => session,
            Err(error) => {
                if self.lifecycle().is_cancelling(generation) {
                    self.complete_cancel_owner(&state, generation).await;
                } else {
                    let original = error.to_string();
                    self.fail_external_error(&state, generation, None, &original)
                        .await;
                }
                return;
            }
        };
        let control = session.control();
        let session_outcome = {
            self.lifecycle()
                .attach_session(generation, cancellation, control)
        };
        match session_outcome {
            SessionOutcome::Active => {}
            SessionOutcome::Finish(control) => {
                let _ = control.finish().await;
            }
            SessionOutcome::Cancel(control) => {
                let _ = control.cancel().await;
                self.complete_cancel_owner(&state, generation).await;
                return;
            }
        }
        if let Some(message) = device_warning {
            let message = localize_audio_message("input-device-fallback", &message, locale);
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                    generation,
                    event: SpeechResult::DeviceFallback(message.clone()),
                })
                .await;
            crate::capture::publish_speech_transient_shortcut_error(
                state.clone(),
                generation,
                message,
            )
            .await;
        }
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = session.next_event().await {
                match event {
                    Ok(event) => {
                        if controller
                            .handle_event(&state, generation, source, event)
                            .await
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if controller.lifecycle().is_cancelling(generation) {
                            controller.complete_cancel_owner(&state, generation).await;
                        } else {
                            let original = error.to_string();
                            controller
                                .fail_external_error(&state, generation, None, &original)
                                .await;
                        }
                        return;
                    }
                }
            }
            controller.fail_without_final(&state, generation).await;
        });
    }

    pub(super) fn finish(
        self: &Arc<Self>,
        state: Arc<DesktopState>,
        _permit: &crate::command_guard::CommandContext,
    ) {
        let Some(outcome) = self.lifecycle().finish() else {
            return;
        };
        let generation = outcome.generation;
        let control = outcome.control;
        tauri::async_runtime::spawn(async move {
            if let Some(control) = control {
                let _ = control.finish().await;
            }
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                    generation,
                    event: SpeechResult::FinishRequested,
                })
                .await;
        });
    }

    pub(super) async fn cancel_and_wait_for_switch(
        self: &Arc<Self>,
        state: &DesktopState,
    ) -> Result<(), String> {
        loop {
            let settled = self.cancel_completed.notified();
            if self.lifecycle().phase() != "sending" {
                break;
            }
            settled.await;
        }
        if self.lifecycle().phase() == "cleaning" {
            self.wait_for_idle().await;
            return Ok(());
        }
        let already_cancelling = self.lifecycle().cancelling_generation();
        let locale = Locale::from_config(&state.runtime_config().ui.language);
        let outcome = self.lifecycle().cancel();
        if let Some(message) = outcome.message {
            let message = message.text(locale);
            if let Some(generation) = already_cancelling {
                self.wait_for_cancel_owner(generation).await;
                return Ok(());
            }
            return Err(message.to_owned());
        }
        if let Some(cancellation) = outcome.cancellation {
            cancellation.cancel();
        }
        if let Some(generation) = outcome.generation {
            if outcome.startup_owned {
                self.complete_cancel_owner(state, generation).await;
                self.wait_for_cancel_owner(generation).await;
            } else {
                wait_for_cancel(outcome.control).await;
                self.complete_cancel_owner(state, generation).await;
            }
        }
        Ok(())
    }

    pub(super) fn lifecycle_generation_is_sending(&self, generation: u64) -> bool {
        self.lifecycle().is_sending(generation)
    }

    pub(super) async fn complete_stale_send(&self, state: &DesktopState, generation: u64) {
        if self.lifecycle().complete(generation) {
            self.reset_view(state, generation).await;
        }
    }

    pub(super) async fn confirm(
        &self,
        state: &Arc<DesktopState>,
        expected_generation: u64,
        text: String,
    ) -> Result<String, String> {
        let locale = Locale::from_config(&state.runtime_config().ui.language);
        let input = text.trim().to_owned();
        if input.is_empty() {
            return Err(locale_text(TextKey::SpeechInputEmpty, locale).to_owned());
        }
        let generation = state
            .dispatch_with_fence(
                crate::command_guard::CommandSource::IpcSpeechPopup,
                crate::command_guard::DesktopCommand::SpeechConfirm,
                crate::command_guard::GenerationStamp {
                    resource: crate::command_guard::GenerationResource::Speech,
                    value: expected_generation,
                },
                |_| async {
                    self.lifecycle().claim_confirmation().ok_or_else(|| {
                        crate::command_guard::DispatchError::handler(locale_text(
                            TextKey::SpeechConfirmationMissing,
                            locale,
                        ))
                    })
                },
            )
            .await
            .map_err(|error| error.format_for_locale(locale))?;
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                generation,
                event: SpeechResult::SubmitRequested,
            })
            .await;
        self.refresh_cancel_shortcut(state).await;
        match send_chat_from_callback(self, state, generation, input).await {
            Ok(id) => {
                if self.lifecycle().complete(generation) {
                    self.reset_view(state, generation).await;
                }
                Ok(id)
            }
            Err(error) => {
                let message = present_speech_error(state, None, &error).message;
                self.restore_confirmation_failure(state, generation, message)
                    .await;
                Err(message.to_owned())
            }
        }
    }

    pub async fn popup_snapshot(&self, state: &DesktopState) -> SpeechPopupSnapshot {
        let snapshot = state.snapshot().await;
        SpeechPopupSnapshot::from_app(&snapshot)
    }

    async fn continue_start(&self, state: &DesktopState, generation: u64) -> bool {
        let outcome = { self.lifecycle().continue_start(generation) };
        match outcome {
            StartOutcome::Continue => true,
            StartOutcome::FinishBeforeStart => {
                self.reset_view(state, generation).await;
                false
            }
            StartOutcome::Stale => {
                if self.lifecycle().is_cancelling(generation) {
                    self.complete_cancel_owner(state, generation).await;
                }
                false
            }
        }
    }

    async fn handle_event(
        &self,
        state: &Arc<DesktopState>,
        generation: u64,
        source: SpeechSource,
        event: SpeechEvent,
    ) -> bool {
        if !self.lifecycle().accepts_session_events(generation) {
            return true;
        }
        match event {
            SpeechEvent::Ready {
                locale: _,
                microphone,
                recognition,
                engine: _,
            } => {
                let snapshot = state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                        generation,
                        event: SpeechResult::SessionStarted {
                            microphone,
                            recognition,
                        },
                    })
                    .await;
                if snapshot.speech.generation == generation
                    && snapshot.speech.phase == "recording"
                    && self.lifecycle().accepts_session_events(generation)
                {
                    log_stage(state.logger.as_ref(), generation, SpeechStage::Recording);
                }
                false
            }
            SpeechEvent::Partial { text } => {
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                        generation,
                        event: SpeechResult::Partial(text),
                    })
                    .await;
                false
            }
            SpeechEvent::Warning { kind, message } => {
                let locale = Locale::from_config(&state.runtime_config().ui.language);
                let message = localize_audio_message(&kind, &message, locale);
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                        generation,
                        event: SpeechResult::Warning {
                            kind: kind.clone(),
                            message: message.clone(),
                        },
                    })
                    .await;
                crate::capture::publish_speech_transient_shortcut_error(
                    state.clone(),
                    generation,
                    message,
                )
                .await;
                false
            }
            SpeechEvent::Final { text } => {
                log_stage(
                    state.logger.as_ref(),
                    generation,
                    SpeechStage::FinalReceived {
                        chars: text.chars().count(),
                    },
                );
                self.accept_final(state, generation, source, text).await;
                true
            }
            SpeechEvent::Error { kind, message } => {
                if kind == "permission-microphone" {
                    state
                        .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                            generation,
                            event: SpeechResult::PermissionError(kind.clone()),
                        })
                        .await;
                }
                if kind == "permission-speech" {
                    state
                        .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                            generation,
                            event: SpeechResult::PermissionError(kind.clone()),
                        })
                        .await;
                }
                self.fail_external_error(state, generation, Some(&kind), &message)
                    .await;
                true
            }
            SpeechEvent::Closed => {
                self.fail_without_final(state, generation).await;
                true
            }
        }
    }

    async fn accept_final(
        &self,
        state: &Arc<DesktopState>,
        generation: u64,
        source: SpeechSource,
        text: String,
    ) {
        let Some(text) = self.transcript().resolve_final(generation, &text) else {
            let locale = Locale::from_config(&state.runtime_config().ui.language);
            self.fail(
                state,
                generation,
                locale_text(TextKey::SpeechNoSpeech, locale),
            )
            .await;
            return;
        };
        let confirm_before_send = state.runtime_config().speech.confirm_before_send;
        let (reply, response) = tokio::sync::oneshot::channel();
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                generation,
                event: SpeechResult::FinalResolved {
                    source,
                    confirm_before_send,
                    reply,
                },
            })
            .await;
        let Ok(Some(plan)) = response.await else {
            return;
        };
        let Some(outcome) = self.lifecycle().claim_final_outcome(generation, plan) else {
            return;
        };
        let chars = text.chars().count();
        match outcome {
            FinalOutcome::Composer => {
                if self.lifecycle().can_apply_cleanup(generation) {
                    state.ui.input(
                        crate::ui_events::UiView::Application,
                        crate::ui_events::UiEvent::CaptureCompleted(Box::new(
                            crate::capture::CaptureEvent::Ui(
                                crate::ui_events::UiEvent::SpeechTranscript { generation, text },
                            ),
                        )),
                    );
                }
                if self.lifecycle().can_apply_cleanup(generation) {
                    self.reset_view(state, generation).await;
                }
            }
            FinalOutcome::Confirm => {
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                        generation,
                        event: SpeechResult::Confirmed(text),
                    })
                    .await;
                if self.lifecycle().is_confirming(generation) {
                    log_stage(
                        state.logger.as_ref(),
                        generation,
                        SpeechStage::Confirming { chars },
                    );
                }
                self.refresh_cancel_shortcut(state).await;
            }
            FinalOutcome::Send => {
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                        generation,
                        event: SpeechResult::AutoSubmitStarted,
                    })
                    .await;
                self.refresh_cancel_shortcut(state).await;
                let result = complete_callback_send(
                    move || send_chat_from_callback(self, state, generation, text),
                    |result| async move {
                        match result {
                            Err(error) => {
                                let message = present_speech_error(state, None, &error).message;
                                self.fail(state, generation, message).await;
                            }
                            Ok(_) => {
                                if self.lifecycle().complete(generation) {
                                    self.reset_view(state, generation).await;
                                }
                            }
                        }
                    },
                )
                .await;
                state
                    .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                        generation,
                        event: SpeechResult::CallbackSent(result),
                    })
                    .await;
            }
        }
    }

    async fn fail_without_final(&self, state: &Arc<DesktopState>, generation: u64) {
        if self.lifecycle().accepts_session_events(generation) {
            let locale = Locale::from_config(&state.runtime_config().ui.language);
            self.fail(
                state,
                generation,
                locale_text(TextKey::SpeechGenericFailure, locale),
            )
            .await;
        }
    }

    async fn restore_confirmation_failure(
        &self,
        state: &Arc<DesktopState>,
        generation: u64,
        message: &str,
    ) {
        if !self.lifecycle().restore_confirmation(generation) {
            return;
        }
        self.cancel_completed.notify_waiters();
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                generation,
                event: SpeechResult::ConfirmationFailed(message.to_owned()),
            })
            .await;
    }

    async fn fail(&self, state: &Arc<DesktopState>, generation: u64, message: &str) {
        if !self.lifecycle().complete(generation) {
            return;
        }
        log_stage(state.logger.as_ref(), generation, SpeechStage::Failed);
        let _projection = self.projection.lock().await;
        if !self.lifecycle().can_apply_cleanup(generation) {
            return;
        }
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                generation,
                event: SpeechResult::Failed(message.to_owned()),
            })
            .await;
        self.refresh_cancel_shortcut(state).await;
        if self.lifecycle().complete_cleanup(generation) {
            self.cancel_completed.notify_waiters();
        }
    }

    async fn reset_view(&self, state: &DesktopState, generation: u64) {
        let _projection = self.projection.lock().await;
        if !self.lifecycle().can_apply_cleanup(generation) {
            return;
        }
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                generation,
                event: SpeechResult::Cleaned,
            })
            .await;
        self.refresh_cancel_shortcut(state).await;
        if self.lifecycle().complete_cleanup(generation) {
            self.cancel_completed.notify_waiters();
        }
    }
}

