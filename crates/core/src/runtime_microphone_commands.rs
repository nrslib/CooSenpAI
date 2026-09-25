use super::*;
use crate::hearing_context::microphone_commands::MicrophoneCommandBatch;

impl RuntimeActor {
    pub(super) fn sync_microphone_command_policy(&self) {
        let enabled = self.config.audio.enabled
            && self.config.audio.mic
            && self.config.audio.microphone_commands_enabled
            && self.observation_delivery == ObservationDelivery::Companion
            && self
                .user_preparer
                .read()
                .is_ok_and(|preparer| preparer.is_some());
        self.hearing_context
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .set_microphone_commands_enabled(enabled);
    }

    pub(super) fn enqueue_microphone_commands(
        &mut self,
        batch: Option<MicrophoneCommandBatch>,
    ) -> Result<(), RuntimeError> {
        let Some(batch) = batch else { return Ok(()) };
        self.sync_microphone_command_policy();
        let commands = self
            .hearing_context
            .lock()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .take_microphone_commands(&batch);
        if commands.is_empty() {
            return Ok(());
        }
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?
            .clone()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        if !preparer.uses_persistent_queue() {
            return Err(RuntimeError::CompanionUnavailable);
        }
        let _commit = self
            .turn_commit_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for command in commands {
            let input =
                preparer.prepare_for_runtime(command.text, Vec::new(), None, false, None)?;
            self.user_work_pending = true;
            self.conversation_revision = self.conversation_revision.saturating_add(1);
            match self
                .user_command_tx
                .try_send(UserCommand::Enqueue(Box::new(UserQueueCommand {
                    input_id: input.id,
                    response: None,
                }))) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => return Err(RuntimeError::Closed),
            }
        }
        Ok(())
    }
}
