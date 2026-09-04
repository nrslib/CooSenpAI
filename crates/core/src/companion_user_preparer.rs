use super::user::UserMessagePreparer;
use super::user_prompt::format_appended_user_message;
use super::*;
use crate::attachments::bound_text_attachment;
use crate::companion_cursor::OWNED_USER_ID_PREFIX;
use crate::companion_storage::{PendingAttachmentFailure, PendingInput, PendingUserMessage};
use crate::provider::ProviderMidTurnInput;
use std::path::PathBuf;
use uuid::Uuid;

impl UserMessagePreparer {
    pub(crate) fn uses_persistent_queue(&self) -> bool {
        self.storage.is_some() && self.delivery_ownership == DeliveryOwnership::Owner
    }

    pub(crate) fn owns_user_queue(&self) -> bool {
        self.delivery_ownership == DeliveryOwnership::Owner
    }

    fn enqueue_runtime_input(&self, input: PendingUserMessage) {
        if let Ok(mut queue) = self.runtime_queue.lock() {
            queue.push_back(input);
        }
    }

    pub(crate) fn take_runtime_input(
        &self,
        input_id: &str,
    ) -> Result<Option<PendingUserMessage>, CompanionError> {
        let mut queue = self.runtime_queue.lock().map_err(|_| {
            PersistenceError::Invalid("runtime user queue が壊れています".to_owned())
        })?;
        Ok(queue
            .iter()
            .position(|input| input.id == input_id)
            .and_then(|position| queue.remove(position)))
    }

    pub(crate) fn extend_user_dispatch(
        &self,
        dispatch_seq: u64,
        input_ids: &[String],
    ) -> Result<(), CompanionError> {
        let Some(storage) = &self.storage else {
            return Err(PersistenceError::Invalid(
                "user dispatch の storage がありません".to_owned(),
            )
            .into());
        };
        storage.extend_user_dispatch(dispatch_seq, input_ids)?;
        Ok(())
    }

