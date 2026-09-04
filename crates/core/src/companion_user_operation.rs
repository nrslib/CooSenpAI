use super::user::UserMessagePreparer;
use super::*;
use crate::companion_storage::PendingInput;

impl UserMessagePreparer {
    pub(crate) fn begin_operation(
        &self,
        input_ids: &[String],
    ) -> Result<Option<u64>, CompanionError> {
        if self.delivery_ownership != DeliveryOwnership::Owner {
            return Ok(None);
        }
        let Some(storage) = &self.storage else {
            return Ok(None);
        };
        storage
            .update_cursor(|cursor| {
                for input_id in input_ids {
                    if cursor.cancelled_input_ids.iter().any(|id| id == input_id) {
                        return Err(PersistenceError::Invalid(
                            "取り消された user input です".to_owned(),
                        ));
                    }
                    if !cursor
                        .pending_inputs
                        .iter()
                        .any(|pending| pending.id() == input_id)
                    {
                        return Err(PersistenceError::Invalid(
                            "user input が cursor にありません".to_owned(),
                        ));
                    }
                }
                cursor.user_operation_generation = next_generation(cursor)?;
                Ok(Some(cursor.user_operation_generation))
            })
            .map_err(CompanionError::from)
    }

    pub(crate) fn request_restart(
        &self,
        expected_generation: Option<u64>,
        input_ids: &[String],
    ) -> Result<bool, CompanionError> {
        if self.delivery_ownership != DeliveryOwnership::Owner {
            return Ok(true);
        }
        let (Some(storage), Some(expected_generation)) = (&self.storage, expected_generation)
        else {
            return Ok(false);
        };
        storage
            .update_cursor(|cursor| {
                if cursor.user_operation_generation != expected_generation {
                    return Ok(false);
                }
                for input_id in input_ids {
                    let Some(PendingInput::UserMessage(input)) = cursor
                        .pending_inputs
                        .iter()
                        .find(|pending| pending.id() == input_id)
                    else {
                        return Ok(false);
                    };
                    if input.prepared_response.is_some() || input.response_commit_started {
                        return Ok(false);
                    }
                }
                cursor.user_operation_generation = next_generation(cursor)?;
                Ok(true)
            })
            .map_err(CompanionError::from)
    }
}

fn next_generation(
    cursor: &crate::companion_storage::CursorSnapshot,
) -> Result<u64, PersistenceError> {
    cursor
        .user_operation_generation
        .checked_add(1)
        .ok_or_else(|| {
            PersistenceError::Invalid("user operation generation が上限に達しました".to_owned())
        })
}

pub(super) struct UserCallCheckpoint {
    session: Option<crate::provider::ProviderSession>,
    session_calls: usize,
    needs_session_context: bool,
    previous_summary: Option<String>,
    pending_session_summary: Option<String>,
    pending_context_notice: Option<String>,
}

impl UserCallCheckpoint {
    pub(super) fn capture(agent: &CompanionAgent) -> Self {
        Self {
            session: agent.session.clone(),
            session_calls: agent.session_calls,
            needs_session_context: agent.needs_session_context,
            previous_summary: agent.previous_summary.clone(),
            pending_session_summary: agent.pending_session_summary.clone(),
            pending_context_notice: agent.pending_context_notice.clone(),
        }
    }

    pub(super) fn restore(self, agent: &mut CompanionAgent) {
        agent.session = self.session;
        agent.session_calls = self.session_calls;
        agent.needs_session_context = self.needs_session_context;
        agent.previous_summary = self.previous_summary;
        agent.pending_session_summary = self.pending_session_summary;
        agent.pending_context_notice = self.pending_context_notice;
    }

    pub(super) fn restore_after_cancellation(self, agent: &mut CompanionAgent) {
        self.restore(agent);
        agent.discard_provider_session();
    }
}
