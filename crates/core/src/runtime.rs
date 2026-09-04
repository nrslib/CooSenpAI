use crate::companion::{
    AttachmentOcrFailureKind, CompanionAgent, CompanionError, CompanionResponse,
};
use crate::config::{validate_config, Config};
use crate::memory::{MemoryService, MemoryStatus};
use crate::observer::{ObservationFrameInput, ObserverAgent, ObserverError};
use crate::ports::RuntimeLogger;
use crate::provider::ProviderUsage;
use crate::state::ObservationRecord;
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[path = "runtime_support.rs"]
mod support;
pub use support::empty_runtime;
use support::{
    config_update_last_error, control_uses_observer, drain_closed_commands,
    initialization_error_kind, linked_cancellation,
};
#[path = "runtime_control_operation.rs"]
mod control_operation;
#[path = "runtime_initialization.rs"]
mod initialization;
#[path = "runtime_observation_handle.rs"]
mod observation_handle;
#[path = "runtime_operation.rs"]
mod operation;
#[path = "runtime_operation_state.rs"]
mod operation_state;
use operation_state::{
    wait_for_running, AppendPendingResult, CancellationResult, OperationCancellation,
    OperationLane, PendingUserDrain, RunningOperation, StartResult,
};
#[path = "runtime_types.rs"]
mod types;
pub use types::{
    RuntimeAgents, RuntimeAttachmentOcrFailure, RuntimeError, RuntimeErrorKind, RuntimeFactory,
    RuntimeLastError, RuntimePhase, RuntimeSnapshot,
};
#[path = "runtime_handle_types.rs"]
mod handle_types;
use handle_types::{ControlCommand, PriorityCommand, UserCommand, UserQueueCommand};
pub use handle_types::{ProviderStartGate, RuntimeHandle};
#[path = "runtime_stream.rs"]
mod stream;
use stream::{ProviderStreamUpdate, RuntimeProviderEvents};
#[cfg(test)]
#[path = "runtime_test_barrier.rs"]
pub(crate) mod test_barrier;
#[path = "runtime_thought.rs"]
mod thought;
#[path = "runtime_user_handle.rs"]
mod user_handle;
#[path = "runtime_watch_scope.rs"]
mod watch_scope;

const COMMAND_CAPACITY: usize = 64;
const MAX_QUEUED_USER_COMMANDS_PER_TURN: usize = COMMAND_CAPACITY;
const MAX_QUEUED_CONTROL_COMMANDS_PER_TURN: usize = 8;

impl RuntimeHandle {
    pub async fn update_config(&self, config: Config) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        validate_config(&config).map_err(RuntimeError::from)?;
        self.advance_watch_scope_generation(&config);
        self.cancel_operations_for_config_update();
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::UpdateConfig {
                config: Box::new(config),
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn replace_config(
        &self,
        config: Config,
        agents: RuntimeAgents,
    ) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        validate_config(&config).map_err(RuntimeError::from)?;
        self.advance_watch_scope_generation(&config);
        self.cancel_operations_for_config_update();
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::ReplaceConfig {
                config: Box::new(config),
                agents: Box::new(agents),
                response,
            })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn enter_degraded(&self, error: RuntimeLastError) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        self.cancel_operations();
        let (response, result) = oneshot::channel();
        self.priority_tx
            .send(PriorityCommand::EnterDegraded { error, response })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub async fn consolidate_memory(&self, period: String) -> Result<u64, RuntimeError> {
        self.ensure_open()?;
        let (response, result) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::ConsolidateMemory { period, response })
            .await
            .map_err(|_| RuntimeError::Closed)?;
        result.await.map_err(|_| RuntimeError::ResponseDropped)?
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    pub fn subscribe_snapshots(&self) -> watch::Receiver<RuntimeSnapshot> {
        self.snapshot_rx.clone()
    }

    pub fn config(&self) -> Config {
        self.config_rx.borrow().clone()
    }

    pub fn request_shutdown(&self) {
        self.cancellation.cancel();
    }

    pub fn cancel_operations(&self) {
        self.operation_cancellation.cancel_current();
    }

