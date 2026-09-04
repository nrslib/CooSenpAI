use super::operation_state::{OperationCancellationCause, OperationCancellationReason};
use super::*;

pub fn empty_runtime(config: Config) -> RuntimeHandle {
    RuntimeActor::spawn(config, None, None)
}

impl RuntimeHandle {
    pub async fn replace_config_when_idle(
        &self,
        config: Config,
        agents: RuntimeAgents,
    ) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        validate_config(&config).map_err(RuntimeError::from)?;
        self.advance_watch_scope_generation(&config);
        self.operation_cancellation
            .cancel_lane_for_config_update(OperationLane::Observer);
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::ReplaceConfigWhenIdle {
                config: Box::new(config),
                agents: Box::new(agents),
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn update_watch_enabled(&self, enabled: bool) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::UpdateWatchEnabled { enabled, response })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn replace_companion(&self, companion: CompanionAgent) -> Result<u64, RuntimeError> {
        self.replace_companion_inner(companion, None).await
    }

    pub async fn replace_companion_with_config(
        &self,
        config: Config,
        companion: CompanionAgent,
    ) -> Result<u64, RuntimeError> {
        self.replace_companion_inner(companion, Some(config)).await
    }

    async fn replace_companion_inner(
        &self,
        companion: CompanionAgent,
        config: Option<Config>,
    ) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::ReplaceCompanion {
                companion: Box::new(companion),
                config: config.map(Box::new),
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }
}

pub(super) fn drain_closed_commands(
    control_rx: &mut mpsc::Receiver<ControlCommand>,
    priority_rx: &mut mpsc::Receiver<PriorityCommand>,
    user_rx: &mut mpsc::Receiver<UserCommand>,
) {
    while let Ok(command) = control_rx.try_recv() {
        let response = match command {
            ControlCommand::Observe { response, .. }
            | ControlCommand::Heartbeat { response, .. }
            | ControlCommand::AudioObservation { response, .. } => Some(response),
            ControlCommand::CompanionObservations { response, .. }
            | ControlCommand::ProcessCompanionMailbox { response, .. } => {
                let _ = response.send(Err(RuntimeError::Closed));
                None
            }
            ControlCommand::ReplaceCompanion { response, .. } => {
                let _ = response.send(Err(RuntimeError::Closed));
                None
            }
            ControlCommand::ReplaceConfigWhenIdle { response, .. } => {
                let _ = response.send(Err(RuntimeError::Closed));
                None
            }
            ControlCommand::ConsolidateMemory { response, .. } => {
                let _ = response.send(Err(RuntimeError::Closed));
                None
            }
        };
        if let Some(response) = response {
            let _ = response.send(Err(RuntimeError::Closed));
        }
    }
    while let Ok(command) = priority_rx.try_recv() {
        match command {
            PriorityCommand::UpdateConfig { response, .. }
            | PriorityCommand::UpdateWatchEnabled { response, .. }
            | PriorityCommand::ReplaceConfig { response, .. }
            | PriorityCommand::EnterDegraded { response, .. } => {
                let _ = response.send(Err(RuntimeError::Closed));
            }
            PriorityCommand::Quiesce { response, .. } => {
                let _ = response.send(Err(RuntimeError::Closed));
            }
            PriorityCommand::CancelUser { response } => {
                let _ = response.send(Err(RuntimeError::Closed));
            }
            PriorityCommand::RetryUser { response } => {
                let _ = response.send(Err(RuntimeError::Closed));
            }
        }
    }
    while let Ok(command) = user_rx.try_recv() {
        let UserCommand::Enqueue(command) = command;
        if let Some(response) = command.response {
            let _ = response.send(Err(RuntimeError::Closed));
        }
    }
}

