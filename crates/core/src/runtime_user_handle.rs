use super::*;

impl RuntimeHandle {
    pub fn register_pending_frame_context(
        &self,
        context: crate::state::PendingFrameContext,
    ) -> Result<(), RuntimeError> {
        self.ensure_open()?;
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        preparer
            .register_pending_frame_context(context)
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
                    .and_then(|error| error.attachment_ocr)
                    .filter(|failure| !failure.retryable)
                    .map(|failure| failure.input_id)
            })
            .ok_or_else(|| RuntimeError::Factory("取り消せる返事はありません".to_owned()))?;
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::CancelUser { response })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)??;
        Ok(input_id)
    }

    pub async fn retry_user_message(&self) -> Result<String, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::RetryUser { response })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        let input_id = result.await.map_err(|_| RuntimeError::ResponseDropped)??;
        Ok(input_id)
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