    pub(super) fn cancel_operations_for_config_update(&self) {
        self.operation_cancellation
            .cancel_current_for_config_update();
    }

    pub fn block_provider_starts(&self) -> ProviderStartGate {
        ProviderStartGate::new(self.operation_cancellation.clone())
    }

    pub fn block_provider_starts_for_config_update(&self) -> ProviderStartGate {
        ProviderStartGate::new_for_config_update(self.operation_cancellation.clone())
    }

    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        self.request_shutdown();
        let mut snapshot_rx = self.snapshot_rx.clone();
        loop {
            if snapshot_rx.borrow().phase == RuntimePhase::Stopping {
                return Ok(());
            }
            snapshot_rx
                .changed()
                .await
                .map_err(|_| RuntimeError::ResponseDropped)?;
        }
    }

    fn ensure_open(&self) -> Result<(), RuntimeError> {
        if self.cancellation.is_cancelled() {
            Err(RuntimeError::Closed)
        } else {
            Ok(())
        }
    }
}

pub struct RuntimeActor {
    observer: Option<ObserverAgent>,
    companion: Option<CompanionAgent>,
    config: Config,
    revision: u64,
    pending_observations: Vec<ObservationRecord>,
    phase: RuntimePhase,
    factory: Option<std::sync::Arc<dyn RuntimeFactory>>,
    logger: Option<std::sync::Arc<dyn RuntimeLogger>>,
    initialization_retry_at: Option<Instant>,
    initialization_retry_delay: Duration,
    user_retry_at: Option<Instant>,
    user_retry_delay: Duration,
    last_error: Option<RuntimeLastError>,
    provider_build_failed: bool,
    memory: Option<MemoryService>,
    memory_run_at: Option<Instant>,
    operation_cancellation: std::sync::Arc<OperationCancellation>,
    watch_scope_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    watch_scope_commit_lock: std::sync::Arc<std::sync::Mutex<()>>,
    turn_commit_lock: std::sync::Arc<std::sync::Mutex<()>>,
    companion_display_name: String,
    runtime_user_queue:
        std::sync::Arc<std::sync::Mutex<VecDeque<crate::companion_storage::PendingUserMessage>>>,
    user_preparer:
        std::sync::Arc<std::sync::RwLock<Option<crate::companion::user::UserMessagePreparer>>>,
    active_user_message_id: Option<String>,
    cancelled_user_message_ids: Vec<String>,
    companion_draft: Option<String>,
    latest_companion_thought: Option<String>,
    provider_usage: ProviderUsage,
    companion_recovery_pending: bool,
    companion_recovery_at: Option<Instant>,
    stream_tx: mpsc::UnboundedSender<ProviderStreamUpdate>,
    user_waiters: HashMap<String, oneshot::Sender<Result<CompanionResponse, RuntimeError>>>,
    user_work_pending: bool,
    user_commands_blocked: bool,
    user_cancel_recovery: Option<String>,
    agent_rebuild_pending: bool,
}

impl RuntimeActor {
    pub fn spawn(
        config: Config,
        observer: Option<ObserverAgent>,
        companion: Option<CompanionAgent>,
    ) -> RuntimeHandle {
        Self::spawn_internal(
            config,
            observer,
            companion,
            None,
            None,
            CancellationToken::new(),
            None,
        )
    }

    pub fn spawn_with_factory(
        config: Config,
        observer: Option<ObserverAgent>,
        companion: Option<CompanionAgent>,
        factory: std::sync::Arc<dyn RuntimeFactory>,
    ) -> RuntimeHandle {
        Self::spawn_internal(
            config,
            observer,
            companion,
            Some(factory),
            None,
            CancellationToken::new(),
            None,
        )
    }

    pub fn spawn_with_logger(
        config: Config,
        observer: Option<ObserverAgent>,
        companion: Option<CompanionAgent>,
        logger: std::sync::Arc<dyn RuntimeLogger>,
    ) -> RuntimeHandle {
        Self::spawn_internal(
            config,
            observer,
            companion,
            None,
            Some(logger),
            CancellationToken::new(),
            None,
        )
    }

