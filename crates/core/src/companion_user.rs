use super::user_operation::UserCallCheckpoint;
use super::user_prompt::{
    format_pending_frame_contexts, format_user_messages, keep_latest_observations,
    turn_observations,
};
use super::*;
use crate::companion_storage::{PendingInput, PendingUserMessage, PreparedUserResponse};
use crate::provider::ProviderMidTurnInput;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::mpsc;
use uuid::Uuid;

const MAX_BATCHED_USER_PROMPT_BYTES: usize = 512 * 1024;

fn tutorial_response_key(inputs: &[PendingUserMessage]) -> Result<Option<String>, CompanionError> {
    let key = inputs
        .iter()
        .find_map(|input| input.tutorial_response_key.clone());
    if inputs
        .iter()
        .all(|input| input.tutorial_response_key == key)
    {
        Ok(key)
    } else {
        Err(PersistenceError::Invalid(
            "異なる tutorial step の入力を同じ turn にまとめられません".to_owned(),
        )
        .into())
    }
}

pub(crate) struct UserOperationResult {
    pub(crate) response: CompanionResponse,
    pub(crate) input_ids: Vec<String>,
    pub(crate) prepared_response: PreparedUserResponse,
    pub(crate) data: crate::prompts::CompanionPromptData,
    pub(crate) observations: Vec<ObservationRecord>,
    pub(crate) source_ids: Vec<String>,
    pub(crate) dispatch_seq: u64,
}

#[derive(Clone)]
pub(crate) struct UserMessagePreparer {
    pub(super) storage: Option<CompanionStorage>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) delivery_ownership: DeliveryOwnership,
    pub(super) runtime_queue: Arc<std::sync::Mutex<VecDeque<PendingUserMessage>>>,
}