impl RuntimeActor {
    pub(super) fn accept_user_command(
        &mut self,
        command: UserCommand,
        _volatile_users: &mut std::collections::VecDeque<
            crate::companion_storage::PendingUserMessage,
        >,
        running_coo: &mut Option<RunningOperation>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let UserCommand::Enqueue(command) = command;
        let UserQueueCommand { input_id, response } = *command;
        if self.user_commands_blocked {
            if let Some(response) = response {
                let _ = response.send(Err(RuntimeError::Closed));
            }
            return;
        }
        let persistent_queue = self.user_queue_is_persistent();
        if let Some(response) = response {
            self.user_waiters.insert(input_id.clone(), response);
        }
        if let Some(companion) = self.companion.as_ref() {
            if companion.owns_user_queue() && !persistent_queue {
                self.respond_user_waiters(&[input_id], Err(RuntimeError::CompanionUnavailable));
                return;
            }
            if let Ok(Some(completed)) = companion.completed_user_response(&input_id) {
                self.respond_user_waiters(&[input_id], Ok(completed));
                self.revision = self.revision.saturating_add(1);
                self.publish(snapshot_tx);
                return;
            }
            if persistent_queue {
                let preparer = companion
                    .user_message_preparer_with_runtime_queue(self.runtime_user_queue.clone());
                match preparer.pending_message(&input_id) {
                    Ok(Some(input)) if input.user_seq == 0 => {
                        match preparer.take_runtime_input(&input_id) {
                            Ok(Some(input)) => _volatile_users.push_back(input),
                            Ok(None) => {
                                self.respond_user_waiters(
                                    &[input_id],
                                    Err(RuntimeError::CompanionUnavailable),
                                );
                                return;
                            }
                            Err(error) => {
                                self.respond_user_waiters(
                                    &[input_id],
                                    Err(RuntimeError::from(error)),
                                );
                                return;
                            }
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        self.respond_user_waiters(
                            &[input_id],
                            Err(RuntimeError::CompanionUnavailable),
                        );
                        return;
                    }
                    Err(error) => {
                        self.respond_user_waiters(&[input_id], Err(RuntimeError::from(error)));
                        return;
                    }
                }
            } else {
                match companion
                    .user_message_preparer_with_runtime_queue(self.runtime_user_queue.clone())
                    .take_runtime_input(&input_id)
                {
                    Ok(Some(input)) => _volatile_users.push_back(input),
                    Ok(None) => {
                        self.respond_user_waiters(
                            &[input_id],
                            Err(RuntimeError::CompanionUnavailable),
                        );
                        return;
                    }
                    Err(error) => {
                        self.respond_user_waiters(&[input_id], Err(RuntimeError::from(error)));
                        return;
                    }
                }
            }
        }
        self.user_work_pending = true;
        if let Some(operation) = running_coo.as_mut() {
            if operation.preempt_for_user() {
                self.companion_recovery_pending = true;
            } else if operation.is_user() {
                match self.append_pending_user_inputs(operation) {
                    Ok(
                        AppendPendingResult::NotUser
                        | AppendPendingResult::NoNewInput
                        | AppendPendingResult::Appended
                        | AppendPendingResult::RestartRequested,
                    ) => {}
                    Err(error) => {
                        self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx)
                    }
                }
            }
        }
        self.revision = self.revision.saturating_add(1);
        self.publish(snapshot_tx);
    }

    pub(super) fn user_queue_is_persistent(&self) -> bool {
        self.user_preparer
            .read()
            .map(|preparer| {
                preparer
                    .as_ref()
                    .is_some_and(|value| value.uses_persistent_queue())
            })
            .unwrap_or(false)
    }

    pub(super) fn close_user_waiters(&mut self) {
        for (_, response) in self.user_waiters.drain() {
            let _ = response.send(Err(RuntimeError::Closed));
        }
    }

    pub(super) fn defer_companion_observation_wake(
        &mut self,
        observations: Vec<ObservationRecord>,
        response: oneshot::Sender<Result<CompanionResponse, RuntimeError>>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let mut durable_observations = self.pending_observations.clone();
        durable_observations.extend(observations);
        durable_observations.sort_by(|left, right| {
            left.created_at()
                .cmp(right.created_at())
                .then_with(|| left.id().cmp(right.id()))
        });
        durable_observations.dedup_by(|left, right| left.id() == right.id());
        let Some(companion) = self.companion.as_mut() else {
            let _ = response.send(Err(RuntimeError::CompanionUnavailable));
            return;
        };
        if let Err(error) = companion.queue_observations_for_user(&durable_observations) {
            self.schedule_initialization_retry(initialization_error_kind(&error), snapshot_tx);
            let _ = response.send(Err(RuntimeError::from(error)));
            return;
        }
        for observation in durable_observations {
            if !self
                .pending_observations
                .iter()
                .any(|pending| pending.id() == observation.id())
            {
                self.pending_observations.push(observation);
            }
        }
        let _ = response.send(Ok(crate::companion::silent_response()));
        self.revision = self.revision.saturating_add(1);
        self.publish(snapshot_tx);
    }

