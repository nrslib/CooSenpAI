use super::operation_state::{
    OperationCancellationHandle, OperationCancellationReason, OperationLane, OperationOutcome,
    OperationReply, RunningOperation, StartResult, UserAppendControl,
};
use super::*;
use crate::companion_storage::PendingUserMessage;
use crate::persistence::PersistenceError;
use futures_util::FutureExt;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::task::JoinError;
impl RuntimeActor {
    pub(super) fn start_pending_user_operation(
        &mut self,
        volatile_users: &mut std::collections::VecDeque<PendingUserMessage>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
        observer_active: bool,
    ) -> StartResult {
        let Some(mut companion) = self.companion.take() else {
            return StartResult::Completed;
        };
        if let Err(error) = companion.recover_active_turn_commit() {
            self.schedule_initialization_retry(initialization_error_kind(&error), snapshot_tx);
            self.companion = Some(companion);
            return StartResult::Completed;
        }
        let persistent_queue = companion.uses_persistent_user_queue();
        let inputs = if persistent_queue {
            while let Some(input) = volatile_users.pop_front() {
                if let Err(error) = companion.persist_prepared_user_input(input) {
                    self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx);
                    self.companion = Some(companion);
                    return StartResult::Completed;
                }
            }
            match companion.next_pending_user_messages(self.config.chat.while_thinking == "append")
            {
                Ok(inputs) => inputs,
                Err(error) => {
                    self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx);
                    self.companion = Some(companion);
                    return StartResult::Completed;
                }
            }
        } else {
            let mut inputs = Vec::new();
            if let Some(input) = volatile_users.pop_front() {
                inputs.push(input);
            }
            inputs
        };
        /*
         * The durable queue is the source of truth for owned runtimes. The
         * volatile branch is used only by the CLI's non-owning companion.
         */
        if let Err(error) = self.restore_terminal_attachment_failure(&companion) {
            self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx);
            self.companion = Some(companion);
            return StartResult::Completed;
        }
        if inputs.is_empty() {
            self.revision = self.revision.saturating_add(1);
            self.publish(snapshot_tx);
            self.companion = Some(companion);
            return StartResult::Completed;
        }
        if let Err(error) = companion.set_pending_observation_in_progress(observer_active) {
            self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx);
            self.companion = Some(companion);
            return StartResult::Completed;
        }
        let mut inputs = inputs;
        for input in &mut inputs {
            input.observation_in_progress = observer_active;
        }
        if let Err(error) = companion.queue_observations_for_user(&self.pending_observations) {
            self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx);
            self.companion = Some(companion);
            return StartResult::Completed;
        }
        self.spawn_user_operation(companion, inputs, snapshot_tx)
    }

    fn spawn_user_operation(
        &mut self,
        mut companion: CompanionAgent,
        inputs: Vec<PendingUserMessage>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> StartResult {
        let input_ids = inputs
            .iter()
            .map(|input| input.id.clone())
            .collect::<Vec<_>>();
        let Some(input_id) = input_ids.first().cloned() else {
            self.companion = Some(companion);
            return StartResult::Completed;
        };
        let Some(cancellation) = self
            .operation_cancellation
            .cancellation_for_start_lane(OperationLane::Coo)
        else {
            self.companion = Some(companion);
            return StartResult::Completed;
        };
        let operation_generation = match companion
            .user_message_preparer()
            .begin_operation(&input_ids)
        {
            Ok(generation) => generation,
            Err(error) => {
                self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx);
                self.companion = Some(companion);
                return StartResult::Completed;
            }
        };
        let append_mode = self.config.chat.while_thinking == "append";
        let dispatch_seq = companion.active_user_dispatch_seq();
        let provider = companion.provider_client();
        let (additional_tx, additional_rx) = tokio::sync::mpsc::unbounded_channel();
        let accepted_mid_turn_ids = Arc::new(Mutex::new(HashSet::new()));
        self.phase = RuntimePhase::Companion;
        self.initialization_retry_at = None;
        self.initialization_retry_delay = Duration::from_secs(1);
        self.active_user_message_id = Some(input_id.clone());
        self.companion_draft = None;
        self.revision = self.revision.saturating_add(1);
        self.publish(snapshot_tx);
        let provider_cancellation = cancellation.token.clone();
        let catch_panic_to_keep_agent = self.factory.is_none();
        let events: Arc<dyn crate::provider::ProviderEventSink> = Arc::new(RuntimeProviderEvents {
            input_id: input_id.clone(),
            sender: self.stream_tx.clone(),
            accepted_mid_turn_ids: accepted_mid_turn_ids.clone(),
        });
        let task = tokio::spawn(async move {
            let process = companion.process_user_messages_streaming(
                inputs,
                provider_cancellation,
                Some(events),
                append_mode.then_some(additional_rx),
                accepted_mid_turn_ids,
                operation_generation,
            );
            let result = if catch_panic_to_keep_agent {
                std::panic::AssertUnwindSafe(process)
                    .catch_unwind()
                    .await
                    .map_or_else(
                        |_| Err(RuntimeError::Closed),
                        |result| result.map_err(RuntimeError::from),
                    )
            } else {
                process.await.map_err(RuntimeError::from)
            };
            Box::new(OperationOutcome::User {
                companion: Box::new(companion),
                active_input_id: input_id,
                operation_generation,
                result,
            })
        });
        let user_append = append_mode.then_some(UserAppendControl {
            tracked_input_ids: input_ids.iter().cloned().collect(),
            operation_input_ids: input_ids.clone(),
            operation_generation,
            dispatch_seq,
            sender: additional_tx,
            provider,
            restart_requested: false,
        });
        StartResult::Running(Box::new(RunningOperation::user(
            cancellation,
            input_ids.clone(),
            OperationReply::User(input_ids),
            task,
            user_append,
        )))
    }

    pub(super) fn start_initialization_operation(
        &mut self,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> StartResult {
        if self.agent_rebuild_pending
            || (self.observer.is_none() && self.companion.is_none() && self.memory.is_none())
        {
            let Some(factory) = self.factory.clone() else {
                self.initialization_retry_at = None;
                return StartResult::Completed;
            };
            let Some(cancellation) = self.operation_cancellation.cancellation_for_start() else {
                return StartResult::Completed;
            };
            let config = self.config.clone();
            let task = tokio::spawn(async move {
                Box::new(OperationOutcome::BuildAgents(
                    factory.build(&config).await.map(Box::new),
                ))
            });
            return StartResult::Running(Box::new(RunningOperation::new(
                cancellation,
                OperationReply::None,
                task,
            )));
        }
        let Some(cancellation) = self.operation_cancellation.cancellation_for_start() else {
            return StartResult::Completed;
        };
        let Some(mut companion) = self.companion.take() else {
            self.initialization_retry_at = None;
            if self.last_error.take().is_some() {
                self.revision = self.revision.saturating_add(1);
                self.publish(snapshot_tx);
            }
            return StartResult::Completed;
        };
        let delivery_status_before = companion.pending_delivery_status();
        let provider_cancellation = cancellation.token.clone();
        let catch_panic_to_keep_agent = self.factory.is_none();
        let task = tokio::spawn(async move {
            let initialize = companion.initialize(provider_cancellation);
            let result = if catch_panic_to_keep_agent {
                std::panic::AssertUnwindSafe(initialize)
                    .catch_unwind()
                    .await
                    .map_or_else(
                        |_| {
                            Err(CompanionError::Persistence(PersistenceError::Invalid(
                                "companion 初期化が panic しました".to_owned(),
                            )))
                        },
                        |result| result.map(|_| ()),
                    )
            } else {
                initialize.await.map(|_| ())
            };
            Box::new(OperationOutcome::Initialize {
                companion: Box::new(companion),
                delivery_status_before,
                result,
            })
        });
        StartResult::Running(Box::new(RunningOperation::new(
            cancellation,
            OperationReply::None,
            task,
        )))
    }

    pub(super) fn start_companion_recovery_operation(
        &mut self,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> StartResult {
        let runtime_observations = self.pending_observations.clone();
        let Some(cancellation) = self.operation_cancellation.cancellation_for_start() else {
            return StartResult::Completed;
        };
        let Some(mut companion) = self.companion.take() else {
            return StartResult::Completed;
        };
        if let Err(error) = companion.recover_active_turn_commit() {
            self.schedule_initialization_retry(initialization_error_kind(&error), snapshot_tx);
            self.companion = Some(companion);
            return StartResult::Completed;
        }
        let observations = match companion.pending_proactive_observations() {
            Ok(mut observations) => {
                observations.extend(runtime_observations);
                let mut seen = std::collections::HashSet::new();
                observations.retain(|observation| seen.insert(observation.id().to_owned()));
                observations
            }
            Err(error) => {
                self.companion = Some(companion);
                self.schedule_initialization_retry(initialization_error_kind(&error), snapshot_tx);
                return StartResult::Completed;
            }
        };
        let user_epoch = match companion.user_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                self.companion = Some(companion);
                self.schedule_user_retry(initialization_error_kind(&error), snapshot_tx);
                return StartResult::Completed;
            }
        };
        let provider_cancellation = cancellation.token.clone();
        let catch_panic_to_keep_agent = self.factory.is_none();
        self.phase = RuntimePhase::Companion;
        self.publish(snapshot_tx);
        let task = tokio::spawn(async move {
            let process =
                companion.process_observations_candidate(observations, None, provider_cancellation);
            let result = if catch_panic_to_keep_agent {
                std::panic::AssertUnwindSafe(process)
                    .catch_unwind()
                    .await
                    .map_or_else(
                        |_| Err(RuntimeError::Closed),
                        |result| result.map_err(RuntimeError::from),
                    )
            } else {
                process.await.map_err(RuntimeError::from)
            };
            Box::new(OperationOutcome::CompanionObservations {
                companion: Box::new(companion),
                user_epoch,
                result,
            })
        });
        StartResult::Running(Box::new(RunningOperation::proactive(
            cancellation,
            None,
            OperationReply::None,
            task,
        )))
    }

    pub(super) fn start_memory_operation(&mut self) -> StartResult {
        let Some(cancellation) = self.operation_cancellation.cancellation_for_start() else {
            return StartResult::Completed;
        };
        let Some(mut memory) = self.memory.take() else {
            self.memory_run_at = None;
            return StartResult::Completed;
        };
        let provider_cancellation = cancellation.token.clone();
        let catch_panic_to_keep_agent = self.factory.is_none();
        let task = tokio::spawn(async move {
            let run_due = memory.run_due(provider_cancellation);
            if catch_panic_to_keep_agent {
                let _ = std::panic::AssertUnwindSafe(run_due).catch_unwind().await;
            } else {
                let _ = run_due.await;
            }
            Box::new(OperationOutcome::Memory(Box::new(memory)))
        });
        StartResult::Running(Box::new(RunningOperation::new(
            cancellation,
            OperationReply::None,
            task,
        )))
    }

    pub(super) fn finish_operation(
        &mut self,
        operation: RunningOperation,
        result: Result<Box<OperationOutcome>, JoinError>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> PendingUserDrain {
        let append_restart_requested = operation.append_restart_requested();
        let preempted_for_user = operation.was_preempted_for_user();
        let operation_cancelled = operation.cancellation.is_cancelled();
        let observer_cancelled = operation.is_observer() && operation_cancelled;
        let coo_cancelled = !operation.is_observer() && operation_cancelled;
        let config_update_cancelled = operation_cancelled
            && operation.cancellation_reason() == Some(OperationCancellationReason::ConfigUpdate);
        let user_input_ids = operation.user_input_ids().to_vec();
        let reply = operation.reply;
        if let Some(stop) = operation.relay_stop {
            stop.cancel();
        }
        let pending_user_drain = match result {
            Ok(outcome) => self.apply_operation_outcome(
                *outcome,
                reply,
                append_restart_requested,
                preempted_for_user,
                config_update_cancelled,
                snapshot_tx,
            ),
            Err(_) => {
                self.agent_rebuild_pending = true;
                if user_input_ids.is_empty() {
                    reply.send_closed();
                } else {
                    self.respond_user_waiters(&user_input_ids, Err(RuntimeError::Closed));
                }
                self.last_error = Some(RuntimeLastError {
                    kind: RuntimeErrorKind::Provider,
                    occurred_at: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    message: Some("provider 操作が異常終了しました".to_owned()),
                    issues: Vec::new(),
                    attachment_ocr: None,
                });
                if !user_input_ids.is_empty() {
                    self.schedule_user_retry(RuntimeErrorKind::Provider, snapshot_tx);
                }
                self.schedule_initialization_retry(RuntimeErrorKind::Provider, snapshot_tx);
                PendingUserDrain::Unchanged
            }
        };
        if coo_cancelled {
            self.operation_cancellation.renew_lane(OperationLane::Coo);
        }
        if observer_cancelled {
            self.operation_cancellation
                .renew_lane(OperationLane::Observer);
        }
        self.revision = self.revision.saturating_add(1);
        self.publish(snapshot_tx);
        pending_user_drain
    }

    pub(super) fn restore_cancelled_operation(
        &mut self,
        outcome: Box<OperationOutcome>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        match *outcome {
            OperationOutcome::Observe { observer, .. } => {
                self.observer = Some(*observer);
            }
            OperationOutcome::CompanionObservations {
                companion, result, ..
            } => {
                let mut companion = *companion;
                if let Ok(candidate) = result {
                    if let Err(error) = companion.discard_proactive_candidate(
                        &candidate.observations,
                        &candidate.consumed_observations,
                    ) {
                        self.schedule_initialization_retry(
                            initialization_error_kind(&error),
                            snapshot_tx,
                        );
                    }
                } else {
                    companion.discard_provider_session();
                }
                self.companion_recovery_pending = companion.has_pending_proactive_observations();
                self.companion_recovery_at = None;
                self.companion = Some(companion);
            }
            OperationOutcome::CompanionMailbox { companion, .. } => {
                let mut companion = *companion;
                companion.discard_provider_session();
                self.companion = Some(companion);
            }
            OperationOutcome::User { companion, .. } => {
                let mut companion = *companion;
                companion.discard_provider_session();
                self.companion = Some(companion);
                self.active_user_message_id = None;
                self.companion_draft = None;
                self.user_work_pending = true;
            }
            OperationOutcome::Initialize { companion, .. } => {
                self.companion = Some(*companion);
            }
            OperationOutcome::BuildAgents(_) => {}
            OperationOutcome::Memory(memory) => {
                self.memory = Some(*memory);
            }
            OperationOutcome::Consolidate { memory, .. } => {
                self.memory = Some(*memory);
            }
        }
    }

    pub(super) fn start_observe(
        &mut self,
        frames: Vec<ObservationFrameInput>,
        request_cancellation: CancellationToken,
        response: oneshot::Sender<Result<ObservationRecord, RuntimeError>>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> StartResult {
        if !self.accepts_watch_scope(&frames) {
            let _ = response.send(Err(RuntimeError::StaleWatchScope));
            return StartResult::Completed;
        }
        let Some(runtime_cancellation) = self
            .operation_cancellation
            .cancellation_for_start_lane(OperationLane::Observer)
        else {
            let _ = response.send(Err(RuntimeError::ProviderStartsBlocked));
            return StartResult::Completed;
        };
        self.phase = RuntimePhase::Observing;
        self.publish(snapshot_tx);
        let Some(mut observer) = self.observer.take() else {
            let _ = response.send(Err(RuntimeError::ObserverUnavailable));
            self.phase = RuntimePhase::Idle;
            self.revision = self.revision.saturating_add(1);
            self.publish(snapshot_tx);
            return StartResult::Completed;
        };
        let (cancellation, relay_stop) = linked_cancellation(
            runtime_cancellation.token.clone(),
            request_cancellation,
            runtime_cancellation.cause.clone(),
        );
        let provider_cancellation = cancellation.clone();
        let scope_generation = self.watch_scope_generation.clone();
        let scope_commit_lock = self.watch_scope_commit_lock.clone();
        let expected_generation = frames.first().map_or_else(
            || {
                self.watch_scope_generation
                    .load(std::sync::atomic::Ordering::Acquire)
            },
            |frame| frame.scope_generation,
        );
        let catch_panic_to_keep_agent = self.factory.is_none();
        let task = tokio::spawn(async move {
            let observe = observer.observe_scoped(
                frames,
                provider_cancellation,
                expected_generation,
                scope_generation,
                scope_commit_lock,
            );
            let result = if catch_panic_to_keep_agent {
                std::panic::AssertUnwindSafe(observe)
                    .catch_unwind()
                    .await
                    .map_or_else(
                        |_| Err(RuntimeError::Closed),
                        |result| match result {
                            Ok(observation) => Ok(ObservationRecord::Visual(observation)),
                            Err(ObserverError::OutboxPending { record }) => Ok(*record),
                            Err(ObserverError::StaleScope) => Err(RuntimeError::StaleWatchScope),
                            Err(error) => Err(RuntimeError::from(error)),
                        },
                    )
            } else {
                observe.await.map_or_else(
                    |error| match error {
                        ObserverError::OutboxPending { record } => Ok(*record),
                        ObserverError::StaleScope => Err(RuntimeError::StaleWatchScope),
                        error => Err(RuntimeError::from(error)),
                    },
                    |observation| Ok(ObservationRecord::Visual(observation)),
                )
            };
            Box::new(OperationOutcome::Observe {
                observer: Box::new(observer),
                result,
            })
        });
        StartResult::Running(Box::new(RunningOperation::linked(
            cancellation,
            runtime_cancellation.cause,
            relay_stop,
            OperationReply::Observation(response),
            task,
        )))
    }

    pub(super) fn process_heartbeat(
        &mut self,
        stagnation: Option<crate::state::StagnationObservation>,
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<ObservationRecord, RuntimeError>>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let result = if cancellation.is_cancelled() {
            Err(RuntimeError::Closed)
        } else {
            match self.observer.as_mut() {
                Some(observer) => match observer.no_change_with_stagnation(stagnation) {
                    Ok(observation) => Ok(observation),
                    Err(ObserverError::OutboxPending { record }) => Ok(*record),
                    Err(error) => Err(RuntimeError::from(error)),
                },
                None => Err(RuntimeError::ObserverUnavailable),
            }
        };
        if let Ok(observation) = &result {
            self.pending_observations.push(observation.clone());
        }
        let _ = response.send(result);
        self.revision = self.revision.saturating_add(1);
        self.publish(snapshot_tx);
    }

    pub(super) fn process_audio_observation(
        &mut self,
        source: crate::state::AudioObservationSource,
        text: String,
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<ObservationRecord, RuntimeError>>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) {
        let result = if cancellation.is_cancelled() {
            Err(RuntimeError::Closed)
        } else {
            match self.observer.as_mut() {
                Some(observer) => match observer.audio_observation(source, &text) {
                    Ok(observation) => Ok(ObservationRecord::Audio(observation)),
                    Err(ObserverError::OutboxPending { record }) => Ok(*record),
                    Err(error) => Err(RuntimeError::from(error)),
                },
                None => Err(RuntimeError::ObserverUnavailable),
            }
        };
        if let Ok(observation) = &result {
            self.pending_observations.push(observation.clone());
        }
        let _ = response.send(result);
        self.revision = self.revision.saturating_add(1);
        self.publish(snapshot_tx);
    }

    pub(super) fn start_companion_observations(
        &mut self,
        observations: Vec<ObservationRecord>,
        context_notice: Option<String>,
        request_cancellation: CancellationToken,
        response: oneshot::Sender<Result<CompanionResponse, RuntimeError>>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> StartResult {
        let Some(runtime_cancellation) = self.operation_cancellation.cancellation_for_start()
        else {
            let _ = response.send(Err(RuntimeError::ProviderStartsBlocked));
            return StartResult::Completed;
        };
        self.phase = RuntimePhase::Companion;
        self.publish(snapshot_tx);
        let Some(mut companion) = self.companion.take() else {
            let _ = response.send(Err(RuntimeError::CompanionUnavailable));
            self.phase = RuntimePhase::Idle;
            self.revision = self.revision.saturating_add(1);
            self.publish(snapshot_tx);
            return StartResult::Completed;
        };
        let user_epoch = match companion.user_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                self.companion = Some(companion);
                let _ = response.send(Err(RuntimeError::Companion(error)));
                return StartResult::Completed;
            }
        };
        let (cancellation, relay_stop) = linked_cancellation(
            runtime_cancellation.token.clone(),
            request_cancellation,
            runtime_cancellation.cause.clone(),
        );
        let provider_cancellation = cancellation.clone();
        let catch_panic_to_keep_agent = self.factory.is_none();
        let task = tokio::spawn(async move {
            let process = companion.process_observations_candidate(
                observations,
                context_notice,
                provider_cancellation,
            );
            let result = if catch_panic_to_keep_agent {
                std::panic::AssertUnwindSafe(process)
                    .catch_unwind()
                    .await
                    .map_or_else(
                        |_| Err(RuntimeError::Closed),
                        |result| result.map_err(RuntimeError::from),
                    )
            } else {
                process.await.map_err(RuntimeError::from)
            };
            Box::new(OperationOutcome::CompanionObservations {
                companion: Box::new(companion),
                user_epoch,
                result,
            })
        });
        StartResult::Running(Box::new(RunningOperation::proactive(
            OperationCancellationHandle {
                token: cancellation,
                cause: runtime_cancellation.cause,
            },
            Some(relay_stop),
            OperationReply::Companion(response),
            task,
        )))
    }

    pub(super) fn start_companion_mailbox(
        &mut self,
        request_cancellation: CancellationToken,
        response: oneshot::Sender<Result<CompanionResponse, RuntimeError>>,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> StartResult {
        let Some(runtime_cancellation) = self.operation_cancellation.cancellation_for_start()
        else {
            let _ = response.send(Err(RuntimeError::ProviderStartsBlocked));
            return StartResult::Completed;
        };
        self.phase = RuntimePhase::Companion;
        self.publish(snapshot_tx);
        if let Some(observer) = self.observer.as_mut() {
            observer.retry_pending_outbox();
        }
        let Some(mut companion) = self.companion.take() else {
            let _ = response.send(Err(RuntimeError::CompanionUnavailable));
            self.phase = RuntimePhase::Idle;
            self.revision = self.revision.saturating_add(1);
            self.publish(snapshot_tx);
            return StartResult::Completed;
        };
        let (cancellation, relay_stop) = linked_cancellation(
            runtime_cancellation.token.clone(),
            request_cancellation,
            runtime_cancellation.cause.clone(),
        );
        let provider_cancellation = cancellation.clone();
        let catch_panic_to_keep_agent = self.factory.is_none();
        let task = tokio::spawn(async move {
            let process = companion.process_incoming_mailbox(provider_cancellation);
            let result = if catch_panic_to_keep_agent {
                std::panic::AssertUnwindSafe(process)
                    .catch_unwind()
                    .await
                    .map_or_else(
                        |_| Err(RuntimeError::Closed),
                        |result| result.map_err(RuntimeError::from),
                    )
            } else {
                process.await.map_err(RuntimeError::from)
            };
            Box::new(OperationOutcome::CompanionMailbox {
                companion: Box::new(companion),
                result,
            })
        });
        StartResult::Running(Box::new(RunningOperation::proactive(
            OperationCancellationHandle {
                token: cancellation,
                cause: runtime_cancellation.cause,
            },
            Some(relay_stop),
            OperationReply::Companion(response),
            task,
        )))
    }

    pub(super) fn start_consolidate(
        &mut self,
        period: String,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    ) -> StartResult {
        let Some(cancellation) = self.operation_cancellation.cancellation_for_start() else {
            let _ = response.send(Err(RuntimeError::ProviderStartsBlocked));
            return StartResult::Completed;
        };
        let Some(mut memory) = self.memory.take() else {
            let _ = response.send(Err(RuntimeError::Factory(
                "記憶メンテナンスは常駐 runtime でのみ利用できます".to_owned(),
            )));
            return StartResult::Completed;
        };
        let provider_cancellation = cancellation.token.clone();
        let catch_panic_to_keep_agent = self.factory.is_none();
        let task = tokio::spawn(async move {
            let consolidate = memory.consolidate(&period, provider_cancellation);
            let result = if catch_panic_to_keep_agent {
                std::panic::AssertUnwindSafe(consolidate)
                    .catch_unwind()
                    .await
                    .map_or_else(
                        |_| Err("memory consolidation が panic しました".to_owned()),
                        |result| result.map(|_| ()).map_err(|error| error.to_string()),
                    )
            } else {
                consolidate
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            };
            Box::new(OperationOutcome::Consolidate {
                memory: Box::new(memory),
                result,
            })
        });
        StartResult::Running(Box::new(RunningOperation::new(
            cancellation,
            OperationReply::Revision(response),
            task,
        )))
    }

    fn apply_operation_outcome(
        &mut self,
        outcome: OperationOutcome,
        reply: OperationReply,
        append_restart_requested: bool,
        preempted_for_user: bool,
        config_update_cancelled: bool,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
    ) -> PendingUserDrain {
        let mut pending_user_drain = PendingUserDrain::Unchanged;
        match outcome {
            OperationOutcome::Observe { observer, result } => {
                self.observer = Some(*observer);
                let result = if config_update_cancelled {
                    Err(RuntimeError::ConfigUpdateCancelled)
                } else {
                    match result {
                        Ok(observation) => {
                            self.pending_observations.push(observation.clone());
                            if let Some(companion) = self.companion.as_mut() {
                                if let Err(error) = companion
                                    .queue_observations_for_user(std::slice::from_ref(&observation))
                                {
                                    self.schedule_initialization_retry(
                                        initialization_error_kind(&error),
                                        snapshot_tx,
                                    );
                                    Err(RuntimeError::from(error))
                                } else {
                                    Ok(observation)
                                }
                            } else {
                                Ok(observation)
                            }
                        }
                        Err(error) => Err(error),
                    }
                };
                self.clear_observation_in_progress();
                if let OperationReply::Observation(response) = reply {
                    let _ = response.send(result);
                }
            }
            OperationOutcome::CompanionObservations {
                companion,
                user_epoch,
                result,
            } => {
                let mut companion = *companion;
                let response_result = if config_update_cancelled {
                    if let Ok(candidate) = result {
                        if let Err(error) = companion.discard_proactive_candidate(
                            &candidate.observations,
                            &candidate.consumed_observations,
                        ) {
                            self.schedule_initialization_retry(
                                initialization_error_kind(&error),
                                snapshot_tx,
                            );
                        }
                    } else {
                        companion.discard_provider_session();
                    }
                    self.companion_recovery_pending =
                        companion.has_pending_proactive_observations();
                    self.companion_recovery_at = None;
                    Err(RuntimeError::ConfigUpdateCancelled)
                } else if preempted_for_user {
                    if let Ok(candidate) = result {
                        let _ = companion.discard_proactive_candidate(
                            &candidate.observations,
                            &candidate.consumed_observations,
                        );
                    } else {
                        companion.discard_provider_session();
                    }
                    self.companion_recovery_pending = true;
                    self.companion_recovery_at = None;
                    Ok(crate::companion::silent_response())
                } else {
                    match result {
                        Ok(candidate) => {
                            let stale =
                                match (companion.user_epoch(), companion.has_pending_user_inputs())
                                {
                                    (Ok(epoch), Ok(has_pending)) => {
                                        epoch != user_epoch || has_pending
                                    }
                                    _ => true,
                                };
                            if stale {
                                let _ = companion.discard_proactive_candidate(
                                    &candidate.observations,
                                    &candidate.consumed_observations,
                                );
                                self.companion_recovery_pending = true;
                                self.companion_recovery_at = None;
                                Ok(crate::companion::silent_response())
                            } else {
                                let candidate_observations = candidate.observations.clone();
                                let candidate_consumed_observations =
                                    candidate.consumed_observations.clone();
                                let turn_commit_lock = self.turn_commit_lock.clone();
                                let _turn_commit_guard = turn_commit_lock
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                match companion
                                    .commit_proactive_candidate_if_current(candidate, user_epoch)
                                {
                                    Ok(Some((response, consumed_ids))) => {
                                        self.accept_companion_thought(&response);
                                        self.pending_observations.retain(|observation| {
                                            !consumed_ids.iter().any(|id| id == observation.id())
                                        });
                                        self.clear_non_attachment_error();
                                        self.initialization_retry_at = None;
                                        self.initialization_retry_delay = Duration::from_secs(1);
                                        self.companion_recovery_at = companion
                                            .proactive_retry_after()
                                            .map(|delay| Instant::now() + delay);
                                        Ok(response)
                                    }
                                    Ok(None) => {
                                        self.companion_recovery_pending = true;
                                        self.companion_recovery_at = None;
                                        let _ = companion.discard_proactive_candidate(
                                            &candidate_observations,
                                            &candidate_consumed_observations,
                                        );
                                        Ok(crate::companion::silent_response())
                                    }
                                    Err(error) => {
                                        if companion.has_active_turn_commit().unwrap_or(false) {
                                            self.companion_recovery_pending = true;
                                            self.companion_recovery_at = None;
                                        }
                                        if let Err(restore_error) = companion
                                            .discard_proactive_candidate(
                                                &candidate_observations,
                                                &candidate_consumed_observations,
                                            )
                                        {
                                            self.schedule_initialization_retry(
                                                initialization_error_kind(&restore_error),
                                                snapshot_tx,
                                            );
                                        }
                                        Err(RuntimeError::Companion(error))
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            companion.discard_provider_session();
                            if let RuntimeError::Companion(companion_error) = &error {
                                self.schedule_initialization_retry(
                                    initialization_error_kind(companion_error),
                                    snapshot_tx,
                                );
                            }
                            Err(error)
                        }
                    }
                };
                self.companion = Some(companion);
                if let OperationReply::Companion(response) = reply {
                    let _ = response.send(response_result);
                }
            }
            OperationOutcome::CompanionMailbox {
                companion,
                mut result,
            } => {
                let mut companion = *companion;
                if config_update_cancelled {
                    companion.discard_provider_session();
                    result = Err(RuntimeError::ConfigUpdateCancelled);
                } else if preempted_for_user {
                    companion.discard_provider_session();
                }
                let proactive_retry_after = if config_update_cancelled {
                    None
                } else {
                    companion.proactive_retry_after()
                };
                self.companion = Some(companion);
                if preempted_for_user && !config_update_cancelled {
                    self.companion_recovery_pending = true;
                    self.companion_recovery_at = None;
                    result = Ok(crate::companion::silent_response());
                }
                if result.is_ok() && !preempted_for_user {
                    if let Ok(response) = &result {
                        self.accept_companion_thought(response);
                    }
                    self.pending_observations.clear();
                    self.companion_recovery_at =
                        proactive_retry_after.map(|delay| Instant::now() + delay);
                }
                if let OperationReply::Companion(response) = reply {
                    let _ = response.send(result);
                }
            }
            OperationOutcome::User {
                companion,
                active_input_id: input_id,
                operation_generation,
                result,
            } => {
                let mut companion = *companion;
                let mut terminal_attachment_failure = false;
                let waiter_ids = match &reply {
                    OperationReply::User(ids) => ids.clone(),
                    _ => Vec::new(),
                };
                let mut response_ids = waiter_ids.clone();
                let mut retry_without_response = false;
                let response_result;
                let cancelled = self
                    .cancelled_user_message_ids
                    .iter()
                    .any(|cancelled| cancelled == &input_id);
                if cancelled {
                    let _ = companion.cancel_user_message(&input_id);
                    companion.discard_provider_session();
                    response_result = Err(RuntimeError::Closed);
                    self.operation_cancellation.renew_lane(OperationLane::Coo);
                } else if append_restart_requested {
                    companion.discard_provider_session();
                    response_result = Err(RuntimeError::Closed);
                    retry_without_response = true;
                    self.operation_cancellation.renew_lane(OperationLane::Coo);
                } else {
                    let mut committed_result = match result {
                        Ok(completed) => {
                            response_ids = completed.input_ids.clone();
                            let turn_commit_lock = self.turn_commit_lock.clone();
                            let _turn_commit_guard = turn_commit_lock
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            match companion.commit_user_candidate(&completed, operation_generation)
                            {
                                Ok(consumed_ids) => {
                                    let finalization = if companion.uses_persistent_user_queue() {
                                        companion
                                            .normalize_pending_user_messages(&completed.input_ids)
                                    } else {
                                        companion.finalize_user_responses(&completed.input_ids)
                                    };
                                    match finalization {
                                        Ok(()) => {
                                            self.pending_observations.retain(|observation| {
                                                !consumed_ids
                                                    .iter()
                                                    .any(|id| id == observation.id())
                                            });
                                            self.last_error = None;
                                            self.user_retry_at = None;
                                            self.user_retry_delay = Duration::from_secs(1);
                                            Ok(completed.response.clone())
                                        }
                                        Err(error) => Err(RuntimeError::Companion(error)),
                                    }
                                }
                                Err(error) => {
                                    companion.discard_provider_session();
                                    Err(RuntimeError::Companion(error))
                                }
                            }
                        }
                        Err(error) => {
                            companion.discard_provider_session();
                            Err(error)
                        }
                    };
                    let attachment_reason = match &committed_result {
                        Err(RuntimeError::Companion(CompanionError::AttachmentOcr(reason))) => {
                            Some(*reason)
                        }
                        _ => None,
                    };
                    if let Some(reason) = attachment_reason {
                        self.preserve_proactive_during_user_failure(&companion);
                        match self.record_attachment_ocr_failure(
                            &companion,
                            &input_id,
                            reason,
                            snapshot_tx,
                        ) {
                            Ok(retryable) => terminal_attachment_failure = !retryable,
                            Err(error) => {
                                self.schedule_user_retry(
                                    initialization_error_kind(&error),
                                    snapshot_tx,
                                );
                                committed_result = Err(RuntimeError::Companion(error));
                            }
                        }
                    } else if let Err(RuntimeError::Companion(error)) = &committed_result {
                        self.preserve_proactive_during_user_failure(&companion);
                        self.schedule_user_retry(initialization_error_kind(error), snapshot_tx);
                    }
                    if committed_result.is_ok() {
                        if let Err(error) = self.restore_terminal_attachment_failure(&companion) {
                            self.schedule_user_retry(
                                initialization_error_kind(&error),
                                snapshot_tx,
                            );
                            committed_result = Err(RuntimeError::Companion(error));
                        }
                    }
                    response_result = committed_result;
                }
                self.resume_proactive_after_user(&companion, cancelled || response_result.is_ok());
                if let Ok(response) = &response_result {
                    self.accept_companion_thought(response);
                }
                self.companion = Some(companion);
                self.active_user_message_id = None;
                self.companion_draft = None;
                pending_user_drain = if cancelled
                    || append_restart_requested
                    || response_result.is_ok()
                    || terminal_attachment_failure
                {
                    self.user_retry_at = None;
                    self.user_retry_delay = Duration::from_secs(1);
                    PendingUserDrain::Continue
                } else {
                    PendingUserDrain::Pause
                };
                if matches!(&reply, OperationReply::User(_)) && !retry_without_response {
                    self.respond_user_waiters(&response_ids, response_result);
                }
            }
            OperationOutcome::Initialize {
                companion,
                delivery_status_before,
                result,
            } => {
                let delivery_status_after = companion.pending_delivery_status();
                let mut companion = *companion;
                if preempted_for_user {
                    companion.discard_provider_session();
                }
                self.companion = Some(companion);
                match result {
                    Ok(()) => {
                        pending_user_drain = PendingUserDrain::Continue;
                        if self.provider_build_failed {
                            self.clear_non_attachment_error();
                            self.provider_build_failed = false;
                        }
                        self.companion_recovery_pending = true;
                        self.initialization_retry_at = None;
                        self.initialization_retry_delay = Duration::from_secs(1);
                        if delivery_status_before != delivery_status_after {
                            self.revision = self.revision.saturating_add(1);
                        }
                    }
                    Err(error) => {
                        pending_user_drain = PendingUserDrain::Continue;
                        if let Some(logger) = &self.logger {
                            let _ = logger.write(
                                "WARN",
                                "companion の保留観察の初期化に失敗しました: error-type=initialization",
                            );
                        }
                        self.schedule_initialization_retry(
                            initialization_error_kind(&error),
                            snapshot_tx,
                        );
                    }
                }
            }
            OperationOutcome::BuildAgents(result) => match result {
                Ok(mut agents) => {
                    if preempted_for_user {
                        if let Some(companion) = agents.companion.as_mut() {
                            companion.discard_provider_session();
                        }
                    }
                    self.observer = agents.observer.take();
                    if let Some(companion) = agents.companion.as_ref() {
                        self.companion_display_name = companion.display_name().to_owned();
                    }
                    self.companion = agents.companion.take();
                    self.refresh_user_preparer();
                    self.memory = agents.memory.take();
                    self.memory_run_at = self.memory.as_ref().map(|_| Instant::now());
                    self.agent_rebuild_pending = false;
                    self.user_commands_blocked = false;
                    self.initialization_retry_at = Some(Instant::now());
                }
                Err(_) => {
                    self.provider_build_failed = true;
                    if let Some(logger) = &self.logger {
                        let _ = logger.write(
                            "WARN",
                            "provider の構成に失敗しました: error-type=provider-build",
                        );
                    }
                    self.schedule_initialization_retry(RuntimeErrorKind::Provider, snapshot_tx);
                }
            },
            OperationOutcome::Memory(memory) => {
                let retry_now = memory.was_preempted();
                self.memory = Some(*memory);
                self.memory_run_at = Some(if retry_now {
                    Instant::now()
                } else {
                    Instant::now() + Duration::from_secs(60)
                });
            }
            OperationOutcome::Consolidate { memory, result } => {
                self.memory = Some(*memory);
                let result = result
                    .map(|_| {
                        self.revision = self.revision.saturating_add(1);
                        self.revision
                    })
                    .map_err(RuntimeError::Factory);
                if let OperationReply::Revision(response) = reply {
                    let _ = response.send(result);
                }
            }
        }
        pending_user_drain
    }
}
