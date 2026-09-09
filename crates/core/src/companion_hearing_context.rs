use super::user::UserMessagePreparer;
use super::*;
use crate::hearing_context::HearingContext;

impl CompanionAgent {
    pub(crate) fn set_hearing_context(
        &mut self,
        context: Arc<std::sync::Mutex<crate::hearing_context::HearingContextBuffer>>,
    ) {
        self.hearing_context = context;
    }
}

impl UserMessagePreparer {
    pub(crate) fn begin_hearing_context(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<String, CompanionError> {
        let mut buffer = self
            .hearing_context
            .lock()
            .map_err(|_| PersistenceError::Invalid("音声文脈のロックが壊れています".to_owned()))?;
        buffer.begin(generation, cancellation).ok_or_else(|| {
            PersistenceError::Invalid("音声文脈の開始世代が古いか、取消済みです".to_owned()).into()
        })
    }

    pub(crate) fn update_hearing_context(
        &self,
        context: HearingContext,
    ) -> Result<bool, CompanionError> {
        let mut buffer = self
            .hearing_context
            .lock()
            .map_err(|_| PersistenceError::Invalid("音声文脈のロックが壊れています".to_owned()))?;
        let Some(context) = buffer.prepare_update(context) else {
            return Ok(false);
        };
        if context.confirmed {
            if let Some(storage) = &self.storage {
                storage.update_cursor(|cursor| {
                    for pending in &mut cursor.pending_inputs {
                        let crate::companion_storage::PendingInput::UserMessage(input) = pending;
                        if input.prepared_response.is_some() || input.response_commit_started {
                            continue;
                        }
                        for captured in &mut input.hearing_context {
                            if captured.same_segment(&context)
                                && captured.sequence < context.sequence
                            {
                                *captured = context.clone();
                            }
                        }
                    }
                    Ok(())
                })?;
            }
        }
        buffer.record(context);
        Ok(true)
    }
}