    pub(super) fn respond_user_waiters(
        &mut self,
        input_ids: &[String],
        result: Result<CompanionResponse, RuntimeError>,
    ) {
        match result {
            Ok(response) => {
                for input_id in input_ids {
                    if let Some(waiter) = self.user_waiters.remove(input_id) {
                        let _ = waiter.send(Ok(response.clone()));
                    }
                }
            }
            Err(error) => {
                let message = error.to_string();
                let mut first = true;
                let mut error = Some(error);
                for input_id in input_ids {
                    let Some(waiter) = self.user_waiters.remove(input_id) else {
                        continue;
                    };
                    let error = if first {
                        first = false;
                        error.take().expect("first waiter error")
                    } else {
                        RuntimeError::Factory(message.clone())
                    };
                    let _ = waiter.send(Err(error));
                }
            }
        }
    }
}

/*
 * The observer is a separate provider lane. Audio observations use this lane
 * so they wait for a running observer agent without consuming the Coo lane.
 */
pub(super) fn control_uses_observer(command: &ControlCommand) -> bool {
    matches!(
        command,
        ControlCommand::Observe { .. }
            | ControlCommand::Heartbeat { .. }
            | ControlCommand::AudioObservation { .. }
    )
}

impl RuntimeActor {
    pub(super) fn update_watch_enabled(
        &mut self,
        enabled: bool,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
        config_tx: &watch::Sender<Config>,
    ) -> u64 {
        if self.config.watch.enabled != enabled {
            self.config.watch.enabled = enabled;
            self.revision = self.revision.saturating_add(1);
            let _ = config_tx.send(self.config.clone());
            self.publish(snapshot_tx);
        }
        self.revision
    }

