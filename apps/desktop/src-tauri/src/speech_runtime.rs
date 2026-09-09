use super::support::{present_speech_error, wait_for_push_to_talk_end, KeyReleaseOutcome};
use super::SpeechController;

use crate::state::DesktopState;
use std::sync::Arc;

impl SpeechController {
    pub(super) async fn fail_external_error(
        &self,
        state: &Arc<DesktopState>,
        generation: u64,
        kind: Option<&str>,
        original: &str,
    ) {
        let presentation = present_speech_error(state, kind, original);
        self.fail(state, generation, presentation.message).await;
    }

    pub(super) async fn complete_cancel_owner(&self, state: &DesktopState, generation: u64) {
        if !self.lifecycle().is_cancelling(generation) {
            return;
        }
        self.reset_view(state, generation).await;
        if self.lifecycle().complete_cancel(generation) {
            self.cancel_completed.notify_waiters();
        }
    }

    pub(super) async fn wait_for_cancel_owner(&self, generation: u64) {
        loop {
            let notified = self.cancel_completed.notified();
            if !self.lifecycle().is_cancelling(generation) {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn wait_for_idle(&self) {
        loop {
            let notified = self.cancel_completed.notified();
            if self.lifecycle().phase() == "idle" {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn refresh_cancel_shortcut(&self, state: &DesktopState) {
        #[cfg(test)]
        if self.shortcut_refresh_disabled.load(Ordering::Acquire) {
            return;
        }
        let shortcut_state = self.lifecycle().shortcut_state();
        crate::capture::refresh_speech_cancel_shortcut(state, shortcut_state).await;
    }

    pub(super) async fn monitor_key_release(
        self: Arc<Self>,
        state: Arc<DesktopState>,
        generation: u64,
        shortcut: String,
    ) {
        let outcome = wait_for_push_to_talk_end(
            self.key_state.as_ref(),
            &shortcut,
            std::time::Duration::from_millis(50),
            std::time::Duration::from_secs(60),
            || self.lifecycle().monitors_key_release(generation),
        )
        .await;
        match outcome {
            Ok(KeyReleaseOutcome::Finish) => {
                self.dispatch_finish_generation(state, generation).await
            }
            Ok(KeyReleaseOutcome::Inactive) => {}
            Err(error) => {
                self.dispatch_finish_generation(state.clone(), generation)
                    .await;
                let original = error.to_string();
                let message = present_speech_error(&state, Some("key-state"), &original)
                    .message
                    .to_owned();
                if self.lifecycle().is_current(generation) {
                    state
                        .publish_event(crate::snapshot_presenter::SnapshotEvent::Speech {
                            generation,
                            event: crate::speech_presenter::SpeechResult::KeyStateFailed(
                                message.clone(),
                            ),
                        })
                        .await;
                    crate::capture::publish_speech_transient_shortcut_error(
                        state, generation, message,
                    )
                    .await;
                }
            }
        }
    }

    async fn dispatch_finish_generation(
        self: &Arc<Self>,
        state: Arc<DesktopState>,
        generation: u64,
    ) {
        state.ui.input(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::CaptureCompleted(Box::new(
                crate::capture::CaptureEvent::VoiceReleaseObserved { generation },
            )),
        );
    }
}
