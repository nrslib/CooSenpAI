use super::*;

impl SpeechController {
    pub(super) fn lifecycle(&self) -> std::sync::MutexGuard<'_, SpeechLifecycle> {
        match self.lifecycle.lock() {
            Ok(lifecycle) => lifecycle,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(super) fn transcript(&self) -> std::sync::MutexGuard<'_, SpeechTranscript> {
        match self.transcript.lock() {
            Ok(transcript) => transcript,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(crate) fn resource_phase(&self) -> crate::command_guard::ResourcePhase {
        match self.lifecycle().phase() {
            "idle" => crate::command_guard::ResourcePhase::Idle,
            "starting" | "cancelling" | "cleaning" => {
                crate::command_guard::ResourcePhase::Transitioning
            }
            "recording" | "finalizing" | "confirming" | "sending" => {
                crate::command_guard::ResourcePhase::Active
            }
            _ => crate::command_guard::ResourcePhase::Transitioning,
        }
    }

    pub(crate) fn accepts_transient_shortcut_error(&self, generation: u64) -> bool {
        let lifecycle = self.lifecycle();
        lifecycle.is_current(generation) && !lifecycle.can_apply_cleanup(generation)
    }
}