    pub(super) fn append_pending_user_inputs(
        &mut self,
        operation: &mut RunningOperation,
    ) -> Result<AppendPendingResult, CompanionError> {
        if self.config.chat.while_thinking != "append" {
            return Ok(AppendPendingResult::NotUser);
        }
        let preparer = self
            .user_preparer
            .read()
            .map(|preparer| preparer.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        let Some(preparer) = preparer else {
            return Ok(AppendPendingResult::NotUser);
        };
        let pending = preparer.pending_messages()?;
        operation.append_pending_inputs(pending, &preparer)
    }

    pub(super) fn cancel_active_user(&mut self) -> Result<(), RuntimeError> {
        let input_id = self
            .active_user_message_id
            .clone()
            .or_else(|| self.terminal_attachment_input_id())
            .ok_or_else(|| RuntimeError::Factory("取り消せる返事はありません".to_owned()))?;
        self.cancel_user_input(&input_id)
    }

    pub(super) fn cancel_user_input(&mut self, input_id: &str) -> Result<(), RuntimeError> {
        let runtime_has_observations = !self.pending_observations.is_empty();
        let resume_proactive = if let Some(companion) = self.companion.as_mut() {
            companion.cancel_user_message(input_id)?;
            let has_observations =
                companion.has_pending_proactive_observations() || runtime_has_observations;
            has_observations && companion.proactive_recovery_can_start(&self.pending_observations)
        } else {
            let preparer = self
                .user_preparer
                .read()
                .map_err(|_| RuntimeError::CompanionUnavailable)?
                .clone()
                .ok_or(RuntimeError::CompanionUnavailable)?;
            preparer.cancel(input_id)?;
            false
        };
        self.operation_cancellation.cancel_lane(OperationLane::Coo);
        self.operation_cancellation.renew_lane(OperationLane::Coo);
        if self.last_error.as_ref().is_some_and(|error| {
            error
                .attachment_ocr
                .as_ref()
                .is_some_and(|failure| failure.input_id == input_id)
        }) {
            self.last_error = None;
            self.user_retry_at = None;
            self.user_retry_delay = Duration::from_secs(1);
        }
        self.active_user_message_id = None;
        if resume_proactive {
            self.companion_recovery_pending = true;
            self.companion_recovery_at = None;
        }
        if !self
            .cancelled_user_message_ids
            .iter()
            .any(|value| value == input_id)
        {
            self.cancelled_user_message_ids.push(input_id.to_owned());
            if self.cancelled_user_message_ids.len() > 200 {
                self.cancelled_user_message_ids.remove(0);
            }
        }
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub(super) fn cancel_user_input_after_termination(
        &mut self,
        input_id: &str,
    ) -> Result<(), RuntimeError> {
        if let Some(companion) = self.companion.as_mut() {
            companion.cancel_user_message_after_termination(input_id)?;
        } else {
            let preparer = self
                .user_preparer
                .read()
                .map_err(|_| RuntimeError::CompanionUnavailable)?
                .clone()
                .ok_or(RuntimeError::CompanionUnavailable)?;
            preparer.cancel_after_termination(input_id)?;
        }
        self.cancel_user_input(input_id)
    }

    pub(super) fn retry_user(&mut self) -> Result<String, RuntimeError> {
        let input_id = if let Some(input_id) = self.terminal_attachment_input_id() {
            let companion = self
                .companion
                .as_ref()
                .ok_or(RuntimeError::CompanionUnavailable)?;
            if !companion.clear_terminal_attachment_failure(&input_id)? {
                return Err(RuntimeError::Factory(
                    "再試行できる発言はありません".to_owned(),
                ));
            }
            input_id
        } else {
            if self.last_error.is_none() || self.user_retry_at.is_none() {
                return Err(RuntimeError::Factory(
                    "再試行できる発言はありません".to_owned(),
                ));
            }
            let preparer = self
                .user_preparer
                .read()
                .map_err(|_| RuntimeError::CompanionUnavailable)?
                .clone()
                .ok_or(RuntimeError::CompanionUnavailable)?;
            preparer
                .pending_messages()?
                .into_iter()
                .find(|input| !input.attachment_is_terminal())
                .map(|input| input.id)
                .ok_or_else(|| RuntimeError::Factory("再試行できる発言はありません".to_owned()))?
        };
        self.operation_cancellation.renew();
        self.last_error = None;
        self.user_retry_at = None;
        self.user_retry_delay = Duration::from_secs(1);
        self.active_user_message_id = None;
        self.revision = self.revision.saturating_add(1);
        Ok(input_id)
    }

    pub(super) fn terminal_attachment_input_id(&self) -> Option<String> {
        self.last_error
            .as_ref()?
            .attachment_ocr
            .as_ref()
            .filter(|failure| !failure.retryable)
            .map(|failure| failure.input_id.clone())
    }

    pub(super) fn refresh_user_preparer(&self) {
        if let Ok(mut slot) = self.user_preparer.write() {
            *slot = self.companion.as_ref().map(|companion| {
                companion.user_message_preparer_with_runtime_queue(self.runtime_user_queue.clone())
            });
        }
    }
    pub(super) fn replace_companion_config(
        &mut self,
        companion: CompanionAgent,
        config: Option<Config>,
    ) -> Result<u64, RuntimeError> {
        if let Some(config) = config {
            validate_config(&config)?;
            self.config = config;
        }
        self.companion = Some(companion);
        self.refresh_user_preparer();
        self.companion_display_name = self.companion.as_ref().map_or_else(
            || self.config.companion.display_name.clone(),
            |agent| agent.display_name().to_owned(),
        );
        self.user_commands_blocked = false;
        self.initialization_retry_delay = Duration::from_secs(1);
        self.initialization_retry_at = Some(Instant::now());
        self.revision = self.revision.saturating_add(1);
        Ok(self.revision)
    }

    pub(super) async fn update_config(&mut self, config: Config) -> Result<u64, RuntimeError> {
        validate_config(&config)?;
        if let Some(factory) = &self.factory {
            let mut agents = factory
                .build(&config)
                .await
                .map_err(RuntimeError::Factory)?;
            self.observer = agents.observer.take();
            if let Some(companion) = agents.companion.as_ref() {
                self.companion_display_name = companion.display_name().to_owned();
            }
            self.companion = agents.companion.take();
            self.refresh_user_preparer();
            self.memory = agents.memory.take();
            self.memory_run_at = self.memory.as_ref().map(|_| Instant::now());
            self.initialization_retry_delay = Duration::from_secs(1);
            self.initialization_retry_at = self.companion.as_ref().map(|_| Instant::now());
            self.user_commands_blocked = false;
        } else {
            if config.observer.provider != self.config.observer.provider
                || config.observer.model != self.config.observer.model
                || config.observer.executable != self.config.observer.executable
                || config.companion.provider != self.config.companion.provider
                || config.companion.model != self.config.companion.model
                || config.companion.executable != self.config.companion.executable
            {
                return Err(RuntimeError::Factory(
                    "provider を更新する RuntimeFactory がありません".to_owned(),
                ));
            }
            if let Some(observer) = self.observer.as_mut() {
                observer.update_config(config.observer.clone());
            }
            if let Some(companion) = self.companion.as_mut() {
                companion.update_config(config.companion.clone());
            }
        }
        self.operation_cancellation.renew();
        self.config = config;
        self.revision = self.revision.saturating_add(1);
        Ok(self.revision)
    }

    pub(super) fn enter_degraded(&mut self, error: RuntimeLastError) -> u64 {
        self.operation_cancellation.cancel_current();
        self.observer = None;
        self.companion = None;
        self.refresh_user_preparer();
        self.memory = None;
        self.initialization_retry_at = None;
        self.memory_run_at = None;
        self.last_error = Some(error);
        self.phase = RuntimePhase::Idle;
        self.revision = self.revision.saturating_add(1);
        self.revision
    }

    pub(super) fn quiesce_runtime(&mut self, clear_user_state: bool) -> u64 {
        self.operation_cancellation.cancel_current();
        self.observer = None;
        self.companion = None;
        self.refresh_user_preparer();
        self.memory = None;
        self.initialization_retry_at = None;
        self.memory_run_at = None;
        self.phase = RuntimePhase::Idle;
        self.active_user_message_id = None;
        self.companion_draft = None;
        if clear_user_state {
            self.cancelled_user_message_ids.clear();
            self.user_retry_at = None;
            self.user_retry_delay = Duration::from_secs(1);
            self.companion_recovery_pending = false;
            self.companion_recovery_at = None;
        }
        self.revision = self.revision.saturating_add(1);
        self.revision
    }

    pub(super) fn replace_config(
        &mut self,
        config: Config,
        mut agents: RuntimeAgents,
    ) -> Result<u64, RuntimeError> {
        validate_config(&config)?;
        self.observer = agents.observer.take();
        if let Some(companion) = agents.companion.as_ref() {
            self.companion_display_name = companion.display_name().to_owned();
        }
        self.companion = agents.companion.take();
        self.refresh_user_preparer();
        self.memory = agents.memory.take();
        self.memory_run_at = self.memory.as_ref().map(|_| Instant::now());
        self.config = config;
        self.user_commands_blocked = false;
        self.initialization_retry_delay = Duration::from_secs(1);
        self.initialization_retry_at = self.companion.as_ref().map(|_| Instant::now());
        self.operation_cancellation.renew();
        self.revision = self.revision.saturating_add(1);
        Ok(self.revision)
    }
}

pub(super) fn linked_cancellation(
    operation: CancellationToken,
    request: CancellationToken,
    cause: OperationCancellationCause,
) -> (CancellationToken, CancellationToken) {
    let effective = CancellationToken::new();
    if operation.is_cancelled() || request.is_cancelled() {
        cause.mark(OperationCancellationReason::Other);
        effective.cancel();
    }
    let output = effective.clone();
    let stop = CancellationToken::new();
    let relay_stop = stop.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = operation.cancelled() => {
                cause.mark(OperationCancellationReason::Other);
                output.cancel();
            }
            _ = request.cancelled() => {
                cause.mark(OperationCancellationReason::Other);
                output.cancel();
            }
            _ = relay_stop.cancelled() => {}
        }
    });
    (effective, stop)
}

