use super::*;
use crate::locale::{text, Locale, TextKey};

impl RuntimeHandle {
    pub fn hearing_audio_ingestion(
        &self,
        session_id: String,
    ) -> Result<crate::hearing_ingestion::HearingAudioIngestion, RuntimeError> {
        self.ensure_open()?;
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?;
        let preparer = preparer
            .as_ref()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        Ok(preparer.hearing_audio_ingestion(session_id))
    }

    pub fn begin_hearing_context(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        self.user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .as_ref()
            .ok_or(RuntimeError::CompanionUnavailable)?
            .begin_hearing_context(generation, cancellation)
            .map_err(RuntimeError::from)
    }

    pub fn acknowledge_saved_hearing_audio(&self, id: &str) -> Result<(), RuntimeError> {
        self.user_preparer
            .read()
            .map_err(|_| RuntimeError::Closed)?
            .as_ref()
            .ok_or(RuntimeError::CompanionUnavailable)?
            .acknowledge_saved_hearing_audio(id)?;
        Ok(())
    }

    pub fn update_hearing_context(
        &self,
        context: crate::hearing_context::HearingContext,
    ) -> Result<bool, RuntimeError> {
        self.ensure_open()?;
        self.user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .as_ref()
            .ok_or(RuntimeError::CompanionUnavailable)?
            .update_hearing_context(context)
            .map_err(RuntimeError::from)
    }

    pub fn register_pending_frame_context(
        &self,
        context: crate::state::PendingFrameContext,
    ) -> Result<(), RuntimeError> {
        self.register_pending_frame_context_cancellable(context, None)
    }

    pub fn register_pending_frame_context_cancellable(
        &self,
        context: crate::state::PendingFrameContext,
        publication: Option<&crate::persistence::PublicationGate>,
    ) -> Result<(), RuntimeError> {
        self.ensure_open()?;
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        preparer
            .register_pending_frame_context(context, publication)
            .map_err(RuntimeError::from)
    }

    pub fn register_pending_frame_contexts_cancellable(
        &self,
        contexts: Vec<crate::state::PendingFrameContext>,
        publication: Option<&crate::persistence::PublicationGate>,
    ) -> Result<Vec<String>, RuntimeError> {
        self.ensure_open()?;
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        preparer
            .register_pending_frame_contexts(contexts, publication)
            .map_err(RuntimeError::from)
    }

    pub fn prepare_pending_frame_contexts(
        &self,
        contexts: Vec<crate::state::PendingFrameContext>,
    ) -> Result<Option<crate::companion_storage::PendingFrameContextChange>, RuntimeError> {
        self.ensure_open()?;
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        preparer
            .prepare_pending_frame_contexts(contexts)
            .map_err(RuntimeError::from)
    }

    pub fn remove_pending_frame_contexts_cancellable(
        &self,
        ids: &[String],
        publication: Option<&crate::persistence::PublicationGate>,
    ) -> Result<(), RuntimeError> {
        self.ensure_open()?;
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        preparer
            .remove_pending_frame_contexts(ids, publication)
            .map_err(RuntimeError::from)
    }

    pub async fn quiesce(&self) -> Result<u64, RuntimeError> {
        self.quiesce_inner(false, false).await
    }

    pub async fn quiesce_for_config_update(&self) -> Result<u64, RuntimeError> {
        self.quiesce_inner(false, true).await
    }

    pub async fn quiesce_for_conversation_reset(&self) -> Result<u64, RuntimeError> {
        self.quiesce_inner(true, false).await
    }