    pub fn spawn_degraded_with_logger_and_cancellation(
        config: Config,
        logger: std::sync::Arc<dyn RuntimeLogger>,
        cancellation: CancellationToken,
        error: RuntimeLastError,
    ) -> RuntimeHandle {
        Self::spawn_internal(
            config,
            None,
            None,
            None,
            Some(logger),
            cancellation,
            Some(error),
        )
    }

    pub fn spawn_with_factory_logger_and_cancellation(
        config: Config,
        observer: Option<ObserverAgent>,
        companion: Option<CompanionAgent>,
        factory: std::sync::Arc<dyn RuntimeFactory>,
        logger: std::sync::Arc<dyn RuntimeLogger>,
        cancellation: CancellationToken,
    ) -> RuntimeHandle {
        Self::spawn_internal(
            config,
            observer,
            companion,
            Some(factory),
            Some(logger),
            cancellation,
            None,
        )
    }

    pub fn spawn_agents_with_factory_logger_and_cancellation(
        config: Config,
        mut agents: RuntimeAgents,
        factory: std::sync::Arc<dyn RuntimeFactory>,
        logger: std::sync::Arc<dyn RuntimeLogger>,
        cancellation: CancellationToken,
    ) -> RuntimeHandle {
        Self::spawn_internal_with_memory(
            config,
            agents.observer.take(),
            agents.companion.take(),
            agents.memory.take(),
            Some(factory),
            Some(logger),
            cancellation,
            None,
        )
    }

