use super::*;

impl RuntimeActor {
    pub(super) fn schedule_initialization_retry(
        &mut self,
        kind: RuntimeErrorKind,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let delay = self.initialization_retry_delay;
        self.initialization_retry_at = Some(Instant::now() + delay);
        self.initialization_retry_delay = (delay * 2).min(Duration::from_secs(30));
        self.publish_retry_error(kind, snapshot_tx);
    }

    pub(super) fn schedule_user_retry(
        &mut self,
        kind: RuntimeErrorKind,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let delay = self.user_retry_delay;
        self.user_retry_at = Some(Instant::now() + delay);
        self.user_retry_delay = (delay * 2).min(Duration::from_secs(30));
        self.publish_retry_error(kind, snapshot_tx);
    }

    pub(super) fn pending_user_can_start(&self) -> bool {
        self.user_retry_at
            .is_none_or(|deadline| deadline <= Instant::now())
    }

    pub(super) fn queued_user_work(
        &self,
        volatile_users: &std::collections::VecDeque<crate::companion_storage::PendingUserMessage>,
    ) -> bool {
        if volatile_users
            .iter()
            .any(|input| !input.attachment_is_terminal())
        {
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

    pub(super) fn clear_non_attachment_error(&mut self) {
        self.last_error = self
            .last_error
            .take()
            .filter(|error| error.attachment_ocr.is_some());
    }

    pub(super) fn companion_recovery_can_start(&self) -> bool {
        self.companion.as_ref().is_some_and(|companion| {
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
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: None,
            issues: Vec::new(),
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

    pub(super) fn restore_terminal_attachment_failure(
        &mut self,
        companion: &CompanionAgent,
    ) -> Result<(), CompanionError> {
        let Some((input_id, failure)) = companion.first_terminal_attachment_failure()? else {
            if self
                .last_error
                .as_ref()
                .and_then(|error| error.attachment_ocr.as_ref())
                .is_some_and(|failure| !failure.retryable)
            {
                self.last_error = None;
            }
            return Ok(());
        };
        self.user_retry_at = None;
        self.user_retry_delay = Duration::from_secs(1);
        self.last_error = Some(RuntimeLastError {
            kind: RuntimeErrorKind::Provider,
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: None,
            issues: Vec::new(),
            attachment_ocr: Some(RuntimeAttachmentOcrFailure {
                input_id,
                reason: failure.reason,
                attempts: failure.attempts,
                retryable: false,
            }),
        });
        Ok(())
    }

    fn publish_retry_error(
        &mut self,
        kind: RuntimeErrorKind,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        self.last_error = Some(RuntimeLastError {
            kind,
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: None,
            issues: Vec::new(),
            attachment_ocr: None,
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
            cancelled_user_message_ids: self.cancelled_user_message_ids.clone(),
            companion_draft: self.companion_draft.clone(),
            latest_companion_thought: self.latest_companion_thought.clone(),
            provider_usage: self.provider_usage.clone(),
        }
    }

    pub(super) fn publish(&self, sender: &watch::Sender<RuntimeSnapshot>) {
        let _ = sender.send(self.snapshot());
    }
}
