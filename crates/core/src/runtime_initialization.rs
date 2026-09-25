use super::*;

impl RuntimeActor {
    pub(super) fn reset_companion_emotions(&mut self) -> Result<(), RuntimeError> {
        self.user_preparer
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .ok_or(RuntimeError::CompanionUnavailable)?
            .reset_companion_emotions()?;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }
    pub(super) fn schedule_initialization_retry(
        &mut self,
        kind: RuntimeErrorKind,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let delay = self.initialization_retry_delay;
        self.initialization_retry_at = Some(Instant::now() + delay);
        self.initialization_retry_delay = (delay * 2).min(Duration::from_secs(30));
        self.publish_retry_error(kind, RuntimeErrorSource::Companion, None, snapshot_tx);
    }

    pub(super) fn schedule_user_retry(
        &mut self,
        kind: RuntimeErrorKind,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let delay = self.user_retry_delay;
        self.user_retry_at = Some(Instant::now() + delay);
        self.user_retry_delay = (delay * 2).min(Duration::from_secs(30));
        self.publish_retry_error(
            kind,
            RuntimeErrorSource::UserResponse,
            self.active_user_message_id.clone(),
            snapshot_tx,
        );
    }

    pub(super) fn pending_user_can_start(&self) -> bool {
        self.user_retry_at
            .is_none_or(|deadline| deadline <= Instant::now())
    }

    pub(super) fn queued_user_work(
        &self,
        volatile_users: &std::collections::VecDeque<crate::companion_storage::PendingUserMessage>,
    ) -> bool {
        if volatile_users.iter().any(|input| !input.is_terminal()) {
            return true;
        }
        self.companion
            .as_ref()
            .is_some_and(|companion| companion.has_runnable_user_inputs().unwrap_or(true))
    }

    pub(super) fn user_work_is_pending(
        &self,
        volatile_users: &std::collections::VecDeque<crate::companion_storage::PendingUserMessage>,
    ) -> bool {
        self.user_work_pending || self.queued_user_work(volatile_users)
    }

    pub(super) fn clear_observation_in_progress(&mut self) {
        if let Some(companion) = self.companion.as_mut() {
            let _ = companion.set_pending_observation_in_progress(false);
        }
    }

    pub(super) fn clear_non_user_error(&mut self) {
        self.last_error = self
            .last_error
            .take()
            .filter(|error| error.is_attachment_error() || error.is_user_response_error());
    }

    pub(super) fn complete_user_response(
        &mut self,
        _companion: &CompanionAgent,
        input_ids: &[String],
    ) {
        if input_ids.is_empty() {
            return;
        }
        if self
            .last_error
            .as_ref()
            .is_some_and(RuntimeLastError::is_user_response_error)
        {
            self.last_error = None;
            self.user_retry_at = None;
            self.user_retry_delay = Duration::from_secs(1);
        }
    }

    pub(super) fn companion_recovery_can_start(&self) -> bool {
        self.observation_delivery == ObservationDelivery::Companion
            && self.companion.as_ref().is_some_and(|companion| {
                companion.proactive_recovery_can_start(&self.pending_observations)
            })
    }

    pub(super) fn resume_proactive_after_user(
        &mut self,
        companion: &CompanionAgent,
        user_settled: bool,
    ) {
        if user_settled && companion.has_pending_proactive_after_user() {
            self.companion_recovery_pending = true;
            self.companion_recovery_at = None;
        }
    }

    pub(super) fn preserve_proactive_during_user_failure(&mut self, companion: &CompanionAgent) {
        self.companion_recovery_pending = companion.has_pending_proactive_observations();
    }

