use crate::snapshot::SpeechView;
pub(crate) use crate::speech_lifecycle::SpeechSource;
use crate::speech_lifecycle::{FinalOutcome, SessionOutcome, SpeechLifecycle, StartOutcome};
use crate::speech_transcript::SpeechTranscript;
use crate::state::DesktopState;
use coosenpai_core::config::ConfigPaths;
use coosenpai_core::ports::{
    HelperResolverPort, SpeechEvent, SpeechInputDevice, SpeechInputDevicePort, SpeechKeyStatePort,
    SpeechPermissionPort, SpeechPort,
};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::Emitter;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[path = "speech_state.rs"]
mod controller_state;
#[path = "speech_devices.rs"]
mod devices;
#[path = "speech_runtime.rs"]
mod runtime;
#[path = "speech_support.rs"]
mod support;
pub(crate) use support::permission_name;
use support::{
    apply_confirmation_failure, apply_failure, apply_speech_completion_foreground, apply_warning,
    denied_permission_message, hide_popup, present_speech_error, schedule_speech_failure_clear,
    send_chat, send_chat_from_callback, show_popup, wait_for_cancel, NO_SPEECH_FAILURE_MESSAGE,
};

pub struct SpeechController {
    speech_port: Mutex<Option<Arc<dyn SpeechPort>>>,
    permission_port: Mutex<Arc<dyn SpeechPermissionPort>>,
    key_state: Arc<dyn SpeechKeyStatePort>,
    input_devices: Arc<dyn SpeechInputDevicePort>,
    lifecycle: Mutex<SpeechLifecycle>,
    transcript: Mutex<SpeechTranscript>,
    projection: tokio::sync::Mutex<()>,
    cancel_completed: Notify,
    failure_ids: Arc<AtomicU64>,
    #[cfg(test)]
    shortcut_refresh_disabled: std::sync::atomic::AtomicBool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechPopupSnapshot {
    pub revision: u64,
    pub companion_display_name: String,
    pub speech: SpeechView,
    pub theme: String,
    pub font: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_image_png: Option<Vec<u8>>,
}

impl SpeechController {
    pub fn new(paths: &ConfigPaths) -> Self {
        let executable_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(ToOwned::to_owned));
        let helper = executable_dir.as_deref().and_then(|directory| {
            crate::platform::MacHelperResolver.resolve_speech_helper(directory, &paths.root)
        });
        Self {
            speech_port: Mutex::new(helper.map(crate::platform::speech_port)),
            permission_port: Mutex::new(Arc::new(crate::platform::MacSpeechPermissions)),
            key_state: crate::platform::speech_key_state(),
            input_devices: crate::platform::speech_input_devices(),
            lifecycle: Mutex::new(SpeechLifecycle::default()),
            transcript: Mutex::new(SpeechTranscript::default()),
            projection: tokio::sync::Mutex::new(()),
            cancel_completed: Notify::new(),
            failure_ids: Arc::new(AtomicU64::new(0)),
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
            lifecycle: Mutex::new(SpeechLifecycle::default()),
            transcript: Mutex::new(SpeechTranscript::default()),
            projection: tokio::sync::Mutex::new(()),
            cancel_completed: Notify::new(),
            failure_ids: Arc::new(AtomicU64::new(0)),
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
        let phase = self.lifecycle().phase();
        if phase != "idle" {
            if source == SpeechSource::Shortcut
                && matches!(
                    phase,
                    "starting" | "recording" | "finalizing" | "confirming" | "sending"
                )
            {
                show_popup(&state, true);
            }
            return match phase {
                "cancelling" | "cleaning" => Err("音声入力を終了しています".to_owned()),
                _ => Ok(()),
            };
        }
        let cancellation = state.cancellation.child_token();
        let command_generation = permit
            .fence(crate::command_guard::GenerationResource::Speech)
            .ok_or_else(|| "音声入力の世代がありません".to_owned())?;
        let Some(generation) = self
            .lifecycle()
            .start_with_generation(cancellation.clone(), command_generation.value)
        else {
            return Err("音声入力を開始できません".to_owned());
        };
        self.transcript().begin(generation);
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
        if !self.continue_start(&state, generation).await {
            return;
        }
        state
            .publish(|snapshot| {
                if !self.lifecycle().is_current(generation) {
                    return;
                }
                snapshot.speech.phase = "starting".to_owned();
                snapshot.speech.generation = generation;
                snapshot.speech.partial.clear();
                snapshot.speech.warning_kind = None;
                snapshot.speech.message = None;
                snapshot.speech.source = Some(source.as_str().to_owned());
            })
            .await;
        if source == SpeechSource::Shortcut && self.lifecycle().is_current(generation) {
            show_popup(&state, false);
        }
        if self.lifecycle().is_current(generation) {
            crate::windows::sync_recording(&state.app, true);
        }
        self.refresh_cancel_shortcut(&state).await;
        if !self.continue_start(&state, generation).await {
            return;
        }
        let permission_port = self
            .permission_port
            .lock()
            .expect("permission port")
            .clone();
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
        if !self.continue_start(&state, generation).await {
            return;
        }
        state
            .publish(|snapshot| {
                if self.lifecycle().is_current(generation)
                    && snapshot.speech.generation == generation
                {
                    snapshot.speech.microphone_permission = permission_name(permissions.microphone);
                    snapshot.speech.recognition_permission =
                        permission_name(permissions.recognition);
                }
            })
            .await;
        if let Some(message) = denied_permission_message(permissions) {
            self.fail(&state, generation, message).await;
            return;
        }
        let speech = self.speech_port.lock().expect("speech port").clone();
        let Some(speech) = speech else {
            self.fail(&state, generation, "音声認識 helper が見つかりません")
                .await;
            return;
        };
        let config = state.runtime_config();
        let locale = config.speech.locale;
        let (input_device, device_warning) = self.resolve_input_device(&config.speech.input_device);
        let mut session = match speech
            .start(&locale, &input_device, cancellation.clone())
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
            state
                .publish(|snapshot| {
                    if self.lifecycle().is_current(generation) {
                        apply_warning(
                            &mut snapshot.speech,
                            "input-device-fallback",
                            message.clone(),
                        );
                    }
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
            controller
                .finish_without_text(&state, generation, source)
                .await;
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
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(control) = control {
                let _ = control.finish().await;
            }
            state
                .publish(|snapshot| {
                    if controller.lifecycle().is_finalizing(generation)
                        && snapshot.speech.generation == generation
                    {
                        snapshot.speech.phase = "finalizing".to_owned();
                    }
                })
                .await;
        });
    }

