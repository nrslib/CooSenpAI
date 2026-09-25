use super::operation_state::StartResult;
use super::*;

impl RuntimeActor {
    pub(super) fn delete_conversation_log_day(
        &mut self,
        paths: &crate::config::ConfigPaths,
        date: chrono::NaiveDate,
        volatile_users: &mut std::collections::VecDeque<
            crate::companion_storage::PendingUserMessage,
        >,
    ) -> Result<(), RuntimeError> {
        let _turn_commit_guard = self
            .turn_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut hearing_context = self
            .hearing_context
            .lock()
            .map_err(|_| RuntimeError::CompanionUnavailable)?;
        let plan = crate::conversation_log::prepare_conversation_log_day_deletion(paths, date)
            .map_err(|error| RuntimeError::Observer(ObserverError::Persistence(error)))?;
        let scope = plan.scope().clone();
        let pending_audio = hearing_context
            .pending_audio_after_discard_for_day_with_ids(date, &scope.audio_ids)
            .map_err(|error| RuntimeError::Observer(ObserverError::Persistence(error)))?;
        let pending_observations = self
            .pending_observations
            .iter()
            .map(|observation| {
                crate::conversation_log::sanitize_observation_record(observation, &scope)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| RuntimeError::Observer(ObserverError::Persistence(error)))?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let volatile_users_after = volatile_users
            .iter()
            .map(|input| crate::conversation_log::sanitize_pending_user_message(input, &scope))
            .collect::<Result<std::collections::VecDeque<_>, _>>()
            .map_err(|error| RuntimeError::Observer(ObserverError::Persistence(error)))?;
        let runtime_queue_after = {
            let runtime_queue = self
                .runtime_user_queue
                .lock()
                .map_err(|_| RuntimeError::CompanionUnavailable)?;
            runtime_queue
                .iter()
                .map(|input| crate::conversation_log::sanitize_pending_user_message(input, &scope))
                .collect::<Result<std::collections::VecDeque<_>, _>>()
                .map_err(|error| RuntimeError::Observer(ObserverError::Persistence(error)))?
        };

        let observer_plan = self
            .observer
            .as_ref()
            .map(|observer| observer.prepare_conversation_log_day(&scope))
            .transpose()?;
        let companion_plan = self
            .companion
            .as_ref()
            .map(|companion| companion.prepare_conversation_log_day(&scope))
            .transpose()?;

        let storage = crate::companion_storage::CompanionStorage::from_paths(
            paths,
            self.config.retention.observation_days,
        );
        let storage_receipt = storage
            .discard_conversation_log_day(&scope)
            .map_err(runtime_companion_storage_deletion_error)?;
        if let Err(error) = plan.apply() {
            return Err(rollback_delete_day_storage(
                RuntimeError::Observer(ObserverError::Persistence(error)),
                &storage_receipt,
            ));
        }

        if let (Some(observer), Some(observer_plan)) = (&mut self.observer, observer_plan) {
            observer.apply_conversation_log_day(observer_plan);
        }
        if let (Some(companion), Some(companion_plan)) = (&mut self.companion, companion_plan) {
            companion.apply_conversation_log_day(companion_plan);
        }

        self.pending_observations = pending_observations;
        *volatile_users = volatile_users_after;
        *self
            .runtime_user_queue
            .lock()
            .map_err(|_| RuntimeError::CompanionUnavailable)? = runtime_queue_after;
        self.user_work_pending = self.queued_user_work(volatile_users);
        hearing_context.replace_pending_audio(pending_audio);
        Ok(())
    }

    pub(super) async fn start_control_operation(
        &mut self,
        command: ControlCommand,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
        config_tx: &watch::Sender<Config>,
        control_tx: &tokio::sync::mpsc::Sender<ControlCommand>,
    ) -> StartResult {
        match command {
            ControlCommand::BeginHearingSession {
                generation,
                cancellation,
                response,
            } => {
                if !response.is_closed() {
                    let result = self.begin_hearing_session(generation, cancellation);
                    let _ = response.send(result);
                }
                StartResult::Completed
            }
            ControlCommand::CompanionObservations { response, .. }
                if self.observation_delivery == ObservationDelivery::CallerOnly =>
            {
                let _ = response.send(Ok(CompanionObservationResult {
                    response: crate::companion::silent_response(),
                    call_id: None,
                    deferred: false,
                }));
                StartResult::Completed
            }
            ControlCommand::ProcessCompanionMailbox { response, .. }
                if self.observation_delivery == ObservationDelivery::CallerOnly =>
            {
                let _ = response.send(Ok(crate::companion::silent_response()));
                StartResult::Completed
            }
            ControlCommand::Observe(request) => {
                self.start_observe(request, snapshot_tx, control_tx)
            }
            ControlCommand::Heartbeat {
                stagnation,
                cancellation,
                response,
            } => {
                self.process_heartbeat(stagnation, cancellation, response, snapshot_tx);
                StartResult::Completed
            }
            ControlCommand::CompanionObservations {
                observations,
                context_notice,
                cancellation,
                response,
            } => self.start_companion_observations(
                observations,
                context_notice,
                cancellation,
                response,
                snapshot_tx,
            ),
            ControlCommand::ProcessCompanionMailbox {
                cancellation,
                response,
            } => self.start_companion_mailbox(cancellation, response, snapshot_tx),
            ControlCommand::JudgeFeed {
                event_id,
                input_id,
                sign,
                strength,
                cancelled,
                response,
            } => self.start_judge_feed(event_id, input_id, sign, strength, cancelled, response),
            ControlCommand::JudgeCompleted { .. } => StartResult::Completed,
            ControlCommand::ReplaceCompanion {
                companion,
                config,
                response,
            } => {
                let result = self
                    .replace_companion_config(*companion, config.map(|value| *value))
                    .await;
                if result.is_ok() {
                    self.operation_cancellation
                        .cancel_current_for_config_update();
                    self.operation_cancellation.renew();
                    let _ = config_tx.send(self.config.clone());
                }
                let _ = response.send(result);
                self.publish(snapshot_tx);
                StartResult::Completed
            }
            ControlCommand::ReplaceConfigWhenIdle {
                config,
                agents,
                response,
            } => {
                let result = self.replace_config(*config, *agents).await;
                if result.is_ok() {
                    let _ = config_tx.send(self.config.clone());
                }
                let _ = response.send(result);
                self.publish(snapshot_tx);
                StartResult::Completed
            }
            ControlCommand::ConsolidateMemory { period, response } => {
                self.start_consolidate(period, response)
            }
        }
    }

    pub(super) fn hearing_session_initialization_pending(&self) -> bool {
        self.agent_rebuild_pending
            || (self.initialization_retry_at.is_some()
                && self.user_preparer.read().is_ok_and(|slot| slot.is_none()))
    }

    pub(super) fn hearing_session_provider_build_failed(&self) -> bool {
        self.provider_build_failed && self.user_preparer.read().is_ok_and(|slot| slot.is_none())
    }

    fn begin_hearing_session(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<(String, crate::hearing_ingestion::HearingAudioIngestion), RuntimeError> {
        if cancellation.is_cancelled() {
            return Err(RuntimeError::Closed);
        }
        let preparer = self
            .user_preparer
            .read()
            .map_err(|_| RuntimeError::CompanionUnavailable)?;
        let preparer = preparer
            .as_ref()
            .ok_or(RuntimeError::CompanionUnavailable)?;
        let session_id = preparer.begin_hearing_context(generation, cancellation)?;
        let ingestion = preparer.hearing_audio_ingestion(session_id.clone());
        Ok((session_id, ingestion))
    }
}

fn runtime_companion_storage_deletion_error(
    error: crate::companion_storage::CompanionStorageDeletionError,
) -> RuntimeError {
    match error {
        crate::companion_storage::CompanionStorageDeletionError::Persistence(error) => {
            RuntimeError::Companion(CompanionError::Persistence(error))
        }
        crate::companion_storage::CompanionStorageDeletionError::Mailbox(error) => {
            RuntimeError::Companion(CompanionError::Mailbox(error))
        }
        crate::companion_storage::CompanionStorageDeletionError::Outbox(error) => {
            RuntimeError::Companion(CompanionError::Outbox(error))
        }
    }
}

fn rollback_delete_day_storage(
    error: RuntimeError,
    receipt: &crate::companion_storage::CompanionStorageDeletionReceipt,
) -> RuntimeError {
    match receipt.rollback() {
        Ok(()) => error,
        Err(rollback) => RuntimeError::Observer(ObserverError::Persistence(
            crate::persistence::PersistenceError::Invalid(format!(
                "Coo の日次削除に失敗し、永続配達経路の復元にも失敗しました: error={error}; rollback={rollback}"
            )),
        )),
    }
}
