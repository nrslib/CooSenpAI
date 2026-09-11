use super::user::UserMessagePreparer;
use super::*;
use crate::companion_storage::{PendingInput, PendingUserMessage, MAX_USER_RESPONSE_ATTEMPTS};

pub(super) fn begin_response_attempt(
    input: &mut PendingUserMessage,
) -> Result<(), PersistenceError> {
    if input.prepared_response.is_some() || input.response_commit_started {
        return Ok(());
    }
    if input.is_terminal() || input.response_attempts >= MAX_USER_RESPONSE_ATTEMPTS {
        return Err(PersistenceError::Invalid(
            "利用者応答の試行回数が上限に達しました".into(),
        ));
    }
    input.response_attempts += 1;
    Ok(())
}

impl UserMessagePreparer {
    pub(crate) fn begin_appended_response_attempt(
        &self,
        input_id: &str,
    ) -> Result<(), CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        storage.update_cursor(|cursor| {
            let Some(PendingInput::UserMessage(input)) = cursor
                .pending_inputs
                .iter_mut()
                .find(|pending| pending.id() == input_id)
            else {
                return Err(PersistenceError::Invalid("言い足す入力がありません".into()));
            };
            begin_response_attempt(input)
        })?;
        Ok(())
    }

    pub(crate) fn retry_user_input(&self, input_id: &str) -> Result<(), CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        storage.update_cursor(|cursor| {
            let Some(PendingInput::UserMessage(input)) = cursor
                .pending_inputs
                .iter_mut()
                .find(|pending| pending.id() == input_id)
            else {
                return Err(PersistenceError::Invalid(
                    "再試行する入力がありません".into(),
                ));
            };
            if input.attachment_is_terminal() {
                input.attachment_failure = None;
            }
            input.response_attempts = 0;
            input.response_terminal = false;
            Ok(())
        })?;
        Ok(())
    }
}

impl CompanionAgent {
    // 実行中には呼ばない。失敗後と起動時に、呼び出し前に保存した回数から停止状態を確定する。
    pub(crate) fn settle_exhausted_user_responses(&self) -> Result<Vec<String>, CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(Vec::new());
        };
        let exhausted = |input: &PendingUserMessage| {
            input.response_attempts >= MAX_USER_RESPONSE_ATTEMPTS
                && input.prepared_response.is_none()
                && !input.response_commit_started
        };
        let cursor = storage.load_cursor()?;
        if !cursor.pending_inputs.iter().any(|pending| {
            let PendingInput::UserMessage(input) = pending;
            exhausted(input) && !input.response_terminal
        }) {
            return Ok(cursor
                .pending_inputs
                .into_iter()
                .filter_map(|pending| {
                    let PendingInput::UserMessage(input) = pending;
                    input.response_terminal.then_some(input.id)
                })
                .collect());
        }
        let terminal_ids = storage.update_cursor(|cursor| {
            let mut terminal_ids = Vec::new();
            for pending in &mut cursor.pending_inputs {
                let PendingInput::UserMessage(input) = pending;
                if exhausted(input) {
                    input.response_terminal = true;
                    terminal_ids.push(input.id.clone());
                }
            }
            if let Some(lease) = cursor.user_dispatch.as_mut() {
                lease.input_ids.retain(|id| !terminal_ids.contains(id));
                if lease.input_ids.is_empty() {
                    cursor.user_dispatch = None;
                }
            }
            Ok(terminal_ids)
        })?;
        Ok(terminal_ids)
    }

    pub(crate) fn first_terminal_user_response(
        &self,
    ) -> Result<Option<(String, u8)>, CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(None);
        };
        Ok(storage
            .load_cursor()?
            .pending_inputs
            .into_iter()
            .find_map(|pending| {
                let PendingInput::UserMessage(input) = pending;
                input
                    .response_terminal
                    .then_some((input.id, input.response_attempts))
            }))
    }
}
