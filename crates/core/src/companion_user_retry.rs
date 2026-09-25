use super::user::UserMessagePreparer;
use super::*;
use crate::companion_storage::{
    PendingInput, PendingUserMessage, TurnCommitKind, MAX_USER_RESPONSE_ATTEMPTS,
};
use crate::state::{
    ConversationEntry, ConversationMessageKind, ConversationRole, RESPONSE_FAILURE_HISTORY_PREFIX,
};
use uuid::Uuid;

fn safe_provider_failure(
    failure: Option<crate::provider::ProviderFailureSummary>,
) -> Option<crate::provider::ProviderFailureSummary> {
    failure.map(|failure| crate::provider::ProviderFailureSummary {
        kind: failure.kind,
        model: failure
            .model
            .as_deref()
            .and_then(crate::provider::safe_provider_model),
    })
}

fn response_failure_history_message(
    attempts: u8,
    failure: Option<&crate::provider::ProviderFailureSummary>,
) -> String {
    let kind = failure
        .map(|failure| failure.kind.as_str())
        .unwrap_or("unknown");
    let model = failure
        .and_then(|failure| failure.model.as_deref())
        .and_then(crate::provider::safe_provider_model)
        .unwrap_or_default();
    format!("{RESPONSE_FAILURE_HISTORY_PREFIX}attempts={attempts};kind={kind};model={model}")
}

fn response_failure_history_entry(
    input_id: &str,
    attempts: u8,
    failure: Option<&crate::provider::ProviderFailureSummary>,
    now: chrono::DateTime<chrono::Utc>,
) -> ConversationEntry {
    ConversationEntry {
        schema_version: 1,
        id: format!("response-failure:{input_id}:{attempts}"),
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        role: ConversationRole::Companion,
        message: response_failure_history_message(attempts, failure),
        message_kind: Some(ConversationMessageKind::System),
        attachment_path: None,
        attachment_text: None,
        tutorial_response_key: None,
        screen_context: None,
        caused_by_ids: vec![input_id.to_owned()],
        notification_priority: "warning".to_owned(),
    }
}

fn fresh_input_from_history(
    preparer: &UserMessagePreparer,
    storage: &crate::companion_storage::CompanionStorage,
    entry: ConversationEntry,
) -> Result<PendingUserMessage, CompanionError> {
    let context = entry.screen_context.unwrap_or_default();
    let id = if preparer.delivery_ownership == DeliveryOwnership::Owner {
        format!(
            "{}{}",
            crate::companion_cursor::OWNED_USER_ID_PREFIX,
            Uuid::new_v4()
        )
    } else {
        Uuid::new_v4().to_string()
    };
    let now = preparer.clock.now();
    let input = PendingUserMessage {
        id,
        conversation_generation: storage.conversation_generation()?,
        user_seq: 0,
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: entry.message,
        attachment_path: entry.attachment_path,
        attachment_text: entry.attachment_text,
        observations: context.observations,
        judge_feedback_targets: Vec::new(),
        pending_frames: context.pending_frames,
        hearing_context: context.hearing_context,
        pending_audio: context.pending_audio,
        pending_audio_ids: context.pending_audio_ids,
        observation_in_progress: false,
        prepared_response: None,
        response_commit_started: false,
        attachment_failure: None,
        response_attempts: 0,
        response_terminal: false,
        response_failure: None,
        tutorial_response_key: entry.tutorial_response_key,
    };
    if storage.append_conversation_once_at(&input.conversation_entry(), now)? {
        storage.record_stagnation_reaction_at(now);
    }
    Ok(storage.enqueue_user_input(input)?)
}

pub(super) fn begin_response_attempt(
    input: &mut PendingUserMessage,
) -> Result<(), PersistenceError> {
    if input.prepared_response.is_some() || input.response_commit_started {
        return Ok(());
    }
    if input.is_terminal() {
        return Err(PersistenceError::Invalid(
            "利用者応答の試行回数が上限に達しました".into(),
        ));
    }
    // 添付準備は provider を呼ばないため、OCR の再試行で応答予算を消費しない。
    if input
        .attachment_failure
        .as_ref()
        .is_some_and(|failure| !failure.terminal)
    {
        return Ok(());
    }
    if input.response_attempts >= MAX_USER_RESPONSE_ATTEMPTS {
        return Err(PersistenceError::Invalid(
            "利用者応答の試行回数が上限に達しました".into(),
        ));
    }
    input.response_attempts += 1;
    input.response_failure = None;
    Ok(())
}

impl UserMessagePreparer {
    pub(crate) fn begin_appended_response_attempt(
        &self,
        input_id: &str,
    ) -> Result<(), CompanionError> {
        let Some(storage) = self.storage.clone() else {
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

    pub(crate) fn retry_user_input(&self, input_id: &str) -> Result<String, CompanionError> {
        let Some(storage) = &self.storage else {
            let mut queue = self.runtime_queue.lock().map_err(|_| {
                PersistenceError::Invalid("runtime user queue が壊れています".to_owned())
            })?;
            let input = queue
                .iter_mut()
                .find(|input| input.id == input_id)
                .ok_or_else(|| PersistenceError::Invalid("再試行する入力がありません".into()))?;
            input.id = Uuid::new_v4().to_string();
            input.response_attempts = 0;
            input.response_terminal = false;
            input.response_failure = None;
            return Ok(input.id.clone());
        };
        let pending = storage
            .load_cursor()?
            .pending_inputs
            .into_iter()
            .find_map(|pending| match pending {
                PendingInput::UserMessage(input) if input.id == input_id => Some(input),
                _ => None,
            });
        if let Some(input) = pending {
            if input.attachment_is_terminal() && !input.response_terminal {
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
                    input.attachment_failure = None;
                    input.response_attempts = 0;
                    input.response_terminal = false;
                    input.response_failure = None;
                    Ok(())
                })?;
                return Ok(input_id.to_owned());
            }
        }
        let entry = storage
            .load_all_conversation()?
            .into_iter()
            .rev()
            .find(|entry| entry.role == ConversationRole::User && entry.id == input_id)
            .ok_or_else(|| PersistenceError::Invalid("再送する履歴がありません".into()))?;
        storage.cancel_user_input_after_termination(input_id)?;
        Ok(fresh_input_from_history(self, storage, entry)?.id)
    }
}