    pub(crate) fn register_pending_frame_context(
        &self,
        context: crate::state::PendingFrameContext,
    ) -> Result<(), CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        storage.register_pending_frame_context(context)?;
        Ok(())
    }

    pub(crate) fn prepare(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_source: Option<PathBuf>,
    ) -> Result<PendingUserMessage, CompanionError> {
        self.prepare_parts(
            message,
            observations,
            attachment_source,
            None,
            false,
            None,
            false,
        )
    }

    pub(crate) fn prepare_for_runtime(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_source: Option<PathBuf>,
        observation_in_progress: bool,
        tutorial_response_key: Option<String>,
    ) -> Result<PendingUserMessage, CompanionError> {
        self.prepare_parts(
            message,
            observations,
            attachment_source,
            None,
            observation_in_progress,
            tutorial_response_key,
            true,
        )
    }

    pub(crate) fn prepare_text(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_text: String,
    ) -> Result<PendingUserMessage, CompanionError> {
        let attachment = bound_text_attachment(&attachment_text)
            .ok_or_else(|| PersistenceError::Invalid("テキスト添付が空です".into()))?;
        self.prepare_parts(
            message,
            observations,
            None,
            Some(attachment.text),
            false,
            None,
            false,
        )
    }

    pub(crate) fn prepare_text_for_runtime(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_text: String,
        observation_in_progress: bool,
        tutorial_response_key: Option<String>,
    ) -> Result<PendingUserMessage, CompanionError> {
        let attachment = bound_text_attachment(&attachment_text)
            .ok_or_else(|| PersistenceError::Invalid("テキスト添付が空です".into()))?;
        self.prepare_parts(
            message,
            observations,
            None,
            Some(attachment.text),
            observation_in_progress,
            tutorial_response_key,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_parts(
        &self,
        message: String,
        observations: Vec<ObservationRecord>,
        attachment_source: Option<PathBuf>,
        attachment_text: Option<String>,
        observation_in_progress: bool,
        tutorial_response_key: Option<String>,
        persist_runtime_queue: bool,
    ) -> Result<PendingUserMessage, CompanionError> {
        let id = if self.delivery_ownership == DeliveryOwnership::Owner {
            format!("{OWNED_USER_ID_PREFIX}{}", Uuid::new_v4())
        } else {
            Uuid::new_v4().to_string()
        };
        let now = self.clock.now();
        let created_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let attachment_path = match (attachment_source.as_deref(), self.storage.as_ref()) {
            (Some(source), Some(storage)) => {
                Some(storage.persist_attachment(source, &id, &created_at)?)
            }
            (Some(_), None) => {
                return Err(CompanionError::Persistence(PersistenceError::Invalid(
                    "添付画像を保存する storage がありません".to_owned(),
                )))
            }
            (None, _) => None,
        };
        let (observations, pending_frames) = if let Some(storage) = &self.storage {
            let observations = crate::recent_observations::merge_recent_observations(
                observations,
                storage.load_recent_observations(now)?,
            );
            let observation_frame_paths = storage.observation_frame_paths(&observations, now)?;
            let pending_frames =
                if persist_runtime_queue && self.delivery_ownership == DeliveryOwnership::Owner {
                    storage
                        .load_cursor()
                        .map(|cursor| cursor.pending_frame_contexts)?
                } else {
                    Vec::new()
                };
            super::user_prompt::bound_user_screen_context_with_frame_paths(
                observations,
                pending_frames,
                &observation_frame_paths,
            )
        } else {
            (observations, Vec::new())
        };
        let mut input = PendingUserMessage {
            id,
            user_seq: 0,
            created_at,
            message,
            attachment_path,
            attachment_text,
            observations,
            pending_frames,
            observation_in_progress,
            prepared_response: None,
            response_commit_started: false,
            attachment_failure: None,
            tutorial_response_key,
        };
        if let Some(storage) = &self.storage {
            let appended = storage.append_conversation_once_at(&input.conversation_entry(), now)?;
            if appended {
                storage.record_stagnation_reaction_at(now);
            }
            if self.delivery_ownership == DeliveryOwnership::Owner {
                input = storage.enqueue_user_input(input.clone())?;
            }
        } else if persist_runtime_queue {
            self.enqueue_runtime_input(input.clone());
        }
        Ok(input)
    }

    pub(crate) fn cancel(&self, input_id: &str) -> Result<(), CompanionError> {
        if let Some(storage) = &self.storage {
            let commit_started = storage.update_cursor(|cursor| {
                let commit_started = cursor.pending_inputs.iter().any(|input| {
                    matches!(input, PendingInput::UserMessage(message)
                        if message.id == input_id && message.response_commit_started)
                });
                if commit_started {
                    return Ok(true);
                }
                cursor.pending_inputs.retain(|input| input.id() != input_id);
                if !cursor.cancelled_input_ids.iter().any(|id| id == input_id) {
                    cursor.cancelled_input_ids.push(input_id.to_owned());
                }
                if let Some(lease) = cursor.user_dispatch.as_mut() {
                    lease.input_ids.retain(|id| id != input_id);
                    if lease.input_ids.is_empty() {
                        cursor.user_dispatch = None;
                    }
                }
                Ok(false)
            })?;
            if commit_started {
                return Err(CompanionError::Persistence(PersistenceError::Invalid(
                    "応答の保存を開始したため取り消せません".to_owned(),
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn cancel_after_termination(&self, input_id: &str) -> Result<(), CompanionError> {
        if let Some(storage) = &self.storage {
            storage.cancel_user_input_after_termination(input_id)?;
            return Ok(());
        }
        self.cancel(input_id)
    }

    pub(crate) fn pending_messages(&self) -> Result<Vec<PendingUserMessage>, CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(self
                .runtime_queue
                .lock()
                .map_err(|_| {
                    PersistenceError::Invalid("runtime user queue が壊れています".to_owned())
                })?
                .iter()
                .filter(|message| !message.attachment_is_terminal())
                .cloned()
                .collect());
        };
        Ok(storage
            .reconcile_pending_user_inputs()?
            .pending_inputs
            .into_iter()
            .map(|pending| match pending {
                PendingInput::UserMessage(message) => message,
            })
            .filter(|message| !message.attachment_is_terminal())
            .collect())
    }

    pub(crate) fn pending_message(
        &self,
        input_id: &str,
    ) -> Result<Option<PendingUserMessage>, CompanionError> {
        if let Some(input) = self
            .pending_messages()?
            .into_iter()
            .find(|input| input.id == input_id)
        {
            return Ok(Some(input));
        }
        Ok(self
            .runtime_queue
            .lock()
            .map_err(|_| PersistenceError::Invalid("runtime user queue が壊れています".to_owned()))?
            .iter()
            .find(|input| input.id == input_id)
            .cloned())
    }

    pub(crate) fn has_pending_tutorial_response(
        &self,
        response_key: &str,
    ) -> Result<bool, CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(false);
        };
        Ok(storage
            .reconcile_pending_user_inputs()?
            .pending_inputs
            .iter()
            .any(|pending| match pending {
                PendingInput::UserMessage(message) => {
                    message.tutorial_response_key.as_deref() == Some(response_key)
                }
            }))
    }

    pub(crate) fn record_attachment_failure(
        &self,
        input_id: &str,
        reason: AttachmentOcrFailureKind,
    ) -> Result<PendingAttachmentFailure, CompanionError> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            PersistenceError::Invalid("添付失敗を保存する storage がありません".to_owned())
        })?;
        storage
            .update_cursor(|cursor| {
                let message = cursor
                    .pending_inputs
                    .iter_mut()
                    .find_map(|pending| match pending {
                        PendingInput::UserMessage(message) if message.id == input_id => {
                            Some(message)
                        }
                        _ => None,
                    })
                    .ok_or_else(|| {
                        PersistenceError::Invalid("添付失敗の入力が cursor にありません".to_owned())
                    })?;
                let attempts = message
                    .attachment_failure
                    .as_ref()
                    .filter(|failure| failure.reason == reason)
                    .map_or(1, |failure| failure.attempts.saturating_add(1));
                let terminal = matches!(
                    reason,
                    AttachmentOcrFailureKind::HelperUnavailable | AttachmentOcrFailureKind::NoText
                ) || (reason == AttachmentOcrFailureKind::Recognition
                    && attempts >= 3);
                let failure = PendingAttachmentFailure {
                    reason,
                    attempts,
                    terminal,
                };
                message.attachment_failure = Some(failure.clone());
                if terminal {
                    if let Some(lease) = cursor.user_dispatch.as_mut() {
                        lease.input_ids.retain(|id| id != input_id);
                        if lease.input_ids.is_empty() {
                            cursor.user_dispatch = None;
                        }
                    }
                }
                Ok(failure)
            })
            .map_err(CompanionError::from)
    }

    pub(crate) fn clear_terminal_attachment_failure(
        &self,
        input_id: &str,
    ) -> Result<bool, CompanionError> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            PersistenceError::Invalid("添付失敗を更新する storage がありません".to_owned())
        })?;
        storage
            .update_cursor(|cursor| {
                let Some(message) =
                    cursor
                        .pending_inputs
                        .iter_mut()
                        .find_map(|pending| match pending {
                            PendingInput::UserMessage(message) if message.id == input_id => {
                                Some(message)
                            }
                            _ => None,
                        })
                else {
                    return Ok(false);
                };
                if !message.attachment_is_terminal() {
                    return Ok(false);
                }
                message.attachment_failure = None;
                Ok(true)
            })
            .map_err(CompanionError::from)
    }

    pub(crate) fn mid_turn_input(
        &self,
        input: &PendingUserMessage,
    ) -> Result<ProviderMidTurnInput, CompanionError> {
        let images = match (&input.attachment_path, &self.storage) {
            (Some(relative), Some(storage)) => {
                vec![storage.resolve_attachment(relative)?.into()]
            }
            (Some(_), None) => {
                return Err(CompanionError::Persistence(PersistenceError::Invalid(
                    "添付画像を読み込む storage がありません".to_owned(),
                )))
            }
            (None, _) => Vec::new(),
        };
        Ok(ProviderMidTurnInput {
            source_id: input.id.clone(),
            message: format_appended_user_message(input)?,
            images,
        })
    }
}
