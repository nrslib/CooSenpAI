use super::DesktopState;
use std::sync::Arc;

impl DesktopState {
    pub(crate) fn audio_session_needs_stop(
        previous: &coosenpai_core::config::Config,
        next: &coosenpai_core::config::Config,
    ) -> bool {
        previous.audio.enabled
            && (!next.audio.enabled
                || previous.audio.mic != next.audio.mic
                || previous.audio.speaker != next.audio.speaker
                || previous.audio.speaker_identification != next.audio.speaker_identification
                || previous.observer.hearing.interval_ms != next.observer.hearing.interval_ms
                || previous.audio.debug_dump_dir != next.audio.debug_dump_dir)
    }

    pub(crate) fn sync_audio(self: &Arc<Self>) {
        self.hearing.sync(self.clone());
    }

    pub(crate) fn activate_runtime(self: &Arc<Self>) {
        self.runtime_active
            .store(true, std::sync::atomic::Ordering::Release);
        self.sync_audio();
    }

    pub(crate) async fn cancel_audio_and_wait(&self) {
        self.hearing.cancel_and_wait(self).await;
    }

    pub(crate) async fn delete_conversation_log_day(
        self: &Arc<Self>,
        paths: coosenpai_core::config::ConfigPaths,
        date: chrono::NaiveDate,
    ) -> Result<(), coosenpai_core::runtime::RuntimeError> {
        let result = self
            .hearing
            .delete_conversation_log_day(self, paths, date)
            .await;
        self.sync_audio();
        result
    }

    pub(crate) async fn cancel_audio(&self) {
        self.hearing.cancel(self).await;
    }

    pub(crate) async fn deactivate_runtime(&self) {
        self.runtime_active
            .store(false, std::sync::atomic::Ordering::Release);
        self.voice_output.stop().await;
        self.cancel_audio_and_wait().await;
    }

}
