use super::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::{JoinError, JoinHandle};

pub(super) struct OperationCancellation {
    shutdown: CancellationToken,
    state: Mutex<OperationCancellationState>,
    start_gate_changed: Notify,
}

struct OperationCancellationState {
    observer: OperationCancellationSource,
    coo: OperationCancellationSource,
    provider_starts_blocked: bool,
}

struct OperationCancellationSource {
    token: CancellationToken,
    cause: OperationCancellationCause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationLane {
    Observer,
    Coo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationCancellationReason {
    Other,
    ConfigUpdate,
}

#[derive(Clone)]
pub(super) struct OperationCancellationCause {
    reason: Arc<Mutex<Option<OperationCancellationReason>>>,
}

impl OperationCancellationCause {
    fn new() -> Self {
        Self {
            reason: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn mark(&self, reason: OperationCancellationReason) {
        let mut current = self
            .reason
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.is_none() {
            *current = Some(reason);
        }
    }

    pub(super) fn get(&self) -> Option<OperationCancellationReason> {
        self.reason
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .copied()
    }
}

pub(super) struct OperationCancellationHandle {
    pub(super) token: CancellationToken,
    pub(super) cause: OperationCancellationCause,
}

impl OperationCancellation {
    pub(super) fn new(shutdown: CancellationToken) -> Self {
        Self {
            shutdown: shutdown.clone(),
            state: Mutex::new(OperationCancellationState {
                observer: OperationCancellationSource {
                    token: shutdown.child_token(),
                    cause: OperationCancellationCause::new(),
                },
                coo: OperationCancellationSource {
                    token: shutdown.child_token(),
                    cause: OperationCancellationCause::new(),
                },
                provider_starts_blocked: false,
            }),
            start_gate_changed: Notify::new(),
        }
    }

    pub(super) fn cancellation_for_start(&self) -> Option<OperationCancellationHandle> {
        self.cancellation_for_start_lane(OperationLane::Coo)
    }

    pub(super) fn cancellation_for_start_lane(
        &self,
        lane: OperationLane,
    ) -> Option<OperationCancellationHandle> {
        self.with_state(|state| {
            (!state.provider_starts_blocked).then(|| {
                let source = match lane {
                    OperationLane::Observer => &state.observer,
                    OperationLane::Coo => &state.coo,
                };
                OperationCancellationHandle {
                    token: source.token.clone(),
                    cause: source.cause.clone(),
                }
            })
        })
    }

    pub(super) fn cancel_current(&self) {
        self.cancel_current_with_reason(OperationCancellationReason::Other);
    }

    pub(super) fn cancel_current_for_config_update(&self) {
        self.cancel_current_with_reason(OperationCancellationReason::ConfigUpdate);
    }

    pub(super) fn cancel_lane(&self, lane: OperationLane) {
        self.cancel_lane_with_reason(lane, OperationCancellationReason::Other);
    }

    pub(super) fn cancel_lane_for_config_update(&self, lane: OperationLane) {
        self.cancel_lane_with_reason(lane, OperationCancellationReason::ConfigUpdate);
    }

    pub(super) fn renew(&self) {
        self.with_state(|state| {
            state.observer.token.cancel();
            state.coo.token.cancel();
            state.observer = self.new_source();
            state.coo = self.new_source();
            if state.provider_starts_blocked {
                state.observer.token.cancel();
                state.coo.token.cancel();
            }
        });
    }

    pub(super) fn renew_lane(&self, lane: OperationLane) {
        self.with_state(|state| {
            let token = self.shutdown.child_token();
            match lane {
                OperationLane::Observer => {
                    state.observer.token.cancel();
                    state.observer = OperationCancellationSource {
                        token,
                        cause: OperationCancellationCause::new(),
                    };
                }
                OperationLane::Coo => {
                    state.coo.token.cancel();
                    state.coo = OperationCancellationSource {
                        token,
                        cause: OperationCancellationCause::new(),
                    };
                }
            }
            if state.provider_starts_blocked {
                match lane {
                    OperationLane::Observer => state.observer.token.cancel(),
                    OperationLane::Coo => state.coo.token.cancel(),
                }
            }
        });
    }

    pub(super) fn block_provider_starts(&self) {
        self.block_provider_starts_with_reason(OperationCancellationReason::Other);
    }

    pub(super) fn block_provider_starts_for_config_update(&self) {
        self.block_provider_starts_with_reason(OperationCancellationReason::ConfigUpdate);
    }

    fn block_provider_starts_with_reason(&self, reason: OperationCancellationReason) {
        self.with_state(|state| {
            state.provider_starts_blocked = true;
            state.observer.cause.mark(reason);
            state.coo.cause.mark(reason);
            state.observer.token.cancel();
            state.coo.token.cancel();
        });
    }

    pub(super) fn unblock_provider_starts(&self) {
        self.with_state(|state| {
            if state.provider_starts_blocked {
                state.provider_starts_blocked = false;
                state.observer = self.new_source();
                state.coo = self.new_source();
            }
        });
        self.start_gate_changed.notify_waiters();
    }

    pub(super) fn provider_starts_blocked(&self) -> bool {
        self.with_state(|state| state.provider_starts_blocked)
    }

    pub(super) async fn wait_for_provider_starts(&self) {
        loop {
            let changed = self.start_gate_changed.notified();
            if !self.provider_starts_blocked() {
                return;
            }
            changed.await;
        }
    }

    fn with_state<T>(&self, operation: impl FnOnce(&mut OperationCancellationState) -> T) -> T {
        match self.state.lock() {
            Ok(mut state) => operation(&mut state),
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                operation(&mut state)
            }
        }
    }

    fn cancel_current_with_reason(&self, reason: OperationCancellationReason) {
        self.with_state(|state| {
            state.observer.cause.mark(reason);
            state.coo.cause.mark(reason);
            state.observer.token.cancel();
            state.coo.token.cancel();
        });
    }

    fn cancel_lane_with_reason(&self, lane: OperationLane, reason: OperationCancellationReason) {
        self.with_state(|state| match lane {
            OperationLane::Observer => {
                state.observer.cause.mark(reason);
                state.observer.token.cancel();
            }
            OperationLane::Coo => {
                state.coo.cause.mark(reason);
                state.coo.token.cancel();
            }
        });
    }

    fn new_source(&self) -> OperationCancellationSource {
        OperationCancellationSource {
            token: self.shutdown.child_token(),
            cause: OperationCancellationCause::new(),
        }
    }
}

pub(super) enum StartResult {
    Completed,
    Running(Box<RunningOperation>),
}

pub(super) enum PendingUserDrain {
    Unchanged,
    Continue,
    Pause,
}

pub(super) struct RunningOperation {
    pub(super) cancellation: CancellationToken,
    cancellation_cause: OperationCancellationCause,
    pub(super) relay_stop: Option<CancellationToken>,
    pub(super) reply: OperationReply,
    pub(super) task: JoinHandle<Box<OperationOutcome>>,
    pub(super) operation_input_ids: Vec<String>,
    user_append: Option<UserAppendControl>,
    kind: RunningOperationKind,
    preempted_for_user: bool,
}

pub(super) enum CancellationResult {
    Outcome(Box<OperationOutcome>),
    TerminatedWithoutOutcome,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunningOperationKind {
    Other,
    Observer,
    User,
    ProactiveCompanion,
}

pub(super) struct UserAppendControl {
    pub(super) tracked_input_ids: HashSet<String>,
    pub(super) operation_input_ids: Vec<String>,
    pub(super) operation_generation: Option<u64>,
    pub(super) dispatch_seq: u64,
    pub(super) sender: tokio::sync::mpsc::UnboundedSender<crate::provider::ProviderMidTurnInput>,
    pub(super) provider: Arc<dyn crate::provider::ProviderClient>,
    pub(super) restart_requested: bool,
}

pub(super) enum AppendPendingResult {
    NotUser,
    NoNewInput,
    Appended,
    RestartRequested,
}

pub(super) enum OperationReply {
    None,
    Observation(oneshot::Sender<Result<ObservationRecord, RuntimeError>>),
    Companion(oneshot::Sender<Result<CompanionResponse, RuntimeError>>),
    User(Vec<String>),
    Revision(oneshot::Sender<Result<u64, RuntimeError>>),
}

pub(super) enum OperationOutcome {
    Observe {
        observer: Box<ObserverAgent>,
        result: Result<ObservationRecord, RuntimeError>,
    },
    CompanionObservations {
        companion: Box<CompanionAgent>,
        user_epoch: u64,
        result: Result<crate::companion::CompanionCallOutcome, RuntimeError>,
    },
    CompanionMailbox {
        companion: Box<CompanionAgent>,
        result: Result<CompanionResponse, RuntimeError>,
    },
    User {
        companion: Box<CompanionAgent>,
        active_input_id: String,
        operation_generation: Option<u64>,
        result: Result<crate::companion::user::UserOperationResult, RuntimeError>,
    },
    Initialize {
        companion: Box<CompanionAgent>,
        delivery_status_before: (usize, bool),
        result: Result<(), CompanionError>,
    },
    BuildAgents(Result<Box<RuntimeAgents>, String>),
    Memory(Box<MemoryService>),
    Consolidate {
        memory: Box<MemoryService>,
        result: Result<(), String>,
    },
}

impl RunningOperation {
    pub(super) fn new(
        cancellation: OperationCancellationHandle,
        reply: OperationReply,
        task: JoinHandle<Box<OperationOutcome>>,
    ) -> Self {
        Self {
            cancellation: cancellation.token,
            cancellation_cause: cancellation.cause,
            relay_stop: None,
            reply,
            task,
            operation_input_ids: Vec::new(),
            user_append: None,
            kind: RunningOperationKind::Other,
            preempted_for_user: false,
        }
    }

    pub(super) fn linked(
        cancellation: CancellationToken,
        cancellation_cause: OperationCancellationCause,
        relay_stop: CancellationToken,
        reply: OperationReply,
        task: JoinHandle<Box<OperationOutcome>>,
    ) -> Self {
        Self {
            cancellation,
            cancellation_cause,
            relay_stop: Some(relay_stop),
            reply,
            task,
            operation_input_ids: Vec::new(),
            user_append: None,
            kind: RunningOperationKind::Observer,
            preempted_for_user: false,
        }
    }

    pub(super) fn proactive(
        cancellation: OperationCancellationHandle,
        relay_stop: Option<CancellationToken>,
        reply: OperationReply,
        task: JoinHandle<Box<OperationOutcome>>,
    ) -> Self {
        Self {
            cancellation: cancellation.token,
            cancellation_cause: cancellation.cause,
            relay_stop,
            reply,
            task,
            operation_input_ids: Vec::new(),
            user_append: None,
            kind: RunningOperationKind::ProactiveCompanion,
            preempted_for_user: false,
        }
    }

    pub(super) fn user(
        cancellation: OperationCancellationHandle,
        operation_input_ids: Vec<String>,
        reply: OperationReply,
        task: JoinHandle<Box<OperationOutcome>>,
        user_append: Option<UserAppendControl>,
    ) -> Self {
        Self {
            cancellation: cancellation.token,
            cancellation_cause: cancellation.cause,
            relay_stop: None,
            reply,
            task,
            operation_input_ids,
            user_append,
            kind: RunningOperationKind::User,
            preempted_for_user: false,
        }
    }

    pub(super) fn preempt_for_user(&mut self) -> bool {
        if self.kind == RunningOperationKind::User {
            return false;
        }
        self.preempted_for_user = true;
        self.cancellation_cause
            .mark(OperationCancellationReason::Other);
        self.cancellation.cancel();
        true
    }

    pub(super) fn is_user(&self) -> bool {
        self.kind == RunningOperationKind::User
    }

    pub(super) fn is_observer(&self) -> bool {
        self.kind == RunningOperationKind::Observer
    }

    pub(super) fn user_input_ids(&self) -> &[String] {
        &self.operation_input_ids
    }

    pub(super) fn was_preempted_for_user(&self) -> bool {
        self.preempted_for_user
    }

    pub(super) fn cancellation_reason(&self) -> Option<OperationCancellationReason> {
        self.cancellation_cause.get()
    }

    pub(super) fn kind_label(&self) -> &'static str {
        match self.kind {
            RunningOperationKind::Other => "other",
            RunningOperationKind::Observer => "observer",
            RunningOperationKind::User => "user",
            RunningOperationKind::ProactiveCompanion => "proactive-companion",
        }
    }

    pub(super) fn append_pending_inputs(
        &mut self,
        pending: Vec<crate::companion_storage::PendingUserMessage>,
        preparer: &crate::companion::user::UserMessagePreparer,
    ) -> Result<AppendPendingResult, CompanionError> {
        let Some(control) = self.user_append.as_mut() else {
            return Ok(AppendPendingResult::NotUser);
        };
        let additional = pending
            .into_iter()
            .filter(|input| !control.tracked_input_ids.contains(&input.id))
            .collect::<Vec<_>>();
        if additional.is_empty() {
            return Ok(AppendPendingResult::NoNewInput);
        }
        preparer.extend_user_dispatch(
            control.dispatch_seq,
            &additional
                .iter()
                .map(|input| input.id.clone())
                .collect::<Vec<_>>(),
        )?;
        if !control
            .provider
            .capabilities()
            .is_some_and(|capabilities| capabilities.mid_turn_input)
        {
            let restart = preparer
                .request_restart(control.operation_generation, &control.operation_input_ids)?;
            control
                .tracked_input_ids
                .extend(additional.iter().map(|input| input.id.clone()));
            if restart {
                control.restart_requested = true;
                self.cancellation.cancel();
                return Ok(AppendPendingResult::RestartRequested);
            }
            return Ok(AppendPendingResult::NoNewInput);
        }
        for input in additional {
            let provider_input = match preparer.mid_turn_input(&input) {
                Ok(input) => input,
                Err(error) => {
                    control.restart_requested = true;
                    self.cancellation.cancel();
                    return Err(error);
                }
            };
            if control.sender.send(provider_input).is_err() {
                control.restart_requested = true;
                self.cancellation.cancel();
                return Ok(AppendPendingResult::RestartRequested);
            }
            control.tracked_input_ids.insert(input.id);
        }
        Ok(AppendPendingResult::Appended)
    }

    pub(super) fn append_restart_requested(&self) -> bool {
        self.user_append
            .as_ref()
            .is_some_and(|control| control.restart_requested)
    }

    pub(super) async fn cancel_and_wait(mut self) -> CancellationResult {
        self.cancellation_cause
            .mark(OperationCancellationReason::Other);
        self.cancellation.cancel();
        if let Some(stop) = self.relay_stop.take() {
            stop.cancel();
        }
        let reason = self
            .cancellation_cause
            .get()
            .unwrap_or(OperationCancellationReason::Other);
        self.reply.send_cancelled(reason);
        match tokio::time::timeout(Duration::from_secs(10), &mut self.task).await {
            Ok(Ok(outcome)) => CancellationResult::Outcome(outcome),
            Ok(Err(_)) => CancellationResult::TerminatedWithoutOutcome,
            Err(_) => {
                self.task.abort();
                let _ = self.task.await;
                CancellationResult::TerminatedWithoutOutcome
            }
        }
    }
}

impl OperationReply {
    pub(super) fn send_closed(self) {
        self.send_cancelled(OperationCancellationReason::Other);
    }

    pub(super) fn send_cancelled(self, reason: OperationCancellationReason) {
        match self {
            Self::None => {}
            Self::Observation(response) => {
                let _ = response.send(Err(cancellation_error(reason)));
            }
            Self::Companion(response) => {
                let _ = response.send(Err(cancellation_error(reason)));
            }
            Self::User(_) => {}
            Self::Revision(response) => {
                let _ = response.send(Err(RuntimeError::Closed));
            }
        }
    }
}

fn cancellation_error(reason: OperationCancellationReason) -> RuntimeError {
    match reason {
        OperationCancellationReason::Other => RuntimeError::Closed,
        OperationCancellationReason::ConfigUpdate => RuntimeError::ConfigUpdateCancelled,
    }
}

pub(super) async fn wait_for_running(
    operation: &mut Option<RunningOperation>,
) -> Result<Box<OperationOutcome>, JoinError> {
    match operation {
        Some(operation) => (&mut operation.task).await,
        None => std::future::pending().await,
    }
}

