use super::*;

pub(super) enum ControlCommand {
    Observe {
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<ObservationRecord, RuntimeError>>,
    },
    Heartbeat {
        stagnation: Option<crate::state::StagnationObservation>,
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<ObservationRecord, RuntimeError>>,
    },
    AudioObservation {
        source: crate::state::AudioObservationSource,
        text: String,
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<ObservationRecord, RuntimeError>>,
    },
    CompanionObservations {
        observations: Vec<ObservationRecord>,
        context_notice: Option<String>,
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<CompanionResponse, RuntimeError>>,
    },
    ProcessCompanionMailbox {
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<CompanionResponse, RuntimeError>>,
    },
    ReplaceCompanion {
        companion: Box<CompanionAgent>,
        config: Option<Box<Config>>,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    ReplaceConfigWhenIdle {
        config: Box<Config>,
        agents: Box<RuntimeAgents>,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    ConsolidateMemory {
        period: String,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    },
}

pub(super) struct UserQueueCommand {
    pub(super) input_id: String,
    pub(super) response: Option<oneshot::Sender<Result<CompanionResponse, RuntimeError>>>,
}

pub(super) enum UserCommand {
    Enqueue(Box<UserQueueCommand>),
}

pub(super) enum PriorityCommand {
    CancelUser {
        response: oneshot::Sender<Result<(), RuntimeError>>,
    },
    RetryUser {
        response: oneshot::Sender<Result<String, RuntimeError>>,
    },
    UpdateConfig {
        config: Box<Config>,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    UpdateWatchEnabled {
        enabled: bool,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    ReplaceConfig {
        config: Box<Config>,
        agents: Box<RuntimeAgents>,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    EnterDegraded {
        error: RuntimeLastError,
        response: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    Quiesce {
        response: oneshot::Sender<Result<u64, RuntimeError>>,
        clear_user_state: bool,
    },
}

impl PriorityCommand {
    pub(super) fn cancellation_reason(&self) -> &'static str {
        match self {
            Self::CancelUser { .. } => "user-cancel",
            Self::RetryUser { .. } => "user-retry",
            Self::UpdateConfig { .. } | Self::ReplaceConfig { .. } => "config-update",
            Self::UpdateWatchEnabled { .. } => "watch-intent",
            Self::EnterDegraded { .. } => "degraded",
            Self::Quiesce { .. } => "quiesce",
        }
    }
}

#[derive(Clone)]
pub struct RuntimeHandle {
    pub(super) control_tx: mpsc::Sender<ControlCommand>,
    pub(super) priority_tx: mpsc::Sender<PriorityCommand>,
    pub(super) user_tx: mpsc::Sender<UserCommand>,
    pub(super) cancellation: CancellationToken,
    pub(super) operation_cancellation: std::sync::Arc<OperationCancellation>,
    pub(super) watch_scope_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(super) watch_scope_commit_lock: std::sync::Arc<std::sync::Mutex<()>>,
    pub(super) turn_commit_lock: std::sync::Arc<std::sync::Mutex<()>>,
    pub(super) snapshot_rx: watch::Receiver<RuntimeSnapshot>,
    pub(super) config_rx: watch::Receiver<Config>,
    pub(super) user_preparer:
        std::sync::Arc<std::sync::RwLock<Option<crate::companion::user::UserMessagePreparer>>>,
}

pub struct ProviderStartGate {
    operation_cancellation: std::sync::Arc<OperationCancellation>,
    active: bool,
}

impl ProviderStartGate {
    pub(super) fn new(operation_cancellation: std::sync::Arc<OperationCancellation>) -> Self {
        operation_cancellation.block_provider_starts();
        Self {
            operation_cancellation,
            active: true,
        }
    }

    pub(super) fn new_for_config_update(
        operation_cancellation: std::sync::Arc<OperationCancellation>,
    ) -> Self {
        operation_cancellation.block_provider_starts_for_config_update();
        Self {
            operation_cancellation,
            active: true,
        }
    }

    pub fn release(mut self) {
        self.operation_cancellation.unblock_provider_starts();
        self.active = false;
    }
}

impl Drop for ProviderStartGate {
    fn drop(&mut self) {
        if self.active {
            self.operation_cancellation.unblock_provider_starts();
        }
    }
}