impl CompanionAgent {
    pub async fn user_message(
        &mut self,
        message: String,
        recent_observations: Vec<ObservationRecord>,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, CompanionError> {
        let input = self.prepare_user_message(message, recent_observations)?;
        let input_id = input.id.clone();
        let response = self.process_user_message(input, cancellation).await?;
        self.finalize_user_response(&input_id)?;
        Ok(response)
    }

    pub fn prepare_user_message(
        &mut self,
        message: String,
        recent_observations: Vec<ObservationRecord>,
    ) -> Result<PendingUserMessage, CompanionError> {
        self.prepare_user_message_with_attachment(message, recent_observations, None)
    }

    pub fn prepare_user_message_with_attachment(
        &mut self,
        message: String,
        recent_observations: Vec<ObservationRecord>,
        attachment_source: Option<PathBuf>,
    ) -> Result<PendingUserMessage, CompanionError> {
        self.initialize_storage()?;
        let input = self.user_message_preparer().prepare(
            message,
            recent_observations,
            attachment_source,
        )?;
        if self.delivery_ownership == DeliveryOwnership::Owner {
            self.pending_user_messages.push_back(input.clone());
        }
        Ok(input)
    }

    pub fn prepare_user_message_with_text_attachment(
        &mut self,
        message: String,
        recent_observations: Vec<ObservationRecord>,
        attachment_text: String,
    ) -> Result<PendingUserMessage, CompanionError> {
        self.initialize_storage()?;
        let input = self.user_message_preparer().prepare_text(
            message,
            recent_observations,
            attachment_text,
        )?;
        if self.delivery_ownership == DeliveryOwnership::Owner {
            self.pending_user_messages.push_back(input.clone());
        }
        Ok(input)
    }

    pub(crate) fn user_message_preparer(&self) -> UserMessagePreparer {
        self.user_message_preparer_with_runtime_queue(self.runtime_user_queue.clone())
    }

    pub(crate) fn user_message_preparer_with_runtime_queue(
        &self,
        runtime_queue: Arc<std::sync::Mutex<VecDeque<PendingUserMessage>>>,
    ) -> UserMessagePreparer {
        UserMessagePreparer {
            storage: self.storage.clone(),
            clock: self.clock.clone(),
            delivery_ownership: self.delivery_ownership,
            runtime_queue,
        }
    }

    pub(crate) fn persist_prepared_user_input(
        &mut self,
        mut input: PendingUserMessage,
    ) -> Result<PendingUserMessage, CompanionError> {
        if self.delivery_ownership != DeliveryOwnership::Owner || input.user_seq != 0 {
            return Ok(input);
        }
        let storage = self.storage.clone().ok_or_else(|| {
            PersistenceError::Invalid("user input を保存する storage がありません".to_owned())
        })?;
        let now = self.clock.now();
        if storage.append_conversation_once_at(&input.conversation_entry(), now)? {
            storage.record_stagnation_reaction_at(now);
        }
        input = storage.enqueue_user_input(input)?;
        self.pending_user_messages.push_back(input.clone());
        Ok(input)
    }

    pub(crate) fn cancel_user_message(&mut self, input_id: &str) -> Result<(), CompanionError> {
        self.user_message_preparer().cancel(input_id)?;
        self.pending_user_messages
            .retain(|input| input.id != input_id);
        Ok(())
    }

    pub(crate) fn cancel_user_message_after_termination(
        &mut self,
        input_id: &str,
    ) -> Result<(), CompanionError> {
        self.user_message_preparer()
            .cancel_after_termination(input_id)?;
        self.pending_user_messages
            .retain(|input| input.id != input_id);
        Ok(())
    }

    pub(crate) fn record_attachment_failure(
        &self,
        input_id: &str,
        reason: AttachmentOcrFailureKind,
    ) -> Result<crate::companion_storage::PendingAttachmentFailure, CompanionError> {
        self.user_message_preparer()
            .record_attachment_failure(input_id, reason)
    }

    pub(crate) fn clear_terminal_attachment_failure(
        &self,
        input_id: &str,
    ) -> Result<bool, CompanionError> {
        self.user_message_preparer()
            .clear_terminal_attachment_failure(input_id)
    }

    pub(crate) fn first_terminal_attachment_failure(
        &self,
    ) -> Result<Option<(String, crate::companion_storage::PendingAttachmentFailure)>, CompanionError>
    {
        let Some(storage) = &self.storage else {
            return Ok(None);
        };
        Ok(storage
            .load_cursor()?
            .pending_inputs
            .into_iter()
            .find_map(|pending| match pending {
                PendingInput::UserMessage(message) => message
                    .attachment_failure
                    .filter(|failure| failure.terminal)
                    .map(|failure| (message.id, failure)),
            }))
    }

    pub async fn process_user_message(
        &mut self,
        input: PendingUserMessage,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, CompanionError> {
        self.process_user_message_streaming(input, cancellation, None)
            .await
    }

    pub async fn process_user_message_streaming(
        &mut self,
        input: PendingUserMessage,
        cancellation: CancellationToken,
        events: Option<Arc<dyn crate::provider::ProviderEventSink>>,
    ) -> Result<CompanionResponse, CompanionError> {
        let accepted = Arc::new(Mutex::new(HashSet::new()));
        let generation = self
            .user_message_preparer()
            .begin_operation(std::slice::from_ref(&input.id))?;
        let candidate = self
            .process_user_messages_streaming(
                vec![input],
                cancellation,
                events,
                None,
                accepted,
                generation,
            )
            .await?;
        self.commit_user_candidate(&candidate, generation)?;
        if candidate.dispatch_seq == 0 {
            self.finalize_user_responses(&candidate.input_ids)?;
        }
        Ok(candidate.response)
    }

    pub(crate) async fn process_user_messages_streaming(
        &mut self,
        mut inputs: Vec<PendingUserMessage>,
        cancellation: CancellationToken,
        events: Option<Arc<dyn crate::provider::ProviderEventSink>>,
        additional_inputs: Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
        accepted_mid_turn_ids: Arc<Mutex<HashSet<String>>>,
        operation_generation: Option<u64>,
    ) -> Result<UserOperationResult, CompanionError> {
        for input in &inputs {
            self.ensure_user_input_pending(input)?;
        }
        self.latest_user_activity_at = inputs
            .iter()
            .filter_map(|input| chrono::DateTime::parse_from_rfc3339(&input.created_at).ok())
            .map(|created_at| created_at.with_timezone(&chrono::Utc))
            .chain(self.latest_user_activity_at)
            .max();
        let input_ids = inputs
            .iter()
            .map(|input| input.id.clone())
            .collect::<Vec<_>>();
        let tutorial_response_key = tutorial_response_key(&inputs)?;
        if let Some(prepared) = common_prepared_response(&inputs)? {
            return Ok(UserOperationResult {
                response: prepared_response(&prepared),
                input_ids,
                prepared_response: prepared,
                data: crate::prompts::CompanionPromptData::default(),
                observations: Vec::new(),
                source_ids: Vec::new(),
                dispatch_seq: self
                    .active_user_dispatch
                    .as_ref()
                    .map_or(0, |lease| lease.dispatch_seq),
            });
        }
        self.refresh_runtime_observation_context(&mut inputs)?;
        let observations = turn_observations(&inputs);
        let attachment_observations =
            super::helpers::unsent_observations(observations.clone(), &self.sent_observation_ids);
        let observation_frame_paths = self.observation_frame_paths(&observations)?;
        let mut data = self.user_prompt_data(&inputs, observation_frame_paths.clone())?;
        let observation_image_paths =
            self.observation_image_paths(&attachment_observations, &observation_frame_paths);
        let image_paths = self
            .bounded_provider_image_paths(self.user_image_paths(&inputs)?, observation_image_paths);
        let (image_paths, attachment_ocr_text) = self
            .prepare_image_attachments(image_paths, cancellation.child_token())
            .await?;
        data.attachment_ocr_text = attachment_ocr_text;
        let checkpoint = UserCallCheckpoint::capture(self);
        let outcome = match self
            .call(
                CompanionTurn {
                    data,
                    user: true,
                    observations,
                    image_paths,
                    events,
                    requested_source_ids: input_ids.clone(),
                    additional_inputs,
                    accepted_mid_turn_ids: Some(accepted_mid_turn_ids.clone()),
                    tutorial_response_key,
                },
                cancellation.clone(),
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                if cancellation.is_cancelled() {
                    checkpoint.restore_after_cancellation(self);
                }
                return Err(error);
            }
        };
        if cancellation.is_cancelled() {
            checkpoint.restore_after_cancellation(self);
            return Err(CompanionError::Cancelled);
        }
        let input_ids = self.complete_turn_input_ids(input_ids, &accepted_mid_turn_ids)?;
        #[cfg(test)]
        if let Some(input) = inputs.first() {
            user_response_barrier::wait_with_cancellation(&input.message, &cancellation).await;
        }
        if cancellation.is_cancelled() {
            checkpoint.restore_after_cancellation(self);
            return Err(CompanionError::Cancelled);
        }
        if let (Some(expected), Some(storage)) = (operation_generation, self.storage.as_ref()) {
            if storage.load_cursor()?.user_operation_generation != expected {
                checkpoint.restore_after_cancellation(self);
                return Err(CompanionError::Cancelled);
            }
        }
        let prepared = PreparedUserResponse {
            id: Uuid::new_v4().to_string(),
            created_at: self
                .clock
                .now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: require_user_message(&outcome.response)?,
            session_summary: self.pending_session_summary.clone(),
        };
        Ok(UserOperationResult {
            response: outcome.response,
            input_ids,
            prepared_response: prepared,
            data: outcome.data,
            observations: outcome.observations,
            source_ids: outcome.source_ids,
            dispatch_seq: self
                .active_user_dispatch
                .as_ref()
                .map_or(0, |lease| lease.dispatch_seq),
        })
    }

    pub(crate) fn next_pending_user_messages(
        &mut self,
        append: bool,
    ) -> Result<Vec<PendingUserMessage>, CompanionError> {
        if self.delivery_ownership != DeliveryOwnership::Owner {
            return Ok(Vec::new());
        }
        let Some(storage) = self.storage.clone() else {
            return Ok(Vec::new());
        };
        let cursor = storage.reconcile_pending_user_inputs()?;
        self.pending_user_messages = cursor
            .pending_inputs
            .into_iter()
            .map(|pending| match pending {
                PendingInput::UserMessage(message) => message,
            })
            .collect();
        let (lease, mut inputs) = storage.lease_user_inputs(1)?;
        self.active_user_dispatch = (lease.dispatch_seq != 0).then_some(lease);
        if append
            && inputs
                .first()
                .is_some_and(|input| input.prepared_response.is_none())
        {
            let eligible = self
                .pending_user_messages
                .iter()
                .filter(|input| {
                    !input.attachment_is_terminal()
                        && input.prepared_response.is_none()
                        && !inputs.iter().any(|selected| selected.id == input.id)
                })
                .cloned()
                .collect::<Vec<_>>();
            for candidate in eligible {
                let mut proposed = inputs.clone();
                proposed.push(candidate.clone());
                if !self.user_turn_fits_bridge(&proposed)? {
                    break;
                }
                let Some(lease) = self.active_user_dispatch.as_ref() else {
                    break;
                };
                storage.extend_user_dispatch(
                    lease.dispatch_seq,
                    std::slice::from_ref(&candidate.id),
                )?;
                inputs.push(candidate);
            }
        }
        let existing_ids = storage
            .load_conversation()?
            .into_iter()
            .map(|entry| entry.id)
            .collect::<HashSet<_>>();
        let now = self.clock.now();
        for input in &inputs {
            if !existing_ids.contains(&input.id) {
                let entry = input.conversation_entry();
                storage.append_conversation_once_at_without_pruning(&entry, now)?;
                self.remember_conversation_entry(entry);
            }
        }
        Ok(inputs)
    }

    fn user_turn_fits_bridge(&self, inputs: &[PendingUserMessage]) -> Result<bool, CompanionError> {
        let mut inputs = inputs.to_vec();
        self.refresh_runtime_observation_context(&mut inputs)?;
        let observations = turn_observations(&inputs);
        let attachment_observations =
            super::helpers::unsent_observations(observations.clone(), &self.sent_observation_ids);
        let observation_frame_paths = self.observation_frame_paths(&observations)?;
        let mut data = self.user_prompt_data(&inputs, observation_frame_paths.clone())?;
        let source_ids = inputs
            .iter()
            .map(|input| input.id.clone())
            .collect::<Vec<_>>();
        self.apply_memory_context(&mut data, &observations, &source_ids)?;
        self.apply_session_context(&mut data, true, &source_ids)?;
        let prompt = build_companion_prompt(&data);
        if prompt.len() > MAX_BATCHED_USER_PROMPT_BYTES {
            return Ok(false);
        }
        let observation_image_paths =
            self.observation_image_paths(&attachment_observations, &observation_frame_paths);
        let images = self
            .bounded_provider_image_paths(self.user_image_paths(&inputs)?, observation_image_paths);
        let session = self
            .session
            .clone()
            .map_or(SessionRequest::New, SessionRequest::Resume);
        let tutorial_response_key = tutorial_response_key(&inputs)?;
        Ok(crate::provider::bridge_send_request_fits(
            &self.provider_call(&prompt, &images, session, tutorial_response_key.as_deref()),
        ))
    }

    fn user_prompt_data(
        &self,
        inputs: &[PendingUserMessage],
        observation_frame_paths: HashMap<String, Vec<PathBuf>>,
    ) -> Result<CompanionPromptData, CompanionError> {
        let input = inputs.first().ok_or_else(|| {
            CompanionError::Persistence(PersistenceError::Invalid("user input が空です".to_owned()))
        })?;
        let input_ids = inputs
            .iter()
            .map(|input| input.id.as_str())
            .collect::<Vec<_>>();
        let observations = turn_observations(inputs);
        Ok(CompanionPromptData {
            companion_name: self.display_name.clone(),
            observations: observation_values(&observations)?,
            observation_frame_paths,
            observation_log_directory: self.observation_log_directory()?,
            omitted_observations: Some(Vec::new()),
            compact_observations: true,
            omitted_summary: None,
            omitted_ids: Vec::new(),
            last_observation: observations.last().map(observation_value).transpose()?,
            elapsed_ms: None,
            stuck_after_ms: Some(self.config.stuck_after_ms),
            repeated_error_count: 0,
            previous_summary: self.previous_summary.clone(),
            recent_conversation_jsonl: None,
            user_message: Some(format_user_messages(inputs)?),
            user_message_id: Some(if input_ids.len() == 1 {
                input.id.clone()
            } else {
                serde_json::to_string(&input_ids)?
            }),
            user_attachment: inputs.iter().any(|input| input.attachment_path.is_some()),
            attachment_ocr_text: None,
            pending_frame_context: format_pending_frame_contexts(inputs),
            memory_block: None,
            context_notice: inputs
                .iter()
                .any(|input| input.observation_in_progress)
                .then(|| "直前の画面はいま確認中".to_owned()),
        })
    }

    fn refresh_runtime_observation_context(
        &self,
        inputs: &mut [PendingUserMessage],
    ) -> Result<(), CompanionError> {
        if self.delivery_ownership != DeliveryOwnership::Owner {
            return Ok(());
        }
        let Some(storage) = self.storage.as_ref() else {
            return Ok(());
        };
        let now = self.clock.now();
        let supplied = inputs
            .iter()
            .flat_map(|input| input.observations.iter().cloned())
            .collect();
        let cursor = storage.load_cursor()?;
        let observations = crate::recent_observations::merge_recent_observations(
            crate::recent_observations::merge_recent_observations(
                supplied,
                storage.load_recent_observations(now)?,
            ),
            cursor.pending.clone(),
        );
        let observations = keep_latest_observations(observations, self.config.wake_coalesce_max);
        let observation_in_progress = inputs.iter().any(|input| input.observation_in_progress);
        let supplied_frames = inputs
            .iter()
            .flat_map(|input| input.pending_frames.iter().cloned())
            .chain(cursor.pending_frame_contexts)
            .collect::<Vec<_>>();
        let observation_frame_paths = storage.observation_frame_paths(&observations, now)?;
        let (observations, pending_frames) =
            super::user_prompt::bound_user_screen_context_with_frame_paths(
                observations,
                supplied_frames,
                &observation_frame_paths,
            );
        for input in inputs.iter_mut() {
            input.observations.clear();
            input.pending_frames.clear();
            input.observation_in_progress = observation_in_progress;
        }
        if let Some(input) = inputs.first_mut() {
            input.observations = observations;
            input.pending_frames = pending_frames;
        }
        storage.update_cursor(|cursor| {
            for input in inputs.iter() {
                if let Some(crate::companion_storage::PendingInput::UserMessage(pending)) = cursor
                    .pending_inputs
                    .iter_mut()
                    .find(|pending| pending.id() == input.id)
                {
                    pending.observations = input.observations.clone();
                    pending.pending_frames = input.pending_frames.clone();
                    pending.observation_in_progress = observation_in_progress;
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    fn user_image_paths(
        &self,
        inputs: &[PendingUserMessage],
    ) -> Result<Vec<PathBuf>, CompanionError> {
        inputs
            .iter()
            .filter_map(|input| input.attachment_path.as_ref())
            .map(|relative| {
                let storage = self.storage.as_ref().ok_or_else(|| {
                    CompanionError::Persistence(PersistenceError::Invalid(
                        "添付画像を読み込む storage がありません".to_owned(),
                    ))
                })?;
                storage
                    .resolve_attachment(relative)
                    .map_err(CompanionError::from)
            })
            .collect()
    }

    pub(crate) fn commit_user_candidate(
        &mut self,
        candidate: &UserOperationResult,
        operation_generation: Option<u64>,
    ) -> Result<Vec<String>, CompanionError> {
        let turn_id = candidate.prepared_response.id.clone();
        let reservation = self
            .storage
            .as_ref()
            .filter(|_| candidate.dispatch_seq != 0)
            .map(|storage| {
                storage.reserve_user_commit(
                    candidate.dispatch_seq,
                    &candidate.input_ids,
                    operation_generation,
                    &turn_id,
                )
            })
            .transpose()?;
        if reservation == Some(false) {
            return Err(CompanionError::Cancelled);
        }
        if reservation == Some(true) {
            let storage = self.storage.as_ref().expect("reserved user commit storage");
            if let Err(error) = storage.mark_turn_commit_persisting(&turn_id) {
                storage.prepare_turn_commit_recovery()?;
                return Err(error.into());
            }
        }
        let result = self.commit_user_candidate_inner(candidate, operation_generation, &turn_id);
        if result.is_ok() && reservation == Some(true) {
            let storage = self.storage.as_ref().expect("reserved user commit storage");
            storage.finalize_user_turn_commit(
                &turn_id,
                candidate.dispatch_seq,
                &candidate.input_ids,
            )?;
        }
        result
    }

    fn commit_user_candidate_inner(
        &mut self,
        candidate: &UserOperationResult,
        operation_generation: Option<u64>,
        turn_id: &str,
    ) -> Result<Vec<String>, CompanionError> {
        self.prepare_user_response(
            &candidate.input_ids,
            &candidate.prepared_response,
            operation_generation,
        )?;
        self.commit_session_summary(candidate.prepared_response.session_summary.clone())?;
        self.commit_call_side_effects(
            &candidate.response,
            &candidate.data,
            &candidate.source_ids,
            &candidate.observations,
        );
        self.commit_user_response(&candidate.input_ids, &candidate.prepared_response)?;
        let consumed = self.consume_user_observation_context(&candidate.input_ids, turn_id)?;
        Ok(consumed)
    }

    pub(super) fn consume_user_observation_context(
        &mut self,
        input_ids: &[String],
        turn_id: &str,
    ) -> Result<Vec<String>, CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(Vec::new());
        };
        let consumed = storage.consume_user_observation_context(input_ids, turn_id)?;
        self.completed_observation_ids
            .extend(consumed.iter().cloned());
        self.pending_observations
            .retain(|observation| !consumed.iter().any(|id| id == observation.id()));
        Ok(consumed)
    }

    fn complete_turn_input_ids(
        &self,
        mut input_ids: Vec<String>,
        accepted_mid_turn_ids: &Arc<Mutex<HashSet<String>>>,
    ) -> Result<Vec<String>, CompanionError> {
        let accepted = accepted_mid_turn_ids
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        if accepted.is_empty() {
            return Ok(input_ids);
        }
        let Some(storage) = &self.storage else {
            return Err(CompanionError::Persistence(PersistenceError::Invalid(
                "言い足しを復元する storage がありません".to_owned(),
            )));
        };
        for pending in storage.reconcile_pending_user_inputs()?.pending_inputs {
            let PendingInput::UserMessage(input) = pending;
            if accepted.contains(&input.id) && !input_ids.contains(&input.id) {
                input_ids.push(input.id);
            }
        }
        if accepted.iter().any(|id| !input_ids.contains(id)) {
            return Err(CompanionError::Persistence(PersistenceError::Invalid(
                "言い足しが cursor にありません".to_owned(),
            )));
        }
        Ok(input_ids)
    }

    fn prepare_user_response(
        &mut self,
        input_ids: &[String],
        response: &PreparedUserResponse,
        operation_generation: Option<u64>,
    ) -> Result<(), CompanionError> {
        if self.delivery_ownership == DeliveryOwnership::Owner {
            if let Some(storage) = &self.storage {
                let cancelled = storage.update_cursor(|cursor| {
                    if operation_generation
                        .is_some_and(|generation| cursor.user_operation_generation != generation)
                    {
                        return Ok(true);
                    }
                    if input_ids
                        .iter()
                        .any(|input_id| cursor.cancelled_input_ids.iter().any(|id| id == input_id))
                    {
                        return Ok(true);
                    }
                    for input_id in input_ids {
                        let Some(PendingInput::UserMessage(input)) = cursor
                            .pending_inputs
                            .iter()
                            .find(|input| input.id() == input_id)
                        else {
                            return Err(PersistenceError::Invalid(
                                "user input が cursor にありません".to_owned(),
                            ));
                        };
                        if input
                            .prepared_response
                            .as_ref()
                            .is_some_and(|existing| existing != response)
                        {
                            return Err(PersistenceError::Invalid(
                                "user input の prepared response が一致しません".to_owned(),
                            ));
                        }
                    }
                    for input in &mut cursor.pending_inputs {
                        if input_ids.iter().any(|id| input.id() == id) {
                            let PendingInput::UserMessage(input) = input;
                            input.prepared_response = Some(response.clone());
                        }
                    }
                    Ok(false)
                })?;
                if cancelled {
                    return Err(CompanionError::Cancelled);
                }
            }
            for input in self
                .pending_user_messages
                .iter_mut()
                .filter(|input| input_ids.contains(&input.id))
            {
                input.prepared_response = Some(response.clone());
            }
        }
        Ok(())
    }

    pub(super) fn commit_user_response(
        &mut self,
        input_ids: &[String],
        response: &PreparedUserResponse,
    ) -> Result<(), CompanionError> {
        if self.delivery_ownership == DeliveryOwnership::Owner {
            if let Some(storage) = &self.storage {
                let cancelled = storage.update_cursor(|cursor| {
                    if input_ids
                        .iter()
                        .any(|input_id| cursor.cancelled_input_ids.iter().any(|id| id == input_id))
                    {
                        return Ok(true);
                    }
                    for input_id in input_ids {
                        let Some(PendingInput::UserMessage(input)) = cursor
                            .pending_inputs
                            .iter()
                            .find(|input| input.id() == input_id)
                        else {
                            return Err(PersistenceError::Invalid(
                                "user input が cursor にありません".to_owned(),
                            ));
                        };
                        if input.prepared_response.as_ref() != Some(response) {
                            return Err(PersistenceError::Invalid(
                                "user input の prepared response が一致しません".to_owned(),
                            ));
                        }
                    }
                    for input in &mut cursor.pending_inputs {
                        if input_ids.iter().any(|id| input.id() == id) {
                            let PendingInput::UserMessage(input) = input;
                            input.response_commit_started = true;
                        }
                    }
                    Ok(false)
                })?;
                if cancelled {
                    return Err(CompanionError::Cancelled);
                }
            }
        }
        self.append_conversation_once(response.conversation_entry(input_ids))?;
        if self.delivery_ownership == DeliveryOwnership::Owner {
            self.publish_user_response(input_ids, response)?;
        }
        Ok(())
    }

    pub(crate) fn finalize_user_response(&mut self, input_id: &str) -> Result<(), CompanionError> {
        self.finalize_user_responses(&[input_id.to_owned()])
    }

    pub(crate) fn finalize_user_responses(
        &mut self,
        input_ids: &[String],
    ) -> Result<(), CompanionError> {
        if self.delivery_ownership == DeliveryOwnership::Owner {
            if let Some(storage) = &self.storage {
                let cursor = storage.load_cursor()?;
                if cursor
                    .pending_inputs
                    .iter()
                    .any(|input| input_ids.iter().any(|id| input.id() == id))
                {
                    storage.update_cursor(|cursor| {
                        cursor
                            .pending_inputs
                            .retain(|input| !input_ids.iter().any(|id| input.id() == id));
                        Ok(())
                    })?;
                }
            }
            self.normalize_pending_user_messages(input_ids)?;
        }
        Ok(())
    }

    pub(crate) fn normalize_pending_user_messages(
        &mut self,
        input_ids: &[String],
    ) -> Result<(), CompanionError> {
        if let Some(storage) = &self.storage {
            self.pending_user_messages = storage
                .reconcile_pending_user_inputs()?
                .pending_inputs
                .into_iter()
                .map(|pending| match pending {
                    PendingInput::UserMessage(input) => input,
                })
                .collect();
        } else {
            self.pending_user_messages
                .retain(|input| !input_ids.contains(&input.id));
        }
        Ok(())
    }

    fn ensure_user_input_pending(&self, input: &PendingUserMessage) -> Result<(), CompanionError> {
        if self.delivery_ownership != DeliveryOwnership::Owner {
            return Ok(());
        }
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        let cursor = storage.load_cursor()?;
        if cursor.cancelled_input_ids.iter().any(|id| id == &input.id) {
            return Err(CompanionError::Cancelled);
        }
        if !cursor
            .pending_inputs
            .iter()
            .any(|pending| pending.id() == input.id)
        {
            return Err(CompanionError::Persistence(PersistenceError::Invalid(
                "user input が cursor にありません".to_owned(),
            )));
        }
        Ok(())
    }

    fn publish_user_response(
        &self,
        input_ids: &[String],
        response: &PreparedUserResponse,
    ) -> Result<(), CompanionError> {
        let payload = serde_json::to_value(crate::notification::RemarkEnvelope {
            conversation_generation: self
                .storage
                .as_ref()
                .map(CompanionStorage::conversation_generation)
                .transpose()?
                .unwrap_or(0),
            entry_id: response.id.clone(),
            message: response.message.clone(),
            message_kind: "chat".to_owned(),
            notification_priority: "none".to_owned(),
            caused_by: input_ids.last().cloned(),
            caused_by_ids: input_ids.to_vec(),
        })?;
        for mailbox in &self.outgoing_mailboxes {
            mailbox.publish_with_identity(
                "remark".to_owned(),
                response.id.clone(),
                response.created_at.clone(),
                payload.clone(),
            )?;
        }
        Ok(())
    }
}

pub(super) fn common_prepared_response(
    inputs: &[PendingUserMessage],
) -> Result<Option<PreparedUserResponse>, CompanionError> {
    let prepared = inputs
        .iter()
        .find_map(|input| input.prepared_response.clone());
    if let Some(expected) = &prepared {
        if inputs
            .iter()
            .any(|input| input.prepared_response.as_ref() != Some(expected))
        {
            return Err(CompanionError::Persistence(PersistenceError::Invalid(
                "まとめ送りの prepared response が一致しません".to_owned(),
            )));
        }
    }
    Ok(prepared)
}

fn prepared_response(response: &PreparedUserResponse) -> CompanionResponse {
    CompanionResponse {
        emit: true,
        message: Some(response.message.clone()),
        message_kind: "chat".to_owned(),
        notification_priority: "none".to_owned(),
        thought: None,
        fact_candidates: Vec::new(),
        fact_updates: Vec::new(),
    }
}

#[cfg(test)]
#[path = "companion_user_test_barrier.rs"]
pub(crate) mod user_response_barrier;