    pub(super) fn record_attachment_ocr_failure(
        &mut self,
        companion: &CompanionAgent,
        input_id: &str,
        reason: AttachmentOcrFailureKind,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> Result<bool, CompanionError> {
        let failure = companion.record_attachment_failure(input_id, reason)?;
        let retryable = !failure.terminal;
        let error = RuntimeLastError {
            kind: RuntimeErrorKind::Provider,
            source: RuntimeErrorSource::AttachmentOcr,
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: None,
            issues: Vec::new(),
            user_response: None,
            user_input_id: None,
            attachment_ocr: Some(RuntimeAttachmentOcrFailure {
                input_id: input_id.to_owned(),
                reason,
                attempts: failure.attempts,
                retryable,
            }),
        };
        if retryable {
            let delay = self.user_retry_delay;
            self.user_retry_at = Some(Instant::now() + delay);
            self.user_retry_delay = (delay * 2).min(Duration::from_secs(30));
            self.last_error = Some(error);
            self.revision = self.revision.saturating_add(1);
            self.publish(snapshot_tx);
        } else {
            self.user_retry_at = None;
            self.user_retry_delay = Duration::from_secs(1);
            self.last_error = Some(error);
            self.revision = self.revision.saturating_add(1);
            self.publish(snapshot_tx);
        }
        Ok(retryable)
    }

    pub(super) fn restore_terminal_user_failure(
        &mut self,
        companion: &CompanionAgent,
        kind: RuntimeErrorKind,
    ) -> Result<(), CompanionError> {
        let terminal_attachment = companion.first_terminal_attachment_failure()?;
        let terminal_user_response =
            companion
                .terminal_user_responses()?
                .into_iter()
                .find(|(input_id, _, _)| {
                    !self.suppressed_terminal_user_failure_ids.contains(input_id)
                        && !self
                            .cancelled_user_message_ids
                            .iter()
                            .any(|cancelled| cancelled == input_id)
                });
        if let Some(error) = self.last_error.as_ref() {
            if !error.is_user_response_error() && !error.is_attachment_error() {
                return Ok(());
            }
            if error.is_attachment_error() {
                // 切替前の添付 OCR エラーは、切替先の cursor にない場合も保持する。
                let same_attachment = error.attachment_ocr.as_ref().is_some_and(|current| {
                    terminal_attachment
                        .as_ref()
                        .is_some_and(|(input_id, _)| input_id == &current.input_id)
                });
                if !same_attachment {
                    return Ok(());
                }
            } else {
                let current_input_id = error.user_input_id.as_deref().or_else(|| {
                    error
                        .user_response
                        .as_ref()
                        .map(|failure| failure.input_id.as_str())
                });
                let same_user_response = terminal_user_response
                    .as_ref()
                    .is_some_and(|(input_id, _, _)| current_input_id == Some(input_id.as_str()));
                if !same_user_response {
                    return Ok(());
                }
            }
        }
        let Some((input_id, failure)) = terminal_attachment else {
            if let Some((input_id, attempts, provider)) = terminal_user_response {
                self.restore_settled_user_failure(&[(input_id, attempts, provider)], kind);
                return Ok(());
            }
            return Ok(());
        };
        self.user_retry_at = None;
        self.user_retry_delay = Duration::from_secs(1);
        self.last_error = Some(RuntimeLastError {
            kind,
            source: RuntimeErrorSource::AttachmentOcr,
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: None,
            issues: Vec::new(),
            user_response: None,
            user_input_id: None,
            attachment_ocr: Some(RuntimeAttachmentOcrFailure {
                input_id,
                reason: failure.reason,
                attempts: failure.attempts,
                retryable: false,
            }),
        });
        Ok(())
    }

    pub(super) fn restore_settled_user_failure(
        &mut self,
        failures: &[(String, u8, Option<crate::provider::ProviderFailureSummary>)],
        kind: RuntimeErrorKind,
    ) {
        for (input_id, _, _) in failures {
            self.suppressed_terminal_user_failure_ids
                .insert(input_id.clone());
        }
        let Some((input_id, attempts, provider)) = failures.last().cloned() else {
            return;
        };
        self.user_retry_at = None;
        self.user_retry_delay = Duration::from_secs(1);
        let kind = if provider
            .as_ref()
            .is_some_and(|failure| failure.kind == crate::provider::ProviderErrorKind::Timeout)
        {
            RuntimeErrorKind::ProviderTimeout
        } else {
            kind
        };
        self.last_error = Some(RuntimeLastError {
            kind,
            source: RuntimeErrorSource::UserResponse,
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: None,
            issues: Vec::new(),
            attachment_ocr: None,
            user_response: Some(RuntimeUserResponseFailure {
                input_id,
                attempts,
                provider,
            }),
            user_input_id: None,
        });
    }

    fn publish_retry_error(
        &mut self,
        kind: RuntimeErrorKind,
        source: RuntimeErrorSource,
        user_input_id: Option<String>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        self.last_error = Some(RuntimeLastError {
            kind,
            source,
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: None,
            issues: Vec::new(),
            attachment_ocr: None,
            user_response: None,
            user_input_id,
        });
        self.revision = self.revision.saturating_add(1);
        self.publish(snapshot_tx);
    }

    pub(super) fn snapshot(&self) -> RuntimeSnapshot {
        let (pending_deliveries, delivery_outbox_blocked) = self
            .companion
            .as_ref()
            .map_or((0, false), CompanionAgent::pending_delivery_status);
        RuntimeSnapshot {
            companion_emotions: self
                .user_preparer
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map_or_else(crate::emotion::EmotionState::default, |preparer| {
                    preparer.companion_emotions()
                }),
            revision: self.revision,
            phase: self.phase,
            pending_observations: self.pending_observations.len(),
            last_error: self.last_error.clone(),
            companion_retry_in_seconds: self
                .initialization_retry_at
                .into_iter()
                .chain(self.user_retry_at)
                .min()
                .map(|deadline| {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
                }),
            pending_deliveries,
            delivery_outbox_blocked,
            memory_status: self
                .memory
                .as_ref()
                .map_or_else(MemoryStatus::default, |service| service.status().clone()),
            companion_display_name: self.companion_display_name.clone(),
            proactive_limit_reached: self
                .companion
                .as_ref()
                .is_some_and(CompanionAgent::proactive_limit_reached),
            active_user_message_id: self.active_user_message_id.clone(),
            user_work_pending: self.user_work_pending,
            cancelled_user_message_ids: self.cancelled_user_message_ids.clone(),
            companion_draft: self.companion_draft.clone(),
            conversation_revision: self.conversation_revision,
            latest_companion_thought: self.latest_companion_thought.clone(),
            latest_companion_decision: self.latest_companion_decision.clone(),
            latest_judge_decision: self.latest_judge_decision.clone(),
            latest_user_interruption: self.latest_user_interruption.clone(),
            latest_companion_thought_generation: self.latest_companion_thought_generation,
            provider_usage: self.provider_usage.clone(),
        }
    }

    pub(super) fn publish(&self, sender: &watch::Sender<RuntimeSnapshot>) {
        self.sync_microphone_command_policy();
        let mut next = self.snapshot();
        sender.send_if_modified(|current| {
            let revision = next.revision;
            next.revision = current.revision;
            if *current == next {
                current.revision = revision;
                return false;
            }
            next.revision = revision;
            *current = next;
            true
        });
    }
}
