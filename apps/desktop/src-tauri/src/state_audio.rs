use super::DesktopState;
use std::sync::Arc;

impl DesktopState {
    pub(crate) fn audio_session_needs_stop(
        previous: &coosenpai_core::config::Config,
        next: &coosenpai_core::config::Config,
    ) -> bool {
        previous.audio.enabled
            && (!next.audio.enabled
                || previous.audio.speaker != next.audio.speaker
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

    pub(crate) async fn deactivate_runtime(&self) {
        self.runtime_active
            .store(false, std::sync::atomic::Ordering::Release);
        self.cancel_audio_and_wait().await;
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub async fn install_hearing_port_for_test(
        &self,
        port: std::sync::Arc<dyn coosenpai_core::ports::HearingPort>,
    ) {
        self.hearing.install_hearing_port_for_test(port).await;
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub async fn install_audio_ingestion_barrier_for_test(
        &self,
        entered: std::sync::Arc<tokio::sync::Notify>,
        release: std::sync::Arc<tokio::sync::Notify>,
    ) {
        self.hearing
            .install_audio_ingestion_barrier_for_test(entered, release)
            .await;
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub async fn install_audio_terminal_barrier_for_test(
        &self,
        received: std::sync::Arc<tokio::sync::Notify>,
        release: std::sync::Arc<tokio::sync::Notify>,
    ) {
        self.hearing
            .install_audio_terminal_barrier_for_test(received, release)
            .await;
    }
}
