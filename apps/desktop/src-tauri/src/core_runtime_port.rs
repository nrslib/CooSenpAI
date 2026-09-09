use async_trait::async_trait;
use coosenpai_core::companion::CompanionResponse;
use coosenpai_core::config::Config;
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::runtime::{RuntimeError, RuntimeHandle, RuntimeSnapshot};
use coosenpai_core::state::{
    AudioObservation, ObservationRecord, PendingFrameContext, StagnationObservation,
};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub(crate) trait CoreRuntimePort: Send + Sync {
    fn config(&self) -> Config;
    fn snapshot(&self) -> RuntimeSnapshot;
    fn subscribe_snapshots(&self) -> tokio::sync::watch::Receiver<RuntimeSnapshot>;
    fn watch_scope_generation(&self) -> u64;
    fn begin_hearing_context(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<String, RuntimeError>;
    fn update_hearing_context(
        &self,
        context: coosenpai_core::hearing_context::HearingContext,
    ) -> Result<bool, RuntimeError>;
    fn register_pending_frame_context(
        &self,
        context: PendingFrameContext,
        publication: &coosenpai_core::persistence::PublicationGate,
    ) -> Result<(), RuntimeError>;
    async fn observe(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError>;
    async fn process_mailbox(
        &self,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, RuntimeError>;
    async fn companion_nudge(
        &self,
        observation: ObservationRecord,
        context_notice: String,
        cancellation: CancellationToken,
    ) -> Result<(), RuntimeError>;
    async fn heartbeat(
        &self,
        stagnation: Option<StagnationObservation>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError>;
    async fn audio_observation(
        &self,
        observation: AudioObservation,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError>;
    async fn cancel_user_message(&self) -> Result<String, RuntimeError>;
    async fn retry_user_message(&self) -> Result<String, RuntimeError>;
    async fn cancel_user_message_for(&self, input_id: String) -> Result<String, RuntimeError>;
    async fn retry_user_message_for(&self, input_id: String) -> Result<String, RuntimeError>;
    async fn reset_companion_emotions(&self) -> Result<(), RuntimeError>;
    async fn consolidate_memory(&self, period: String) -> Result<u64, RuntimeError>;
}

#[async_trait]
impl CoreRuntimePort for RuntimeHandle {
    fn begin_hearing_context(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<String, RuntimeError> {
        RuntimeHandle::begin_hearing_context(self, generation, cancellation)
    }

    fn update_hearing_context(
        &self,
        context: coosenpai_core::hearing_context::HearingContext,
    ) -> Result<bool, RuntimeError> {
        RuntimeHandle::update_hearing_context(self, context)
    }

    fn config(&self) -> Config {
        RuntimeHandle::config(self)
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeHandle::snapshot(self)
    }

    fn subscribe_snapshots(&self) -> tokio::sync::watch::Receiver<RuntimeSnapshot> {
        RuntimeHandle::subscribe_snapshots(self)
    }

    fn watch_scope_generation(&self) -> u64 {
        RuntimeHandle::watch_scope_generation(self)
    }

    fn register_pending_frame_context(
        &self,
        context: PendingFrameContext,
        publication: &coosenpai_core::persistence::PublicationGate,
    ) -> Result<(), RuntimeError> {
        RuntimeHandle::register_pending_frame_context_cancellable(self, context, Some(publication))
    }

    async fn observe(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.observe_cancellable(frames, cancellation).await
    }

    async fn process_mailbox(
        &self,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, RuntimeError> {
        self.process_companion_mailbox_cancellable(cancellation)
            .await
    }

    async fn companion_nudge(
        &self,
        observation: ObservationRecord,
        context_notice: String,
        cancellation: CancellationToken,
    ) -> Result<(), RuntimeError> {
        RuntimeHandle::companion_nudge_cancellable(self, observation, context_notice, cancellation)
            .await
            .map(|_| ())
    }

    async fn heartbeat(
        &self,
        stagnation: Option<StagnationObservation>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.heartbeat_with_stagnation_cancellable(stagnation, cancellation)
            .await
    }

    async fn audio_observation(
        &self,
        observation: AudioObservation,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.ingest_audio_observation_cancellable(observation, cancellation)
            .await
    }

    async fn cancel_user_message(&self) -> Result<String, RuntimeError> {
        RuntimeHandle::cancel_user_message(self).await
    }

    async fn retry_user_message(&self) -> Result<String, RuntimeError> {
        RuntimeHandle::retry_user_message(self).await
    }

    async fn cancel_user_message_for(&self, input_id: String) -> Result<String, RuntimeError> {
        RuntimeHandle::cancel_user_message_for(self, input_id).await
    }

    async fn retry_user_message_for(&self, input_id: String) -> Result<String, RuntimeError> {
        RuntimeHandle::retry_user_message_for(self, input_id).await
    }

    async fn consolidate_memory(&self, period: String) -> Result<u64, RuntimeError> {
        RuntimeHandle::consolidate_memory(self, period).await
    }

    async fn reset_companion_emotions(&self) -> Result<(), RuntimeError> {
        RuntimeHandle::reset_companion_emotions(self).await
    }
}
