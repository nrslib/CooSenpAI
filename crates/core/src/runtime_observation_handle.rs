use super::*;

impl RuntimeHandle {
    pub async fn observe(
        &self,
        frames: Vec<ObservationFrameInput>,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.observe_cancellable(frames, self.cancellation.child_token())
            .await
    }

    pub async fn observe_cancellable(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.observe_cancellable_with_delivery(frames, cancellation, true)
            .await
    }

    pub async fn observe_without_companion_delivery(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.observe_cancellable_with_delivery(frames, cancellation, false)
            .await
    }

    async fn observe_cancellable_with_delivery(
        &self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
        allow_companion_delivery: bool,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.ensure_open()?;
        let generation = self.watch_scope_generation();
        if frames
            .iter()
            .any(|frame| frame.scope_generation != generation)
        {
            return Err(RuntimeError::StaleWatchScope);
        }
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::Observe(ObserveRequest {
                frames,
                audio: Vec::new(),
                allow_companion_delivery,
                cancellation,
                response,
            }))
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn companion_observations(
        &self,
        observations: Vec<ObservationRecord>,
    ) -> Result<CompanionResponse, RuntimeError> {
        self.companion_observations_cancellable(observations, self.cancellation.child_token())
            .await
    }

    pub async fn companion_observations_cancellable(
        &self,
        observations: Vec<ObservationRecord>,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, RuntimeError> {
        self.companion_observations_cancellable_with_result(observations, cancellation)
            .await
            .map(|result| result.response)
    }

    pub async fn companion_observations_with_result(
        &self,
        observations: Vec<ObservationRecord>,
    ) -> Result<CompanionObservationResult, RuntimeError> {
        self.companion_observations_cancellable_with_result(
            observations,
            self.cancellation.child_token(),
        )
        .await
    }

    pub async fn companion_observations_cancellable_with_result(
        &self,
        observations: Vec<ObservationRecord>,
        cancellation: CancellationToken,
    ) -> Result<CompanionObservationResult, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::CompanionObservations {
                observations,
                context_notice: None,
                cancellation,
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn process_companion_mailbox(&self) -> Result<CompanionResponse, RuntimeError> {
        self.process_companion_mailbox_cancellable(self.cancellation.child_token())
            .await
    }

    pub async fn process_companion_mailbox_cancellable(
        &self,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::ProcessCompanionMailbox {
                cancellation,
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn heartbeat(&self) -> Result<ObservationRecord, RuntimeError> {
        self.heartbeat_with_stagnation_cancellable(None, self.cancellation.child_token())
            .await
    }

    pub async fn observe_audio_batch(
        &self,
        audio: Vec<crate::state::AudioObservation>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.ensure_open()?;
        if cancellation.is_cancelled() {
            return Err(RuntimeError::Closed);
        }
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::Observe(ObserveRequest {
                frames: Vec::new(),
                audio,
                allow_companion_delivery: true,
                cancellation,
                response,
            }))
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn heartbeat_cancellable(
        &self,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.heartbeat_with_stagnation_cancellable(None, cancellation)
            .await
    }

    pub async fn heartbeat_with_stagnation_cancellable(
        &self,
        stagnation: Option<crate::state::StagnationObservation>,
        cancellation: CancellationToken,
    ) -> Result<ObservationRecord, RuntimeError> {
        self.ensure_open()?;
        if cancellation.is_cancelled() {
            return Err(RuntimeError::Closed);
        }
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::Heartbeat {
                stagnation,
                cancellation,
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn companion_nudge(
        &self,
        observation: ObservationRecord,
        context_notice: String,
    ) -> Result<CompanionResponse, RuntimeError> {
        self.companion_nudge_cancellable(
            observation,
            context_notice,
            self.cancellation.child_token(),
        )
        .await
    }

    pub async fn companion_nudge_cancellable(
        &self,
        observation: ObservationRecord,
        context_notice: String,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, RuntimeError> {
        self.companion_nudge_cancellable_with_result(observation, context_notice, cancellation)
            .await
            .map(|result| result.response)
    }

    pub async fn companion_nudge_with_result(
        &self,
        observation: ObservationRecord,
        context_notice: String,
    ) -> Result<CompanionObservationResult, RuntimeError> {
        self.companion_nudge_cancellable_with_result(
            observation,
            context_notice,
            self.cancellation.child_token(),
        )
        .await
    }

    pub async fn companion_nudge_cancellable_with_result(
        &self,
        observation: ObservationRecord,
        context_notice: String,
        cancellation: CancellationToken,
    ) -> Result<CompanionObservationResult, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::CompanionObservations {
                observations: vec![observation],
                context_notice: Some(context_notice),
                cancellation,
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }
}