    pub(super) fn cancel(
        self: &Arc<Self>,
        state: Arc<DesktopState>,
        _permit: &crate::command_guard::CommandContext,
    ) -> Result<(), String> {
        let outcome = self.lifecycle().cancel();
        if let Some(message) = outcome.message {
            if let Some(generation) = outcome.generation {
                tauri::async_runtime::spawn(
                    crate::capture::publish_speech_transient_shortcut_error(
                        state,
                        generation,
                        message.to_owned(),
                    ),
                );
            }
            return Err(message.to_owned());
        }
        if !outcome.changed {
            return Ok(());
        }
        let Some(generation) = outcome.generation else {
            return Ok(());
        };
        if let Some(cancellation) = outcome.cancellation {
            cancellation.cancel();
        }
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            wait_for_cancel(outcome.control).await;
            controller.complete_cancel_owner(&state, generation).await;
        });
        Ok(())
    }

    pub(super) async fn cancel_and_wait(self: &Arc<Self>, state: &DesktopState) {
        let _ = self.cancel_and_wait_for_switch(state).await;
    }

    pub(super) async fn cancel_and_wait_for_switch(
        self: &Arc<Self>,
        state: &DesktopState,
    ) -> Result<(), String> {
        if self.lifecycle().phase() == "cleaning" {
            self.wait_for_idle().await;
            return Ok(());
        }
        let already_cancelling = self.lifecycle().cancelling_generation();
        let outcome = self.lifecycle().cancel();
        if let Some(message) = outcome.message {
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

    pub(crate) fn is_recording(&self) -> bool {
        self.lifecycle().is_recording()
    }

    pub(super) fn lifecycle_generation_is_sending(&self, generation: u64) -> bool {
        self.lifecycle().is_sending(generation)
    }

    pub(super) fn confirming_generation(&self) -> Option<u64> {
        self.lifecycle().confirming_generation()
    }

    pub(super) async fn complete_stale_send(&self, state: &DesktopState, generation: u64) {
        if self.lifecycle().complete(generation) {
            self.reset_view(state, generation).await;
        }
    }

    pub(super) async fn confirm(
        &self,
        state: &Arc<DesktopState>,
        context: &crate::command_guard::CommandContext,
        text: String,
    ) -> Result<String, String> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err("音声入力が空です".to_owned());
        }
        let generation = self
            .lifecycle()
            .claim_confirmation()
            .ok_or_else(|| "確認する音声入力がありません".to_owned())?;
        state
            .publish(|snapshot| {
                if self.lifecycle().is_sending(generation)
                    && snapshot.speech.generation == generation
                {
                    snapshot.speech.phase = "sending".to_owned();
                    snapshot.speech.message = None;
                }
            })
            .await;
        self.refresh_cancel_shortcut(state).await;
        match send_chat(state, context, text).await {
            Ok(id) => {
                apply_speech_completion_foreground(true, || {
                    crate::windows::show_main(&state.app);
                });
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
        SpeechPopupSnapshot {
            revision: snapshot.revision,
            companion_display_name: snapshot.companion_display_name,
            speech: snapshot.speech,
            theme: snapshot.config.ui.theme,
            font: snapshot.config.ui.font,
            avatar_color: snapshot.config.ui.avatar_color,
            avatar_image_png: snapshot.avatar_image_png,
        }
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
            } => {
                state
                    .publish(|snapshot| {
                        if self.lifecycle().accepts_session_events(generation)
                            && snapshot.speech.generation == generation
                            && snapshot.speech.phase != "idle"
                        {
                            snapshot.speech.phase = "recording".to_owned();
                            snapshot.speech.microphone_permission = permission_name(microphone);
                            snapshot.speech.recognition_permission = permission_name(recognition);
                        }
                    })
                    .await;
                false
            }
            SpeechEvent::Partial { text } => {
                self.transcript().remember_partial(generation, &text);
                state
                    .publish(|snapshot| {
                        if self.lifecycle().accepts_session_events(generation)
                            && snapshot.speech.generation == generation
                            && snapshot.speech.phase != "idle"
                        {
                            snapshot.speech.partial = text;
                        }
                    })
                    .await;
                false
            }
            SpeechEvent::Warning { kind, message } => {
                state
                    .publish(|snapshot| {
                        if self.lifecycle().accepts_session_events(generation) {
                            apply_warning(&mut snapshot.speech, kind.clone(), message.clone());
                        }
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
                self.accept_final(state, generation, source, text).await;
                true
            }
            SpeechEvent::Error { kind, message } => {
                if kind == "permission-microphone" {
                    state
                        .publish(|snapshot| {
                            if self.lifecycle().accepts_session_events(generation) {
                                snapshot.speech.microphone_permission = "denied".to_owned();
                            }
                        })
                        .await;
                }
                if kind == "permission-speech" {
                    state
                        .publish(|snapshot| {
                            if self.lifecycle().accepts_session_events(generation) {
                                snapshot.speech.recognition_permission = "denied".to_owned();
                            }
                        })
                        .await;
                }
                self.fail_external_error(state, generation, Some(&kind), &message)
                    .await;
                true
            }
            SpeechEvent::Closed => {
                self.finish_without_text(state, generation, source).await;
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
            self.fail(state, generation, NO_SPEECH_FAILURE_MESSAGE)
                .await;
            return;
        };
        let confirm_before_send = state.runtime_config().speech.confirm_before_send;
        let Some(outcome) = self
            .lifecycle()
            .claim_final(generation, source, confirm_before_send)
        else {
            return;
        };
        match outcome {
            FinalOutcome::Composer => {
                if self.lifecycle().can_apply_cleanup(generation) {
                    let _ = state.app.emit("coosenpai:speech:composer-final", &text);
                }
                if self.lifecycle().can_apply_cleanup(generation) {
                    self.reset_view(state, generation).await;
                }
            }
            FinalOutcome::Confirm => {
                state
                    .publish(|snapshot| {
                        if self.lifecycle().is_confirming(generation) {
                            snapshot.speech.phase = "confirming".to_owned();
                            snapshot.speech.partial = text;
                        }
                    })
                    .await;
                if self.lifecycle().is_confirming(generation) {
                    show_popup(state, true);
                }
                if self.lifecycle().is_confirming(generation) {
                    crate::windows::sync_recording(&state.app, false);
                }
                self.refresh_cancel_shortcut(state).await;
            }
            FinalOutcome::Send => {
                state
                    .publish(|snapshot| {
                        if self.lifecycle().is_sending(generation)
                            && snapshot.speech.generation == generation
                        {
                            snapshot.speech.phase = "sending".to_owned();
                        }
                    })
                    .await;
                self.refresh_cancel_shortcut(state).await;
                if let Err(error) = send_chat_from_callback(self, state, generation, text).await {
                    let message = present_speech_error(state, None, &error).message;
                    self.fail(state, generation, message).await;
                } else {
                    apply_speech_completion_foreground(true, || {
                        crate::windows::show_main(&state.app);
                    });
                    if self.lifecycle().complete(generation) {
                        self.reset_view(state, generation).await;
                    }
                }
            }
        }
    }

    async fn finish_without_text(
        &self,
        state: &Arc<DesktopState>,
        generation: u64,
        source: SpeechSource,
    ) {
        if self.lifecycle().accepts_session_events(generation) {
            let fallback = { self.transcript().resolve_closed(generation) };
            if let Some(text) = fallback {
                self.accept_final(state, generation, source, text).await;
            } else {
                self.fail(state, generation, NO_SPEECH_FAILURE_MESSAGE)
                    .await;
            }
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
        let failure_id = self.next_failure_id();
        let snapshot = state
            .publish(|snapshot| {
                if self.lifecycle().is_confirming(generation) {
                    apply_confirmation_failure(&mut snapshot.speech, generation, message);
                }
            })
            .await;
        if snapshot.speech.generation == generation
            && snapshot.speech.phase == "confirming"
            && snapshot.speech.message.as_deref() == Some(message)
        {
            schedule_speech_failure_clear(
                state.clone(),
                generation,
                message.to_owned(),
                self.failure_ids.clone(),
                failure_id,
            );
        }
    }

    fn next_failure_id(&self) -> u64 {
        self.failure_ids
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1)
    }

    async fn fail(&self, state: &Arc<DesktopState>, generation: u64, message: &str) {
        if !self.lifecycle().complete(generation) {
            return;
        }
        let _projection = self.projection.lock().await;
        if !self.lifecycle().can_apply_cleanup(generation) {
            return;
        }
        let failure_id = self.next_failure_id();
        state
            .publish(|snapshot| {
                if !self.lifecycle().can_apply_cleanup(generation) {
                    return;
                }
                apply_failure(&mut snapshot.speech, message);
            })
            .await;
        schedule_speech_failure_clear(
            state.clone(),
            generation,
            message.to_owned(),
            self.failure_ids.clone(),
            failure_id,
        );
        if self.lifecycle().can_apply_cleanup(generation) {
            crate::windows::sync_recording(&state.app, false);
        }
        if self.lifecycle().can_apply_cleanup(generation) {
            hide_popup(state);
        }
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
            .publish(|snapshot| {
                if !self.lifecycle().can_apply_cleanup(generation)
                    || snapshot.speech.generation != generation
                {
                    return;
                }
                snapshot.speech.phase = "idle".to_owned();
                snapshot.speech.partial.clear();
                snapshot.speech.warning_kind = None;
                snapshot.speech.message = None;
                snapshot.speech.source = None;
            })
            .await;
        if self.lifecycle().can_apply_cleanup(generation) {
            crate::windows::sync_recording(&state.app, false);
        }
        if self.lifecycle().can_apply_cleanup(generation) {
            hide_popup(state);
        }
        self.refresh_cancel_shortcut(state).await;
        if self.lifecycle().complete_cleanup(generation) {
            self.cancel_completed.notify_waiters();
        }
    }
}

