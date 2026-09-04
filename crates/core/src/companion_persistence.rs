use super::support::conversation_entry_with_causes_at;
use super::user::common_prepared_response;
use super::*;
use crate::companion_storage::{
    ActiveTurnCommit, ObservationAttempt, ObservationConsumption, PendingDelivery, PendingInput,
    TurnCommitKind,
};
use crate::prompts::ordered_json_string;
use crate::state::ConversationRole;

impl CompanionAgent {
    pub(super) fn exclude_user_claimed_observations(
        &mut self,
        observations: Vec<ObservationRecord>,
    ) -> Result<Vec<ObservationRecord>, CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(observations);
        };
        let mut retained = Vec::new();
        for observation in observations {
            if storage.observation_claimed_by_user(&observation)? {
                self.completed_observation_ids
                    .insert(observation.id().to_owned());
            } else {
                retained.push(observation);
            }
        }
        Ok(retained)
    }

    pub(crate) fn has_pending_proactive_after_user(&self) -> bool {
        self.pending_user_messages.is_empty() && self.has_pending_proactive_observations()
    }

    pub(crate) fn has_pending_proactive_observations(&self) -> bool {
        if !self.pending_observations.is_empty() {
            return true;
        }
        self.storage.as_ref().is_some_and(|storage| {
            storage
                .load_cursor()
                .is_ok_and(|cursor| !cursor.pending.is_empty())
        })
    }

    pub(crate) fn proactive_recovery_can_start(
        &self,
        runtime_observations: &[ObservationRecord],
    ) -> bool {
        !self.has_pending_user_inputs().unwrap_or(true)
            && (self.has_pending_proactive_observations()
                || !runtime_observations.is_empty()
                || self.has_active_turn_commit().is_ok_and(|active| active))
    }

    pub(crate) fn pending_proactive_observations(
        &self,
    ) -> Result<Vec<ObservationRecord>, CompanionError> {
        let mut observations = self.pending_observations.clone();
        if let Some(storage) = &self.storage {
            observations.extend(storage.load_cursor()?.pending);
        }
        let mut seen = HashSet::new();
        observations.retain(|observation| seen.insert(observation.id().to_owned()));
        Ok(observations)
    }

    pub(super) fn defer_observations(
        &mut self,
        observations: &[ObservationRecord],
    ) -> Result<(), CompanionError> {
        self.mark_pending(observations)?;
        for observation in observations {
            if !self
                .pending_observations
                .iter()
                .any(|pending| pending.id() == observation.id())
            {
                self.pending_observations.push(observation.clone());
            }
        }
        Ok(())
    }

    pub(super) fn initialize_storage(&mut self) -> Result<(), CompanionError> {
        self.initialize_storage_with_pruning(true)
    }

    pub fn recover_persisted_state_before_conversation_archive(
        &mut self,
    ) -> Result<(), CompanionError> {
        self.conversation_pruning_enabled = false;
        if let Some(mailbox) = &self.incoming_mailbox {
            mailbox.recover()?;
        }
        if self.delivery_ownership == DeliveryOwnership::Owner {
            if let Some(storage) = &self.storage {
                storage.clear_transient_observation_markers()?;
            }
        }
        self.initialize_storage_with_pruning(false)?;
        if self.delivery_ownership == DeliveryOwnership::Owner {
            self.deliver_outbox()?;
            self.retry_pending_remarks()?;
        }
        Ok(())
    }

    fn initialize_storage_with_pruning(
        &mut self,
        prune_conversation: bool,
    ) -> Result<(), CompanionError> {
        if self.storage_loaded {
            return Ok(());
        }
        let Some(storage) = self.storage.clone() else {
            self.storage_loaded = true;
            return Ok(());
        };
        if prune_conversation {
            storage.prune_attachments()?;
        }
        self.conversation = storage.load_conversation()?;
        if let Some(summary) = storage.load_summary()? {
            self.previous_summary = Some(summary);
        }
        if self.delivery_ownership == DeliveryOwnership::None {
            self.storage_loaded = true;
            return Ok(());
        }
        let active_turn_commit = storage.prepare_turn_commit_recovery()?;
        let cursor = if prune_conversation {
            storage.reconcile_pending_user_inputs()?
        } else {
            storage.reconcile_pending_user_inputs_without_pruning()?
        };
        self.completed_observation_ids = cursor.ids.iter().cloned().collect();
        self.failed_observation_ids = cursor.failed.iter().cloned().collect();
        self.observation_attempts = cursor
            .observation_attempts
            .into_iter()
            .map(|attempt| (attempt.observation_id, attempt.attempts))
            .collect();
        self.pending_observations = cursor.pending.clone();
        self.pending_user_messages = cursor
            .pending_inputs
            .into_iter()
            .map(|input| match input {
                crate::companion_storage::PendingInput::UserMessage(input) => input,
            })
            .collect();
        self.restore_pending_deliveries(&storage, cursor.pending_deliveries)?;
        if let Some(commit) = active_turn_commit {
            self.recover_persisting_turn_commit(&commit)?;
        }
        self.storage_loaded = true;
        Ok(())
    }

    pub(crate) fn recover_active_turn_commit(&mut self) -> Result<(), CompanionError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        let Some(commit) = storage.prepare_turn_commit_recovery()? else {
            return Ok(());
        };
        self.recover_persisting_turn_commit(&commit)
    }

    pub(crate) fn has_active_turn_commit(&self) -> Result<bool, CompanionError> {
        match &self.storage {
            Some(storage) => Ok(storage.load_cursor()?.active_turn_commit.is_some()),
            None => Ok(false),
        }
    }

    fn recover_persisting_turn_commit(
        &mut self,
        commit: &ActiveTurnCommit,
    ) -> Result<(), CompanionError> {
        match commit.kind {
            TurnCommitKind::User => self.recover_persisting_user_commit(commit),
            TurnCommitKind::Proactive => self.recover_persisting_proactive_commit(commit),
        }
    }

    fn recover_persisting_user_commit(
        &mut self,
        commit: &ActiveTurnCommit,
    ) -> Result<(), CompanionError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        let cursor = storage.load_cursor()?;
        let inputs = commit
            .target_ids
            .iter()
            .filter_map(|id| {
                cursor
                    .pending_inputs
                    .iter()
                    .find_map(|pending| match pending {
                        PendingInput::UserMessage(input) if &input.id == id => Some(input.clone()),
                        _ => None,
                    })
            })
            .collect::<Vec<_>>();
        if inputs.is_empty()
            && self
                .conversation
                .iter()
                .any(|entry| entry.id == commit.turn_id && entry.caused_by_ids == commit.target_ids)
        {
            storage.finalize_user_turn_commit(
                &commit.turn_id,
                commit
                    .dispatch_seq
                    .expect("user commit has dispatch sequence"),
                &commit.target_ids,
            )?;
            return Ok(());
        }
        let response = match common_prepared_response(&inputs) {
            Ok(Some(response)) => response,
            Ok(None) => {
                storage.quarantine_turn_commit_recovery(commit, "prepared-response-missing")?;
                return Ok(());
            }
            Err(CompanionError::Persistence(PersistenceError::Invalid(_))) => {
                storage.quarantine_turn_commit_recovery(commit, "prepared-response-mismatch")?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if inputs.len() != commit.target_ids.len() {
            storage.quarantine_turn_commit_recovery(commit, "user-input-missing")?;
            return Ok(());
        }
        self.commit_session_summary(response.session_summary.clone())?;
        self.commit_user_response(&commit.target_ids, &response)?;
        let consumed =
            self.consume_user_observation_context(&commit.target_ids, &commit.turn_id)?;
        self.pending_observations
            .retain(|observation| !consumed.iter().any(|id| id == observation.id()));
        storage.finalize_user_turn_commit(
            &commit.turn_id,
            commit
                .dispatch_seq
                .expect("user commit has dispatch sequence"),
            &commit.target_ids,
        )?;
        self.pending_user_messages
            .retain(|input| !commit.target_ids.contains(&input.id));
        Ok(())
    }

    fn recover_persisting_proactive_commit(
        &mut self,
        commit: &ActiveTurnCommit,
    ) -> Result<(), CompanionError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        let cursor = storage.load_cursor()?;
        let completed = commit
            .target_ids
            .iter()
            .all(|id| cursor.ids.iter().any(|completed| completed == id));
        let has_pending_delivery = cursor.pending_deliveries.iter().any(|delivery| {
            delivery
                .observation_ids
                .iter()
                .any(|id| commit.target_ids.iter().any(|target| target == id))
        });
        if completed {
            storage.clear_active_turn_commit(&commit.turn_id)?;
            return Ok(());
        }
        if has_pending_delivery {
            if let Err(error) = self.retry_pending_remarks() {
                if let Some(logger) = &self.logger {
                    let _ = logger.write(
                        "WARN",
                        &format!(
                            "active TurnCommit の delivery recovery を保留しました: error-type=persistence ({error})"
                        ),
                    );
                }
                storage.quarantine_turn_commit_recovery(
                    commit,
                    "proactive-delivery-recovery-failed",
                )?;
                return Ok(());
            }
            storage.clear_active_turn_commit(&commit.turn_id)?;
            return Ok(());
        }
        storage.quarantine_turn_commit_recovery(commit, "proactive-persist-incomplete")?;
        Ok(())
    }

    fn restore_pending_deliveries(
        &mut self,
        storage: &CompanionStorage,
        deliveries: Vec<PendingDelivery>,
    ) -> Result<(), CompanionError> {
        for mut delivery in deliveries {
            let outbox_enqueued = storage.outbox().contains("remark", &delivery.remark_id)?;
            if delivery.enqueued != outbox_enqueued {
                let expected = delivery.clone();
                storage.update_cursor(|cursor| {
                    let Some(current) = cursor
                        .pending_deliveries
                        .iter_mut()
                        .find(|current| current.remark_id == expected.remark_id)
                    else {
                        return Ok(());
                    };
                    let mut expected_current = expected;
                    expected_current.enqueued = current.enqueued;
                    if *current != expected_current {
                        return Err(PersistenceError::Invalid(
                            "pending delivery の payload が一致しません".to_owned(),
                        ));
                    }
                    current.enqueued = outbox_enqueued;
                    Ok(())
                })?;
                delivery.enqueued = outbox_enqueued;
            }
            self.pending_delivery_observation_ids
                .extend(delivery.observation_ids.iter().cloned());
            self.queue_pending_remark(delivery);
        }
        Ok(())
    }

    pub(super) fn prepare_pending_remark(
        &mut self,
        entry: &ConversationEntry,
        message_kind: &str,
    ) -> Result<Option<PendingDelivery>, CompanionError> {
        let delivery = PendingDelivery {
            remark_id: entry.id.clone(),
            created_at: entry.created_at.clone(),
            proactive_date: local_date_at(self.clock.now()),
            message: entry.message.clone(),
            message_kind: message_kind.to_owned(),
            notification_priority: entry.notification_priority.clone(),
            observation_ids: entry.observation_ids().map(str::to_owned).collect(),
            enqueued: false,
        };
        delivery.validate_payload_size()?;
        let mut already_completed = false;
        if let Some(storage) = &self.storage {
            storage.update_cursor(|cursor| {
                if delivery
                    .observation_ids
                    .iter()
                    .all(|id| cursor.ids.contains(id))
                {
                    already_completed = true;
                    return Ok(());
                }
                if let Some(existing) = cursor
                    .pending_deliveries
                    .iter()
                    .find(|existing| existing.remark_id == delivery.remark_id)
                {
                    if existing != &delivery {
                        return Err(PersistenceError::Invalid(
                            "pending delivery の remarkId が重複しています".to_owned(),
                        ));
                    }
                } else {
                    cursor.pending_deliveries.push(delivery.clone());
                }
                Ok(())
            })?;
        }
        if already_completed {
            return Ok(None);
        }
        self.pending_delivery_observation_ids
            .extend(delivery.observation_ids.iter().cloned());
        self.queue_pending_remark(delivery.clone());
        Ok(Some(delivery))
    }

    pub(super) fn ensure_pending_remark_counted(
        &mut self,
        delivery: &PendingDelivery,
    ) -> Result<bool, CompanionError> {
        let Some(storage) = &self.storage else {
            if self.proactive_emit_ids.contains(&delivery.remark_id) {
                return Ok(true);
            }
            if self
                .config
                .daily_proactive_limit
                .is_some_and(|limit| self.proactive_calls_today >= limit)
            {
                return Ok(false);
            }
            self.proactive_emit_ids.insert(delivery.remark_id.clone());
            self.proactive_calls_today = self.proactive_calls_today.saturating_add(1);
            return Ok(true);
        };
        let Some(usage) = storage.try_record_proactive_emit(
            &delivery.proactive_date,
            self.config.daily_proactive_limit,
            &delivery.remark_id,
        )?
        else {
            return Ok(false);
        };
        self.day_key = usage.date;
        self.proactive_calls_today = usage.proactive_calls;
        self.proactive_emit_ids = usage.proactive_emit_ids.into_iter().collect();
        self.total_calls_today = usage.total_calls;
        Ok(true)
    }

    pub(super) fn persist_proactive_response(
        &mut self,
        response: &mut CompanionResponse,
        observations: &[ObservationRecord],
    ) -> Result<(bool, bool), CompanionError> {
        if !response.emit {
            return Ok((false, false));
        }
        let Some(message) = response.message.clone() else {
            return Ok((false, false));
        };
        let entry = conversation_entry_with_causes_at(
            self.clock.now(),
            ConversationRole::Companion,
            message,
            &response.notification_priority,
            observations
                .iter()
                .map(|observation| observation.id().to_owned())
                .collect(),
        );
        let Some(delivery) = self.prepare_pending_remark(&entry, &response.message_kind)? else {
            return Ok((true, false));
        };
        if !self.ensure_pending_remark_counted(&delivery)? {
            self.complete_pending_remark(&delivery)?;
            *response = silent_response();
            return Ok((false, false));
        }
        self.append_pending_remark_conversation(&delivery)?;
        if self.publish_remark(&delivery)? {
            self.mark_pending_remark_enqueued(&delivery.remark_id)?;
            let mut enqueued_delivery = delivery;
            enqueued_delivery.enqueued = true;
            self.complete_pending_remark(&enqueued_delivery)?;
        }
        Ok((true, true))
    }

    pub(super) fn mark_pending_remark_enqueued(
        &mut self,
        remark_id: &str,
    ) -> Result<(), CompanionError> {
        if let Some(storage) = &self.storage {
            storage.update_cursor(|cursor| {
                let Some(delivery) = cursor
                    .pending_deliveries
                    .iter_mut()
                    .find(|delivery| delivery.remark_id == remark_id)
                else {
                    return Ok(());
                };
                delivery.enqueued = true;
                Ok(())
            })?;
        }
        if let Some(pending) = self
            .pending_remarks
            .iter_mut()
            .find(|pending| pending.delivery.remark_id == remark_id)
        {
            pending.delivery.enqueued = true;
        }
        Ok(())
    }

    pub(super) fn complete_pending_remark(
        &mut self,
        delivery: &PendingDelivery,
    ) -> Result<(), CompanionError> {
        if let Some(storage) = &self.storage {
            storage.update_cursor(|cursor| {
                let before = cursor.pending_deliveries.len();
                cursor
                    .pending_deliveries
                    .retain(|pending| pending.remark_id != delivery.remark_id);
                if before == cursor.pending_deliveries.len() {
                    return Ok(());
                }
                cursor.pending.retain(|observation| {
                    !delivery
                        .observation_ids
                        .iter()
                        .any(|id| id == observation.id())
                });
                for id in &delivery.observation_ids {
                    if !cursor.ids.contains(id) {
                        cursor.ids.push(id.clone());
                    }
                }
                if cursor.ids.len() > 500 {
                    cursor.ids.drain(..cursor.ids.len() - 500);
                }
                cursor.observation_attempts.retain(|attempt| {
                    !delivery
                        .observation_ids
                        .iter()
                        .any(|id| id == &attempt.observation_id)
                });
                for id in &delivery.observation_ids {
                    if !cursor
                        .observation_consumptions
                        .iter()
                        .any(|consumption| &consumption.observation_id == id)
                    {
                        cursor
                            .observation_consumptions
                            .push(ObservationConsumption {
                                observation_id: id.clone(),
                                turn_id: delivery.remark_id.clone(),
                                reason: "proactive-delivery".to_owned(),
                            });
                    }
                }
                Ok(())
            })?;
        }
        self.completed_observation_ids
            .extend(delivery.observation_ids.iter().cloned());
        for id in &delivery.observation_ids {
            self.observation_attempts.remove(id);
            self.pending_delivery_observation_ids.remove(id);
        }
        self.pending_remarks
            .retain(|pending| pending.delivery.remark_id != delivery.remark_id);
        self.proactive_emit_ids.remove(&delivery.remark_id);
        if let Some(storage) = &self.storage {
            if storage
                .forget_proactive_emit_id(&delivery.remark_id)
                .is_err()
            {
                if let Some(logger) = &self.logger {
                    let _ = logger.write(
                        "WARN",
                        "自発発話の冪等 marker を整理できませんでした: error-type=usage",
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn mark_pending(
        &self,
        observations: &[ObservationRecord],
    ) -> Result<(), CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        storage.update_cursor(|cursor| {
            for observation in observations {
                if self.completed_observation_ids.contains(observation.id())
                    || self.failed_observation_ids.contains(observation.id())
                    || cursor
                        .pending
                        .iter()
                        .any(|item| item.id() == observation.id())
                {
                    continue;
                }
                cursor.pending.push(observation.clone());
            }
            if cursor.pending.len() > MAX_PENDING_OBSERVATIONS {
                cursor
                    .pending
                    .drain(..cursor.pending.len() - MAX_PENDING_OBSERVATIONS);
            }
            Ok(())
        })?;
        Ok(())
    }

    pub(super) fn complete_observations_for_turn(
        &mut self,
        observations: &[ObservationRecord],
        turn_id: &str,
        reason: &str,
    ) -> Result<(), CompanionError> {
        let ids = observations
            .iter()
            .map(|observation| observation.id().to_owned())
            .collect::<Vec<_>>();
        self.complete_observation_ids(ids, turn_id, reason)
    }

    pub(super) fn complete_observation_ids(
        &mut self,
        ids: Vec<String>,
        turn_id: &str,
        reason: &str,
    ) -> Result<(), CompanionError> {
        if let Some(storage) = &self.storage {
            storage.update_cursor(|cursor| {
                cursor
                    .pending
                    .retain(|observation| !ids.iter().any(|id| id == observation.id()));
                for id in &ids {
                    if !cursor.ids.iter().any(|existing| existing == id) {
                        cursor.ids.push(id.clone());
                    }
                }
                if cursor.ids.len() > 500 {
                    cursor.ids.drain(..cursor.ids.len() - 500);
                }
                cursor
                    .observation_attempts
                    .retain(|attempt| !ids.iter().any(|id| id == &attempt.observation_id));
                for id in &ids {
                    if !cursor
                        .observation_consumptions
                        .iter()
                        .any(|consumption| &consumption.observation_id == id)
                    {
                        cursor
                            .observation_consumptions
                            .push(ObservationConsumption {
                                observation_id: id.clone(),
                                turn_id: turn_id.to_owned(),
                                reason: reason.to_owned(),
                            });
                    }
                }
                Ok(())
            })?;
        }
        self.completed_observation_ids.extend(ids.iter().cloned());
        for id in ids {
            self.observation_attempts.remove(&id);
            self.pending_delivery_observation_ids.remove(&id);
        }
        Ok(())
    }

    pub(super) fn restore_observations(
        &mut self,
        observations: &[ObservationRecord],
        error: &CompanionError,
    ) -> Result<(), CompanionError> {
        let mut failed = HashSet::new();
        let deterministic =
            error.observation_failure_kind() == ObservationFailureKind::DeterministicObservation;
        let mut next_attempts = HashMap::new();
        for observation in observations {
            let id = observation.id().to_owned();
            if self.completed_observation_ids.contains(&id)
                || self.failed_observation_ids.contains(&id)
            {
                continue;
            }
            if deterministic {
                let attempts = self
                    .observation_attempts
                    .get(&id)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1);
                next_attempts.insert(id.clone(), attempts);
                if attempts >= MAX_OBSERVATION_ATTEMPTS {
                    failed.insert(id);
                }
            }
        }
        for observation in observations {
            if failed.contains(observation.id())
                || self.completed_observation_ids.contains(observation.id())
                || self.failed_observation_ids.contains(observation.id())
                || self
                    .pending_observations
                    .iter()
                    .any(|pending| pending.id() == observation.id())
            {
                continue;
            }
            self.pending_observations.push(observation.clone());
        }
        if self.pending_observations.len() > MAX_PENDING_OBSERVATIONS {
            self.pending_observations
                .drain(..self.pending_observations.len() - MAX_PENDING_OBSERVATIONS);
        }
        if let Some(storage) = &self.storage {
            let failed_for_write = failed.clone();
            storage.update_cursor(|cursor| {
                for (id, attempts) in &next_attempts {
                    if failed_for_write.contains(id) {
                        cursor
                            .observation_attempts
                            .retain(|attempt| &attempt.observation_id != id);
                        continue;
                    }
                    if let Some(existing) = cursor
                        .observation_attempts
                        .iter_mut()
                        .find(|attempt| &attempt.observation_id == id)
                    {
                        existing.attempts = existing.attempts.max(*attempts);
                    } else {
                        cursor.observation_attempts.push(ObservationAttempt {
                            observation_id: id.clone(),
                            attempts: *attempts,
                        });
                    }
                }
                cursor
                    .pending
                    .retain(|observation| !failed.contains(observation.id()));
                cursor.failed.extend(failed_for_write.iter().cloned());
                if cursor.failed.len() > 500 {
                    cursor.failed.drain(..cursor.failed.len() - 500);
                }
                Ok(())
            })?;
        }
        self.failed_observation_ids.extend(failed.iter().cloned());
        for (id, attempts) in next_attempts {
            if failed.contains(&id) {
                self.observation_attempts.remove(&id);
            } else {
                self.observation_attempts.insert(id, attempts);
            }
        }
        Ok(())
    }

    pub(super) fn append_pending_remark_conversation(
        &mut self,
        delivery: &PendingDelivery,
    ) -> Result<(), CompanionError> {
        self.append_conversation_once(delivery.conversation_entry())
    }

    pub(super) fn append_conversation_once(
        &mut self,
        entry: ConversationEntry,
    ) -> Result<(), CompanionError> {
        if let Some(existing) = self
            .conversation
            .iter()
            .find(|existing| existing.id == entry.id)
        {
            if existing != &entry {
                return Err(CompanionError::Persistence(PersistenceError::Invalid(
                    "同じ ID の conversation が異なる内容を持ちます".to_owned(),
                )));
            }
            return Ok(());
        }
        if let Some(storage) = &self.storage {
            let now = self.clock.now();
            let appended = if self.conversation_pruning_enabled {
                storage.append_conversation_once_at(&entry, now)?
            } else {
                storage.append_conversation_once_at_without_pruning(&entry, now)?
            };
            if appended {
                storage.record_stagnation_reaction_at(now);
            }
        }
        self.remember_conversation_entry(entry);
        Ok(())
    }

    pub(super) fn remember_conversation_entry(&mut self, entry: ConversationEntry) {
        self.conversation.push(entry);
        if self.conversation.len() > MAX_CONVERSATION_ENTRIES {
            self.conversation
                .drain(..self.conversation.len() - MAX_CONVERSATION_ENTRIES);
        }
    }

    pub(super) fn conversation_jsonl(&self) -> Result<Option<String>, CompanionError> {
        self.conversation_jsonl_excluding(&[])
    }

    pub(super) fn conversation_jsonl_excluding(
        &self,
        excluded_ids: &[String],
    ) -> Result<Option<String>, CompanionError> {
        if self.conversation.is_empty() {
            return Ok(None);
        }
        let mut entries = self
            .conversation
            .iter()
            .rev()
            .filter(|entry| !excluded_ids.contains(&entry.id))
            .take(20)
            .collect::<Vec<_>>();
        entries.reverse();
        let lines = entries
            .into_iter()
            .map(|entry| {
                ordered_json_string(&serde_json::json!({
                    "role": entry.role,
                    "message": entry.message,
                }))
            })
            .collect::<Vec<_>>();
        Ok(Some(lines.join("\n")))
    }

    pub(super) fn system_prompt(&self) -> String {
        let assertiveness = self
            .temporary_assertiveness
            .effective(&self.config.assertiveness, self.clock.now());
        companion_system_prompt(&assertiveness, &self.display_name, &self.persona)
    }

    pub(super) fn accept_session(
        &mut self,
        request: &SessionRequest,
        returned: Option<ProviderSession>,
    ) -> Result<(), CompanionError> {
        let mut context_compacted = false;
        match (request, returned) {
            (SessionRequest::New, Some(session)) => {
                self.validate_returned_session(&session)?;
                self.session = Some(session);
            }
            (SessionRequest::New, None) => {
                return Err(CompanionError::Provider(ProviderError {
                    kind: ProviderErrorKind::InvalidOutput,
                    message: "companion provider が有効な session id を返しませんでした".to_owned(),
                }));
            }
            (SessionRequest::Resume(expected), Some(session)) => {
                if session.provider != expected.provider {
                    return Err(CompanionError::Provider(ProviderError {
                        kind: ProviderErrorKind::InvalidOutput,
                        message: "companion provider の session provider が一致しません".to_owned(),
                    }));
                }
                self.validate_returned_session(&session)?;
                context_compacted = session.id != expected.id;
                self.session = Some(session);
            }
            (SessionRequest::Resume(_), None) => {}
            (SessionRequest::Ephemeral, _) => {}
        }
        self.needs_session_context = context_compacted;
        Ok(())
    }

    pub(super) fn validate_returned_session(
        &self,
        session: &ProviderSession,
    ) -> Result<(), CompanionError> {
        let expected_model = (self.config.model != "default").then_some(self.config.model.as_str());
        if session.id.trim().is_empty() {
            return Err(CompanionError::Provider(ProviderError {
                kind: ProviderErrorKind::InvalidOutput,
                message: "companion provider の session id が空です".to_owned(),
            }));
        }
        if self
            .provider
            .provider_name()
            .is_some_and(|provider| provider != session.provider)
        {
            return Err(CompanionError::Provider(ProviderError {
                kind: ProviderErrorKind::InvalidOutput,
                message: "companion provider の session provider が一致しません".to_owned(),
            }));
        }
        if session.model.as_deref() != expected_model {
            return Err(CompanionError::Provider(ProviderError {
                kind: ProviderErrorKind::InvalidOutput,
                message: "companion provider の session model が一致しません".to_owned(),
            }));
        }
        Ok(())
    }

    pub(super) fn commit_session_summary(
        &mut self,
        summary: Option<String>,
    ) -> Result<(), CompanionError> {
        let Some(summary) = summary else {
            return Ok(());
        };
        if let Some(storage) = &self.storage {
            storage.save_summary(&summary)?;
        }
        self.previous_summary = Some(summary.clone());
        if self.pending_session_summary.as_ref() == Some(&summary) {
            self.pending_session_summary = None;
        }
        Ok(())
    }

    pub fn reset_session(&mut self) {
        self.session = None;
        self.session_calls = 0;
        self.needs_session_context = true;
    }

    pub fn update_config(&mut self, config: CompanionConfig) {
        if self.config.provider != config.provider
            || self.config.model != config.model
            || self.config.executable != config.executable
        {
            self.reset_session();
        }
        self.display_name = config.display_name.clone();
        self.config = config;
    }

    pub fn session(&self) -> Option<ProviderSession> {
        self.session.clone()
    }

    pub fn usage(&self) -> (u32, u32) {
        (self.proactive_calls_today, self.total_calls_today)
    }
}