    fn spawn_internal(
        config: Config,
        observer: Option<ObserverAgent>,
        companion: Option<CompanionAgent>,
        factory: Option<std::sync::Arc<dyn RuntimeFactory>>,
        logger: Option<std::sync::Arc<dyn RuntimeLogger>>,
        cancellation: CancellationToken,
        initial_error: Option<RuntimeLastError>,
    ) -> RuntimeHandle {
        Self::spawn_internal_with_memory(
            config,
            observer,
            companion,
            None,
            factory,
            logger,
            cancellation,
            initial_error,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_internal_with_memory(
        config: Config,
        observer: Option<ObserverAgent>,
        companion: Option<CompanionAgent>,
        memory: Option<MemoryService>,
        factory: Option<std::sync::Arc<dyn RuntimeFactory>>,
        logger: Option<std::sync::Arc<dyn RuntimeLogger>>,
        cancellation: CancellationToken,
        initial_error: Option<RuntimeLastError>,
    ) -> RuntimeHandle {
        let companion_display_name = companion.as_ref().map_or_else(
            || config.companion.display_name.clone(),
            |agent| agent.display_name().to_owned(),
        );
        let runtime_user_queue = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new()));
        let user_preparer =
            std::sync::Arc::new(std::sync::RwLock::new(companion.as_ref().map(|agent| {
                agent.user_message_preparer_with_runtime_queue(runtime_user_queue.clone())
            })));
        let (control_tx, mut control_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (priority_tx, mut priority_rx) = mpsc::channel::<PriorityCommand>(COMMAND_CAPACITY);
        let (user_tx, mut user_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (stream_tx, mut stream_rx) = mpsc::unbounded_channel();
        let (snapshot_tx, snapshot_rx) = watch::channel(RuntimeSnapshot {
            revision: 0,
            phase: RuntimePhase::Idle,
            pending_observations: 0,
            last_error: initial_error.clone(),
            companion_retry_in_seconds: None,
            pending_deliveries: 0,
            delivery_outbox_blocked: false,
            memory_status: memory
                .as_ref()
                .map_or_else(MemoryStatus::default, |service| service.status().clone()),
            companion_display_name: companion_display_name.clone(),
            proactive_limit_reached: companion
                .as_ref()
                .is_some_and(CompanionAgent::proactive_limit_reached),
            active_user_message_id: None,
            cancelled_user_message_ids: Vec::new(),
            companion_draft: None,
            latest_companion_thought: None,
            provider_usage: ProviderUsage::default(),
        });
        let (config_tx, config_rx) = watch::channel(config.clone());
        let actor_cancellation = cancellation.clone();
        let operation_cancellation =
            std::sync::Arc::new(OperationCancellation::new(cancellation.clone()));
        let watch_scope_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let watch_scope_commit_lock = std::sync::Arc::new(std::sync::Mutex::new(()));
        let turn_commit_lock = std::sync::Arc::new(std::sync::Mutex::new(()));
        let actor_operation_cancellation = operation_cancellation.clone();
        let actor_watch_scope_generation = watch_scope_generation.clone();
        let actor_watch_scope_commit_lock = watch_scope_commit_lock.clone();
        let actor_turn_commit_lock = turn_commit_lock.clone();
        let actor_user_preparer = user_preparer.clone();
        tokio::spawn(async move {
            let initialization_retry_at = if initial_error.is_none()
                && (companion.is_some()
                    || (observer.is_none()
                        && companion.is_none()
                        && memory.is_none()
                        && factory.is_some()))
            {
                Some(Instant::now())
            } else {
                None
            };
            let mut actor = Self {
                observer,
                companion,
                config,
                revision: 0,
                pending_observations: Vec::new(),
                phase: RuntimePhase::Idle,
                factory,
                logger,
                initialization_retry_at,
                initialization_retry_delay: Duration::from_secs(1),
                user_retry_at: None,
                user_retry_delay: Duration::from_secs(1),
                last_error: initial_error,
                provider_build_failed: false,
                memory_run_at: memory.as_ref().map(|_| Instant::now()),
                memory,
                operation_cancellation: actor_operation_cancellation,
                watch_scope_generation: actor_watch_scope_generation,
                watch_scope_commit_lock: actor_watch_scope_commit_lock,
                turn_commit_lock: actor_turn_commit_lock,
                companion_display_name,
                runtime_user_queue,
                user_preparer: actor_user_preparer,
                active_user_message_id: None,
                cancelled_user_message_ids: Vec::new(),
                companion_draft: None,
                latest_companion_thought: None,
                provider_usage: ProviderUsage::default(),
                companion_recovery_pending: false,
                companion_recovery_at: None,
                stream_tx,
                user_waiters: HashMap::new(),
                user_work_pending: false,
                user_commands_blocked: false,
                user_cancel_recovery: None,
                agent_rebuild_pending: false,
            };
            let mut running_observer: Option<RunningOperation> = None;
            let mut running_coo: Option<RunningOperation> = None;
            let mut volatile_users = VecDeque::new();
            let mut control_queue = VecDeque::new();
            let mut user_commands_processed = 0;
            let mut control_commands_processed = 0;
            if let Some(logger) = &actor.logger {
                let _ = logger.write("INFO", "CooSenpAI runtimeを初期化しました。");
            }
            loop {
                for _ in
                    0..MAX_QUEUED_USER_COMMANDS_PER_TURN.saturating_sub(user_commands_processed)
                {
                    let Ok(command) = user_rx.try_recv() else {
                        break;
                    };
                    user_commands_processed += 1;
                    actor.accept_user_command(
                        command,
                        &mut volatile_users,
                        &mut running_coo,
                        &snapshot_tx,
                    );
                }
                if running_coo.is_none() && !actor.operation_cancellation.provider_starts_blocked()
                {
                    if actor.user_work_is_pending(&volatile_users) {
                        actor.user_work_pending = true;
                    }
                    if actor.user_work_is_pending(&volatile_users)
                        && actor.user_cancel_recovery.is_none()
                        && actor.pending_user_can_start()
                    {
                        match actor.start_pending_user_operation(
                            &mut volatile_users,
                            &snapshot_tx,
                            running_observer.is_some(),
                        ) {
                            StartResult::Running(operation) => {
                                running_coo = Some(*operation);
                            }
                            StartResult::Completed => {
                                actor.user_work_pending = actor.queued_user_work(&volatile_users)
                            }
                        }
                    }
                    if running_coo.is_none()
                        && !actor.user_work_is_pending(&volatile_users)
                        && actor.companion_recovery_pending
                        && actor.companion_recovery_can_start()
                    {
                        actor.companion_recovery_pending = false;
                        if let StartResult::Running(operation) =
                            actor.start_companion_recovery_operation(&snapshot_tx)
                        {
                            running_coo = Some(*operation);
                        }
                    }
                }
                for _ in 0..MAX_QUEUED_CONTROL_COMMANDS_PER_TURN
                    .saturating_sub(control_commands_processed)
                {
                    if control_queue.len() >= COMMAND_CAPACITY {
                        break;
                    }
                    let Ok(command) = control_rx.try_recv() else {
                        break;
                    };
                    control_commands_processed += 1;
                    control_queue.push_back(command);
                }
                let control_queue_len = control_queue.len();
                let mut observer_started = false;
                let mut coo_started = false;
                for _ in 0..control_queue_len {
                    let Some(command) = control_queue.pop_front() else {
                        break;
                    };
                    let observer_command = control_uses_observer(&command);
                    if matches!(&command, ControlCommand::CompanionObservations { .. })
                        && actor.user_work_is_pending(&volatile_users)
                    {
                        if let ControlCommand::CompanionObservations {
                            observations,
                            response,
                            ..
                        } = command
                        {
                            actor.defer_companion_observation_wake(
                                observations,
                                response,
                                &snapshot_tx,
                            );
                        }
                        continue;
                    }
                    let lane_free = if observer_command {
                        !observer_started && running_observer.is_none()
                    } else {
                        !coo_started
                            && running_coo.is_none()
                            && !actor.user_work_is_pending(&volatile_users)
                            && (!matches!(&command, ControlCommand::ReplaceConfigWhenIdle { .. })
                                || running_observer.is_none())
                    };
                    if !lane_free {
                        control_queue.push_back(command);
                        continue;
                    }
                    match actor.start_control_operation(command, &snapshot_tx, &config_tx) {
                        StartResult::Running(operation) if observer_command => {
                            running_observer = Some(*operation);
                            observer_started = true;
                        }
                        StartResult::Running(operation) => {
                            running_coo = Some(*operation);
                            coo_started = true;
                        }
                        StartResult::Completed => {}
                    }
                }
                let coo_idle = running_coo.is_none();
                let initialization_retry_deadline = actor
                    .initialization_retry_at
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(24 * 60 * 60));
                let user_retry_deadline = actor
                    .user_retry_at
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(24 * 60 * 60));
                let memory_deadline = actor
                    .memory_run_at
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(24 * 60 * 60));
                let companion_recovery_deadline = actor
                    .companion_recovery_at
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(24 * 60 * 60));
                let fairness_deadline = tokio::time::sleep(Duration::from_millis(1));
                tokio::pin!(fairness_deadline);
                tokio::select! {
                    biased;
                    _ = actor_cancellation.cancelled() => {
                        if let Some(operation) = running_observer.take() {
                            let _ = operation.cancel_and_wait().await;
                            actor.operation_cancellation.renew_lane(OperationLane::Observer);
                        }
                        if let Some(operation) = running_coo.take() {
                            let _ = operation.cancel_and_wait().await;
                            actor.operation_cancellation.renew_lane(OperationLane::Coo);
                        }
                        actor.close_user_waiters();
                        break;
                    }
                    _ = &mut fairness_deadline, if user_commands_processed >= MAX_QUEUED_USER_COMMANDS_PER_TURN
                        || control_commands_processed >= MAX_QUEUED_CONTROL_COMMANDS_PER_TURN => {
                        user_commands_processed = 0;
                        control_commands_processed = 0;
                    }
                    observer_result = wait_for_running(&mut running_observer), if running_observer.is_some() && priority_rx.is_empty() => {
                        if let Some(operation) = running_observer.take() {
                            let _ = actor.finish_operation(operation, observer_result, &snapshot_tx);
                            if actor.queued_user_work(&volatile_users) {
                                actor.user_work_pending = true;
                            }
                            if !actor.user_work_is_pending(&volatile_users) && !actor.pending_observations.is_empty() {
                                actor.companion_recovery_pending = true;
                            }
                        }
                        actor.phase = if running_coo.is_some() { RuntimePhase::Companion } else { RuntimePhase::Idle };
                        actor.revision = actor.revision.saturating_add(1);
                        actor.publish(&snapshot_tx);
                    }
                    coo_result = wait_for_running(&mut running_coo), if running_coo.is_some()
                        && user_commands_processed < MAX_QUEUED_USER_COMMANDS_PER_TURN => {
                        for _ in 0..MAX_QUEUED_USER_COMMANDS_PER_TURN
                            .saturating_sub(user_commands_processed)
                        {
                            let Ok(command) = user_rx.try_recv() else {
                                break;
                            };
                            user_commands_processed += 1;
                            actor.accept_user_command(
                                command,
                                &mut volatile_users,
                                &mut running_coo,
                                &snapshot_tx,
                            );
                        }
                        if let Some(operation) = running_coo.take() {
                            let was_user_operation = operation.is_user();
                            match actor.finish_operation(operation, coo_result, &snapshot_tx) {
                                PendingUserDrain::Continue => actor.user_work_pending = true,
                                PendingUserDrain::Pause => {
                                    actor.user_work_pending = actor.queued_user_work(&volatile_users)
                                }
                                PendingUserDrain::Unchanged => {}
                            }
                            if was_user_operation && !actor.pending_observations.is_empty() {
                                actor.companion_recovery_pending = true;
                            }
                        }
                        actor.phase = if running_observer.is_some() { RuntimePhase::Observing } else { RuntimePhase::Idle };
                        actor.revision = actor.revision.saturating_add(1);
                        actor.publish(&snapshot_tx);
                    }
                    Some(command) = priority_rx.recv() => {
                        match command {
                            PriorityCommand::CancelUser { response } => {
                                let input_id = actor
                                    .active_user_message_id
                                    .clone()
                                    .or_else(|| actor.terminal_attachment_input_id());
                                let mut termination_ack = true;
                                if let Some(operation) = running_coo.take() {
                                    match operation.cancel_and_wait().await {
                                        CancellationResult::Outcome(outcome) => {
                                            actor.restore_cancelled_operation(outcome, &snapshot_tx);
                                        }
                                        CancellationResult::TerminatedWithoutOutcome => {
                                            if let Some(input_id) = input_id.as_deref() {
                                                if let Err(_error) = actor
                                                    .cancel_user_input_after_termination(input_id)
                                                {
                                                    actor.user_cancel_recovery = Some(input_id.to_owned());
                                                    actor.user_retry_at = Some(Instant::now() + Duration::from_secs(1));
                                                    termination_ack = false;
                                                }
                                            } else {
                                                termination_ack = false;
                                            }
                                        }
                                    }
                                    actor.operation_cancellation.renew_lane(OperationLane::Coo);
                                }
                                let result = if !termination_ack {
                                    Err(RuntimeError::Closed)
                                } else if let Some(ref input_id) = input_id {
                                    actor.cancel_user_input(input_id)
                                } else {
                                    actor.cancel_active_user()
                                };
                                if let Some(input_id) = input_id {
                                    actor.respond_user_waiters(&[input_id], Err(RuntimeError::Closed));
                                }
                                let _ = response.send(result);
                                actor.publish(&snapshot_tx);
                            }
                            PriorityCommand::UpdateWatchEnabled { enabled, response } => {
                                let revision = actor.update_watch_enabled(
                                    enabled,
                                    &snapshot_tx,
                                    &config_tx,
                                );
                                let _ = response.send(Ok(revision));
                            }
                            command => {
                                let clear_user_state = matches!(
                                    &command,
                                    PriorityCommand::Quiesce {
                                        clear_user_state: true,
                                        ..
                                    }
                                );
                                let interrupted_operation = running_observer.is_some() || running_coo.is_some();
                                if let Some(operation) = running_observer.take() {
                                    if let Some(logger) = &actor.logger {
                                        let _ = logger.write("INFO", &format!("runtime 操作を中断しました: operation={} reason={}", operation.kind_label(), command.cancellation_reason()));
                                    }
                                    if let CancellationResult::Outcome(outcome) =
                                        operation.cancel_and_wait().await
                                    {
                                        actor.restore_cancelled_operation(outcome, &snapshot_tx);
                                    }
                                    actor.operation_cancellation.renew_lane(OperationLane::Observer);
                                }
                                if let Some(operation) = running_coo.take() {
                                    if let Some(logger) = &actor.logger {
                                        let _ = logger.write("INFO", &format!("runtime 操作を中断しました: operation={} reason={}", operation.kind_label(), command.cancellation_reason()));
                                    }
                                    let user_input_ids = operation.user_input_ids().to_vec();
                                    match operation.cancel_and_wait().await {
                                        CancellationResult::Outcome(outcome) => {
                                            actor.restore_cancelled_operation(outcome, &snapshot_tx);
                                        }
                                        CancellationResult::TerminatedWithoutOutcome => {
                                            for input_id in &user_input_ids {
                                                if actor
                                                    .cancel_user_input_after_termination(input_id)
                                                    .is_err()
                                                {
                                                    actor.user_cancel_recovery =
                                                        Some(input_id.clone());
                                                    actor.user_retry_at = Some(
                                                        Instant::now() + Duration::from_secs(1),
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    actor.operation_cancellation.renew_lane(OperationLane::Coo);
                                    actor.respond_user_waiters(&user_input_ids, Err(RuntimeError::Closed));
                                }
                                if clear_user_state {
                                    volatile_users.clear();
                                    actor.close_user_waiters();
                                    actor.user_work_pending = false;
                                    actor.user_commands_blocked = true;
                                }
                                actor.operation_cancellation.cancel_current();
                                if actor.handle_priority(command, interrupted_operation, &snapshot_tx, &config_tx).await { break; }
                            }
                        }
                    }
                    Some(command) = control_rx.recv(), if control_commands_processed < MAX_QUEUED_CONTROL_COMMANDS_PER_TURN && control_queue.len() < COMMAND_CAPACITY => {
                        if control_queue.len() < COMMAND_CAPACITY {
                            control_commands_processed += 1;
                            control_queue.push_back(command);
                        }
                    }
                    Some(update) = stream_rx.recv() => {
                        let input_id = match &update {
                            ProviderStreamUpdate::Delta { input_id, .. }
                            | ProviderStreamUpdate::Reset { input_id }
                            | ProviderStreamUpdate::Usage { input_id, .. } => input_id,
                        };
                        if actor.active_user_message_id.as_ref() == Some(input_id) {
                            match update {
                                ProviderStreamUpdate::Delta { text, .. } => {
                                    actor.companion_draft.get_or_insert_with(String::new).push_str(&text);
                                }
                                ProviderStreamUpdate::Reset { .. } => actor.companion_draft = None,
                                ProviderStreamUpdate::Usage { usage, .. } => actor.provider_usage = usage,
                            }
                            actor.revision = actor.revision.saturating_add(1);
                            actor.publish(&snapshot_tx);
                        }
                    }
                    _ = tokio::time::sleep_until(user_retry_deadline), if coo_idle && actor.user_retry_at.is_some() && !actor.operation_cancellation.provider_starts_blocked() => {
                        if let Some(input_id) = actor.user_cancel_recovery.clone() {
                            if actor.cancel_user_input_after_termination(&input_id).is_ok() {
                                actor.user_cancel_recovery = None;
                                actor.user_retry_at = None;
                                actor.user_work_pending = true;
                            } else {
                                actor.schedule_user_retry(RuntimeErrorKind::Provider, &snapshot_tx);
                            }
                        } else {
                            actor.user_retry_at = None;
                            actor.user_work_pending = true;
                        }
                    }
                    _ = tokio::time::sleep_until(initialization_retry_deadline), if coo_idle && (actor.agent_rebuild_pending || !actor.user_work_is_pending(&volatile_users) || actor.companion.is_none()) && actor.initialization_retry_at.is_some() && !actor.operation_cancellation.provider_starts_blocked() => {
                        if let StartResult::Running(operation) = actor.start_initialization_operation(&snapshot_tx) {
                            running_coo = Some(*operation);
                        }
                    }
                    _ = tokio::time::sleep_until(companion_recovery_deadline), if coo_idle && !actor.user_work_is_pending(&volatile_users) && actor.companion_recovery_at.is_some() && !actor.operation_cancellation.provider_starts_blocked() => {
                        actor.companion_recovery_at = None;
                        actor.companion_recovery_pending = true;
                    }
                    _ = tokio::time::sleep_until(memory_deadline), if coo_idle && !actor.user_work_is_pending(&volatile_users) && actor.memory_run_at.is_some() && !actor.operation_cancellation.provider_starts_blocked() => {
                        if let StartResult::Running(operation) = actor.start_memory_operation() {
                            running_coo = Some(*operation);
                        }
                    }
                    _ = actor.operation_cancellation.wait_for_provider_starts(), if actor.operation_cancellation.provider_starts_blocked() => {}
                    Some(command) = user_rx.recv(), if user_commands_processed < MAX_QUEUED_USER_COMMANDS_PER_TURN => {
                        user_commands_processed += 1;
                        actor.accept_user_command(
                            command,
                            &mut volatile_users,
                            &mut running_coo,
                            &snapshot_tx,
                        );
                    }
                    else => break,
                }
            }
            actor.close_user_waiters();
            drain_closed_commands(&mut control_rx, &mut priority_rx, &mut user_rx);
            if let Some(factory) = &actor.factory {
                factory.shutdown().await;
            }
            actor.phase = RuntimePhase::Stopping;
            actor.publish(&snapshot_tx);
            if let Some(logger) = &actor.logger {
                let _ = logger.write("INFO", "CooSenpAI runtimeを停止しました。");
            }
        });
        RuntimeHandle {
            control_tx,
            priority_tx,
            user_tx,
            cancellation,
            operation_cancellation,
            watch_scope_generation,
            watch_scope_commit_lock,
            turn_commit_lock,
            snapshot_rx,
            config_rx,
            user_preparer,
        }
    }

    async fn handle_priority(
        &mut self,
        command: PriorityCommand,
        interrupted_operation: bool,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
        config_tx: &watch::Sender<Config>,
    ) -> bool {
        match command {
            PriorityCommand::CancelUser { response } => {
                let _ = response.send(self.cancel_active_user());
                self.publish(snapshot_tx);
            }
            PriorityCommand::RetryUser { response } => {
                let result = self.retry_user();
                if result.is_ok() {
                    self.user_work_pending = true;
                }
                let _ = response.send(result);
                self.publish(snapshot_tx);
            }
            PriorityCommand::UpdateWatchEnabled { enabled, response } => {
                let revision = self.update_watch_enabled(enabled, snapshot_tx, config_tx);
                let _ = response.send(Ok(revision));
            }
            PriorityCommand::UpdateConfig { config, response } => {
                self.advance_watch_scope_generation(&config);
                let result = if interrupted_operation && self.factory.is_none() {
                    Err(RuntimeError::Factory(
                        "実行中の操作を設定変更後に再構築する RuntimeFactory がありません"
                            .to_owned(),
                    ))
                } else {
                    self.update_config(*config).await
                };
                if result.is_ok() {
                    let _ = config_tx.send(self.config.clone());
                } else if let Err(error) = &result {
                    self.enter_degraded(config_update_last_error(error));
                }
                let _ = response.send(result);
                self.publish(snapshot_tx);
            }
            PriorityCommand::ReplaceConfig {
                config,
                agents,
                response,
            } => {
                self.advance_watch_scope_generation(&config);
                let result = self.replace_config(*config, *agents);
                if result.is_ok() {
                    let _ = config_tx.send(self.config.clone());
                }
                let _ = response.send(result);
                self.publish(snapshot_tx);
            }
            PriorityCommand::EnterDegraded { error, response } => {
                let revision = self.enter_degraded(error);
                let _ = response.send(Ok(revision));
                self.publish(snapshot_tx);
            }
            PriorityCommand::Quiesce {
                response,
                clear_user_state,
            } => {
                let revision = self.quiesce_runtime(clear_user_state);
                let _ = response.send(Ok(revision));
                self.publish(snapshot_tx);
            }
        }
        false
    }
}