impl CompanionAgent {
    pub(crate) fn record_user_response_failure(
        &self,
        input_ids: &[String],
        error: &CompanionError,
    ) -> Result<(), CompanionError> {
        let kind = match error {
            CompanionError::Provider(error) => error.kind,
            CompanionError::Output => ProviderErrorKind::InvalidOutput,
            _ => return Ok(()),
        };
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        // 設定値を診断へ渡す際も、パス・認証情報・任意の文章をモデル名として公開しない。
        let model = crate::provider::safe_provider_model(&self.config.model);
        storage.update_cursor(|cursor| {
            let dispatch = cursor
                .user_dispatch
                .as_ref()
                .filter(|lease| lease.input_ids.iter().any(|id| input_ids.contains(id)));
            for pending in &mut cursor.pending_inputs {
                let PendingInput::UserMessage(input) = pending;
                let belongs_to_call = input_ids.contains(&input.id)
                    || dispatch.is_some_and(|lease| {
                        lease.input_ids.contains(&input.id) && input.response_attempts > 0
                    });
                if belongs_to_call
                    && input.prepared_response.is_none()
                    && !input.response_commit_started
                {
                    input.response_failure = Some(crate::provider::ProviderFailureSummary {
                        kind,
                        model: model.clone(),
                    });
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    // 実行中には呼ばない。失敗後と起動時に、呼び出し前に保存した回数から停止状態を確定する。
    pub(crate) fn settle_exhausted_user_responses(
        &mut self,
    ) -> Result<Vec<(String, u8, Option<crate::provider::ProviderFailureSummary>)>, CompanionError>
    {
        let Some(storage) = self.storage.clone() else {
            return Ok(Vec::new());
        };
        let exhausted = |input: &PendingUserMessage| {
            (input.response_attempts >= MAX_USER_RESPONSE_ATTEMPTS
                || input
                    .response_failure
                    .as_ref()
                    .is_some_and(|failure| failure.kind.stops_automatic_retry()))
                && input.attachment_failure.is_none()
                && input.prepared_response.is_none()
                && !input.response_commit_started
        };
        let cursor = storage.load_cursor()?;
        let terminal = cursor
            .pending_inputs
            .iter()
            .filter_map(|pending| {
                let PendingInput::UserMessage(input) = pending;
                (input.response_terminal || exhausted(input)).then(|| {
                    (
                        input.id.clone(),
                        input.response_attempts,
                        safe_provider_failure(input.response_failure.clone()),
                    )
                })
            })
            .collect::<Vec<_>>();
        if terminal.is_empty() {
            return Ok(Vec::new());
        }
        let now = self.clock.now();
        for (input_id, attempts, failure) in &terminal {
            let entry = response_failure_history_entry(input_id, *attempts, failure.as_ref(), now);
            self.append_conversation_once(entry)?;
        }
        let terminal_ids = terminal
            .iter()
            .map(|(input_id, _, _)| input_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        storage.update_cursor(|cursor| {
            cursor
                .pending_inputs
                .retain(|pending| !terminal_ids.contains(pending.id()));
            if let Some(lease) = cursor.user_dispatch.as_mut() {
                lease
                    .input_ids
                    .retain(|id| !terminal_ids.contains(id.as_str()));
                if lease.input_ids.is_empty() {
                    cursor.user_dispatch = None;
                }
            }
            if cursor.active_turn_commit.as_ref().is_some_and(|commit| {
                commit.kind == TurnCommitKind::User
                    && commit
                        .target_ids
                        .iter()
                        .any(|id| terminal_ids.contains(id.as_str()))
            }) {
                cursor.active_turn_commit = None;
            }
            Ok(())
        })?;
        Ok(terminal)
    }

    pub(crate) fn terminal_user_responses(
        &self,
    ) -> Result<Vec<(String, u8, Option<crate::provider::ProviderFailureSummary>)>, CompanionError>
    {
        let Some(storage) = &self.storage else {
            return Ok(Vec::new());
        };
        let cancelled = storage
            .load_cursor()?
            .cancelled_input_ids
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        Ok(storage
            .load_all_conversation()?
            .into_iter()
            .rev()
            .filter_map(|entry| {
                entry
                    .is_response_failure()
                    .then(|| entry.response_failure())
                    .flatten()
                    .map(|(attempts, provider)| {
                        (
                            entry.caused_by_ids.into_iter().next().unwrap_or_default(),
                            attempts,
                            provider,
                        )
                    })
            })
            .filter(|(input_id, _, _)| !input_id.is_empty() && !cancelled.contains(input_id))
            .collect())
    }

    pub(crate) fn first_terminal_user_response(
        &self,
    ) -> Result<Option<(String, u8, Option<crate::provider::ProviderFailureSummary>)>, CompanionError>
    {
        Ok(self.terminal_user_responses()?.into_iter().next())
    }
}
