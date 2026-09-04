use coosenpai_core::onboarding::TutorialStep;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DesktopCommand {
    ChatSend,
    ChatCancel,
    ChatRetry,
    CaptureStartImage,
    CaptureStartText,
    CaptureSendImage,
    CaptureSendText,
    CaptureCancel,
    SpeechStart,
    SpeechFinish,
    SpeechCancel,
    SpeechConfirm,
    SettingsAppearancePreview,
    ConfigDisplayUpdate,
    ConfigProviderUpdate,
    ProviderApiKeyUpdate,
    ConfigWatchUpdate,
    ConfigKeymapUpdate,
    WatchTargetUpdate,
    PersonaSelect,
    PersonaSave,
    PersonaDelete,
    PersonaRestore,
    PersonaReload,
    MemoryConfirm,
    MemoryReject,
    MemoryConfirmUpdate,
    MemoryRejectUpdate,
    MemoryDelete,
    MemoryConsolidate,
    ConversationReset,
    ConversationResetDismiss,
    BubbleDismiss,
    TutorialInteract,
    TutorialFastForward,
    TutorialAdvance,
    TutorialSettingsPresented,
    TutorialFinish,
    TutorialResume,
    TutorialRestart,
    SetupPrompt,
    SetupRestart,
    SettingsOpen,
    WatchStart,
    WatchStop,
    WatchPowerSuspend,
    WatchPowerResume,
    PresentTutorialResponse,
    CompanionPresence,
    CopyLastReply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandSource {
    IpcMain,
    IpcBubble,
    IpcCapturePopup,
    IpcSpeechPopup,
    IpcModelPopup,
    Tray,
    GlobalShortcut,
    SpeechCallback,
    TutorialAutomation,
    PowerEvent,
    RuntimeMonitor,
    Startup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandEnvelope {
    pub command_id: String,
    pub source: CommandSource,
    pub command: DesktopCommand,
    pub expected: GenerationFences,
}

impl CommandEnvelope {
    pub(crate) fn new(source: CommandSource, command: DesktopCommand) -> Self {
        Self {
            command_id: Uuid::new_v4().to_string(),
            source,
            command,
            expected: GenerationFences::default(),
        }
    }

    pub(crate) fn with_fence(mut self, stamp: GenerationStamp) -> Self {
        self.expected.insert(stamp);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TransitionOperation {
    FinishTutorial,
    ResetConversation,
    ReplaceConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecyclePhase {
    Running,
    ShuttingDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnboardingPhase {
    Setup,
    Tutorial {
        step: TutorialStep,
        chat_input_enabled: bool,
    },
    TutorialFinishing,
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExclusiveTransition {
    Idle,
    InProgress(TransitionOperation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourcePhase {
    Idle,
    Active,
    Transitioning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResourcePhases {
    pub runtime_available: bool,
    pub speech: ResourcePhase,
    pub capture: ResourcePhase,
    pub watch: ResourcePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagerState {
    pub lifecycle: LifecyclePhase,
    pub onboarding: OnboardingPhase,
    pub transition: ExclusiveTransition,
    pub resources: ResourcePhases,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolicyContext {
    pub manager: ManagerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RejectReason {
    ShuttingDown,
    TransitionInProgress,
    TutorialFinishing,
    SetupRequired,
    TutorialOperationNotAllowed,
    StaleGeneration,
    RuntimeUnavailable,
    InvalidInput,
}

impl RejectReason {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::ShuttingDown => "終了処理中です",
            Self::TransitionInProgress => "設定の反映処理中です",
            Self::TutorialFinishing => "終了処理をやり直してください",
            Self::SetupRequired => "初期設定を完了してください",
            Self::TutorialOperationNotAllowed => "この操作は現在の案内では使えません",
            Self::StaleGeneration => "古い操作の完了は反映されませんでした",
            Self::RuntimeUnavailable => "設定エラーで停止中です",
            Self::InvalidInput => "この状態では操作できません",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DispatchError {
    #[error("{0:?}")]
    Rejected(RejectReason),
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    #[allow(dead_code)]
    Indeterminate(String),
}

impl DispatchError {
    pub(crate) fn handler(error: impl ToString) -> Self {
        Self::Failed(error.to_string())
    }

    #[allow(dead_code)]
    pub(crate) fn indeterminate(error: impl ToString) -> Self {
        Self::Indeterminate(error.to_string())
    }

    pub(crate) fn format_for_user(&self) -> String {
        match self {
            Self::Rejected(reason) => reason.message().to_owned(),
            Self::Failed(message) | Self::Indeterminate(message) => message.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionPoint {
    DurableCoreAcceptance,
    DomainCommit,
    HandlerReturn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reservation {
    pub transition: Option<TransitionOperation>,
    pub completion: CompletionPoint,
    pub tutorial_response: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Admission {
    Accept {
        reservation: Reservation,
        fences: GenerationFences,
    },
    Reject(RejectReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum GenerationResource {
    Conversation,
    Speech,
    Capture,
    Bubble,
    Config,
    Finish,
    Watch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GenerationStamp {
    pub resource: GenerationResource,
    pub value: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GenerationFences(Vec<GenerationStamp>);

impl GenerationFences {
    pub(crate) fn insert(&mut self, stamp: GenerationStamp) {
        if let Some(current) = self
            .0
            .iter_mut()
            .find(|item| item.resource == stamp.resource)
        {
            *current = stamp;
        } else {
            self.0.push(stamp);
        }
    }

    pub(crate) fn get(&self, resource: GenerationResource) -> Option<GenerationStamp> {
        self.0
            .iter()
            .copied()
            .find(|item| item.resource == resource)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = GenerationStamp> + '_ {
        self.0.iter().copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum EventDecision {
    Apply,
    Park,
    DropStale,
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct PendingEvent {
    pub replacement_key: String,
    pub wait_for: GenerationResource,
    pub fence: GenerationStamp,
    pub deadline: Instant,
}

impl PendingEvent {
    #[cfg(test)]
    pub(crate) fn expires_after(
        replacement_key: impl Into<String>,
        fence: GenerationStamp,
        duration: std::time::Duration,
    ) -> Self {
        Self {
            replacement_key: replacement_key.into(),
            wait_for: fence.resource,
            fence,
            deadline: Instant::now() + duration,
        }
    }
}