    async fn quiesce_inner(
        &self,
        clear_user_state: bool,
        config_update: bool,
    ) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        if config_update {
            self.cancel_operations_for_config_update();
        } else {
            self.cancel_operations();
        }
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::Quiesce {
                response,
                clear_user_state,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn user_message(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
    ) -> Result<CompanionResponse, RuntimeError> {
        self.ensure_open()?;
        let input = self.prepare_user_message(message, observations, None, None)?;
        let (response, result) = oneshot::channel();
        self.user_tx
            .send(UserCommand::Enqueue(Box::new(UserQueueCommand {
                input_id: input.id,
                response: Some(response),
            })))
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn enqueue_user_message(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
    ) -> Result<String, RuntimeError> {
        self.enqueue_user_message_with_attachment_and_tutorial_response(
            message,
            observations,
            None,
            None,
        )
        .await
    }

    pub async fn enqueue_user_message_with_attachment(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_source: Option<std::path::PathBuf>,
    ) -> Result<String, RuntimeError> {
        self.enqueue_user_message_with_attachment_and_tutorial_response(
            message,
            observations,
            attachment_source,
            None,
        )
        .await
    }

    pub async fn enqueue_user_message_with_attachment_and_tutorial_response(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_source: Option<std::path::PathBuf>,
        tutorial_response_key: Option<String>,
    ) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        let input = self.prepare_user_message(
            message,
            observations,
            attachment_source,
            tutorial_response_key,
        )?;
        let id = input.id.clone();
        #[cfg(test)]
        test_barrier::wait(&input.message).await;
        if self
            .user_tx
            .send(UserCommand::Enqueue(Box::new(UserQueueCommand {
                input_id: input.id,
                response: None,
            })))
            .await
            .is_err()
            && input.user_seq == 0
        {
            return Err(RuntimeError::Closed);
        }
        Ok(id)
    }

    pub async fn enqueue_user_message_with_text_attachment(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_text: String,
    ) -> Result<String, RuntimeError> {
        self.enqueue_user_message_with_text_attachment_and_tutorial_response(
            message,
            observations,
            attachment_text,
            None,
        )
        .await
    }

    pub async fn enqueue_user_message_with_text_attachment_and_tutorial_response(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_text: String,
        tutorial_response_key: Option<String>,
    ) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        if preparer.owns_user_queue() && !preparer.uses_persistent_queue() {
            return Err(RuntimeError::CompanionUnavailable);
        }
        let input = {
            let _turn_commit_guard = self
                .turn_commit_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let observation_in_progress = self.snapshot().phase == RuntimePhase::Observing;
            preparer
                .prepare_text_for_runtime(
                    message,
                    observations,
                    attachment_text,
                    observation_in_progress,
                    tutorial_response_key,
                )
                .map_err(RuntimeError::from)?
        };
        let id = input.id.clone();
        if self
            .user_tx
            .send(UserCommand::Enqueue(Box::new(UserQueueCommand {
                input_id: input.id,
                response: None,
            })))
            .await
            .is_err()
            && input.user_seq == 0
        {
            return Err(RuntimeError::Closed);
        }
        Ok(id)
    }

    fn prepare_user_message(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_source: Option<std::path::PathBuf>,
        tutorial_response_key: Option<String>,
    ) -> Result<crate::companion_storage::PendingUserMessage, RuntimeError> {
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        if preparer.owns_user_queue() && !preparer.uses_persistent_queue() {
            return Err(RuntimeError::CompanionUnavailable);
        }
        let _turn_commit_guard = self
            .turn_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let observation_in_progress = self.snapshot().phase == RuntimePhase::Observing;
        let input = preparer
            .prepare_for_runtime(
                message,
                observations,
                attachment_source,
                observation_in_progress,
                tutorial_response_key,
            )
            .map_err(RuntimeError::from)?;
        Ok(input)
    }

    pub async fn cancel_user_message(&self) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        let snapshot = self.snapshot();
        let input_id = snapshot
            .active_user_message_id
            .or_else(|| {
                snapshot
                    .last_error
                    .and_then(|error| error.terminal_user_input_id().map(str::to_owned))
            })
            .ok_or_else(|| {
                RuntimeError::Factory(
                    text(
                        TextKey::RuntimeCancelableReplyMissing,
                        Locale::from_config(&self.config().ui.language),
                    )
                    .to_owned(),
                )
            })?;
        self.cancel_user_message_for(input_id).await
    }

    pub async fn cancel_user_message_for(&self, input_id: String) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::CancelUser {
                input_id: Some(input_id.clone()),
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)??;
        Ok(input_id)
    }

    pub async fn retry_user_message(&self) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::RetryUser {
                input_id: None,
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        let input_id = result.await.map_err(|_| RuntimeError::ResponseDropped)??;
        Ok(input_id)
    }

    pub async fn retry_user_message_for(&self, input_id: String) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::RetryUser {
                input_id: Some(input_id),
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub fn has_pending_tutorial_response(&self, response_key: &str) -> Result<bool, RuntimeError> {
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        preparer
            .has_pending_tutorial_response(response_key)
            .map_err(RuntimeError::from)
    }
}
