use async_trait::async_trait;
use coosenpai_core::companion::CompanionResponse;
use coosenpai_core::config::Config;
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::runtime::{
    CompanionObservationResult, RuntimeError, RuntimeHandle, RuntimeSnapshot,
};
use coosenpai_core::state::{
    AudioObservation, ObservationRecord, PendingFrameContext, StagnationObservation,
};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub(crate) trait CoreRuntimePort: Send + Sync {
    fn config(&self) -> Config;
    fn judge_feed_available(&self) -> bool;
    fn snapshot(&self) -> RuntimeSnapshot;
    fn judge_trace_for_input(&self, input_id: &str) -> Option<coosenpai_core::judge::JudgeTrace>;
    // 観察単位の trace を扱うポート契約として保持するが、現行 UI からは未参照。
    #[allow(dead_code)]
    fn judge_trace_for_observation(
        &self,
        observation_id: &str,
    ) -> Option<coosenpai_core::judge::JudgeTrace>;
    async fn feed_judge_with_event_id(
        &self,
        event_id: String,
        input_id: String,
        sign: coosenpai_core::judge::JudgeFeedSign,
        strength: f64,
    ) -> Result<coosenpai_core::judge::JudgeFeedApplyResult, RuntimeError>;
    async fn correct_judge_feed(
        &self,
        event_id: String,
        input_id: String,
        sign: coosenpai_core::judge::JudgeFeedSign,
        strength: f64,
    ) -> Result<coosenpai_core::judge::JudgeFeedApplyResult, RuntimeError>;
    async fn cancel_judge_feed(
        &self,
        event_id: String,
        input_id: String,
        sign: coosenpai_core::judge::JudgeFeedSign,
        strength: f64,
    ) -> Result<coosenpai_core::judge::JudgeFeedApplyResult, RuntimeError>;
    #[allow(dead_code)]
    fn install_fixture_judge_trace(
        &self,
        observation: &ObservationRecord,
        trace: coosenpai_core::judge::JudgeTrace,
    );
    #[allow(dead_code)]
    async fn companion_observations(
        &self,
        observations: Vec<ObservationRecord>,
    ) -> Result<CompanionResponse, RuntimeError>;
    async fn companion_observations_with_result(
        &self,
        observations: Vec<ObservationRecord>,
    ) -> Result<CompanionObservationResult, RuntimeError>;
    fn subscribe_snapshots(&self) -> tokio::sync::watch::Receiver<RuntimeSnapshot>;
    fn watch_scope_generation(&self) -> u64;
    async fn begin_hearing_session(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<
        (
            String,
            coosenpai_core::hearing_ingestion::HearingAudioIngestion,
        ),
        RuntimeError,
    >;
    async fn delete_conversation_log_day(
        &self,
        paths: coosenpai_core::config::ConfigPaths,
        date: chrono::NaiveDate,
    ) -> Result<(), RuntimeError>;
    fn update_hearing_context(
        &self,
        context: coosenpai_core::hearing_context::HearingContext,
    ) -> Result<bool, RuntimeError>;
    fn register_pending_frame_context(
        &self,
        context: PendingFrameContext,
        publication: Option<&coosenpai_core::persistence::PublicationGate>,
    ) -> Result<(), RuntimeError>;
    fn prepare_pending_frame_contexts(
        &self,
        contexts: Vec<PendingFrameContext>,
    ) -> Result<Option<coosenpai_core::companion_storage::PendingFrameContextChange>, RuntimeError>;
    async fn observe(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError>;
    async fn observe_without_companion_delivery(
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
    ) -> Result<CompanionResponse, RuntimeError>;
    async fn companion_nudge_with_result(
        &self,
        observation: ObservationRecord,
        context_notice: String,
    ) -> Result<CompanionObservationResult, RuntimeError>;
    async fn heartbeat(
        &self,
        stagnation: Option<StagnationObservation>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError>;
    async fn audio_observation(
        &self,
        observations: Vec<AudioObservation>,
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
    async fn delete_conversation_log_day(
        &self,
        paths: coosenpai_core::config::ConfigPaths,
        date: chrono::NaiveDate,
    ) -> Result<(), RuntimeError> {
        RuntimeHandle::delete_conversation_log_day(self, paths, date).await
    }

    async fn begin_hearing_session(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<
        (
            String,
            coosenpai_core::hearing_ingestion::HearingAudioIngestion,
        ),
        RuntimeError,
    > {
        RuntimeHandle::begin_hearing_session(self, generation, cancellation).await
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

    fn judge_feed_available(&self) -> bool {
        !RuntimeHandle::config(self).judge.modules.is_empty()
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeHandle::snapshot(self)
    }

    fn judge_trace_for_input(&self, input_id: &str) -> Option<coosenpai_core::judge::JudgeTrace> {
        RuntimeHandle::judge_trace_for_input(self, input_id)
    }

    fn judge_trace_for_observation(
        &self,
        observation_id: &str,
    ) -> Option<coosenpai_core::judge::JudgeTrace> {
        RuntimeHandle::judge_trace_for_observation(self, observation_id)
    }

    async fn feed_judge_with_event_id(
        &self,
        event_id: String,
        input_id: String,
        sign: coosenpai_core::judge::JudgeFeedSign,
        strength: f64,
    ) -> Result<coosenpai_core::judge::JudgeFeedApplyResult, RuntimeError> {
        RuntimeHandle::feed_judge_with_event_id_result(self, event_id, input_id, sign, strength)
            .await
    }

    async fn correct_judge_feed(
        &self,
        event_id: String,
        input_id: String,
        sign: coosenpai_core::judge::JudgeFeedSign,
        strength: f64,
    ) -> Result<coosenpai_core::judge::JudgeFeedApplyResult, RuntimeError> {
        RuntimeHandle::correct_judge_feed_result(self, event_id, input_id, sign, strength).await
    }

    async fn cancel_judge_feed(
        &self,
        event_id: String,
        input_id: String,
        sign: coosenpai_core::judge::JudgeFeedSign,
        strength: f64,
    ) -> Result<coosenpai_core::judge::JudgeFeedApplyResult, RuntimeError> {
        RuntimeHandle::cancel_judge_feed_result(self, event_id, input_id, sign, strength).await
    }

    fn install_fixture_judge_trace(
        &self,
        observation: &ObservationRecord,
        trace: coosenpai_core::judge::JudgeTrace,
    ) {
        RuntimeHandle::install_fixture_judge_trace(self, observation, trace);
    }

    async fn companion_observations(
        &self,
        observations: Vec<ObservationRecord>,
    ) -> Result<CompanionResponse, RuntimeError> {
        RuntimeHandle::companion_observations(self, observations).await
    }

    async fn companion_observations_with_result(
        &self,
        observations: Vec<ObservationRecord>,
    ) -> Result<CompanionObservationResult, RuntimeError> {
        RuntimeHandle::companion_observations_with_result(self, observations).await
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
        publication: Option<&coosenpai_core::persistence::PublicationGate>,
    ) -> Result<(), RuntimeError> {
        RuntimeHandle::register_pending_frame_context_cancellable(self, context, publication)
    }

    fn prepare_pending_frame_contexts(
        &self,
        contexts: Vec<PendingFrameContext>,
    ) -> Result<Option<coosenpai_core::companion_storage::PendingFrameContextChange>, RuntimeError>
    {
        RuntimeHandle::prepare_pending_frame_contexts(self, contexts)
    }

    async fn observe(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.observe_cancellable(frames, cancellation).await
    }

    async fn observe_without_companion_delivery(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        RuntimeHandle::observe_without_companion_delivery(self, frames, cancellation).await
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
    ) -> Result<CompanionResponse, RuntimeError> {
        RuntimeHandle::companion_nudge_cancellable(self, observation, context_notice, cancellation)
            .await
    }

    async fn companion_nudge_with_result(
        &self,
        observation: ObservationRecord,
        context_notice: String,
    ) -> Result<CompanionObservationResult, RuntimeError> {
        RuntimeHandle::companion_nudge_with_result(self, observation, context_notice).await
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
        observations: Vec<AudioObservation>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.observe_audio_batch(observations, cancellation).await
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