pub(super) fn initialization_error_kind(error: &CompanionError) -> RuntimeErrorKind {
    match error {
        CompanionError::Cancelled
        | CompanionError::Provider(_)
        | CompanionError::Output
        | CompanionError::ObservationPrompt
        | CompanionError::LimitReached
        | CompanionError::AttachmentOcr(_) => RuntimeErrorKind::Provider,
        CompanionError::Usage(_) | CompanionError::Persistence(_) | CompanionError::Memory(_) => {
            RuntimeErrorKind::Persistence
        }
        CompanionError::Mailbox(_) => RuntimeErrorKind::Mailbox,
        CompanionError::Outbox(_) => RuntimeErrorKind::Outbox,
        CompanionError::Log(_) => RuntimeErrorKind::Logging,
        CompanionError::Json(_) => RuntimeErrorKind::Serialization,
    }
}

pub(super) fn config_update_last_error(error: &RuntimeError) -> RuntimeLastError {
    let issues = match error {
        RuntimeError::Config(crate::config::ConfigError::Validation(issues)) => issues.clone(),
        _ => Vec::new(),
    };
    RuntimeLastError {
        kind: RuntimeErrorKind::Config,
        occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: Some(format!("設定の適用に失敗しました: {error}")),
        issues,
        attachment_ocr: None,
    }
}
