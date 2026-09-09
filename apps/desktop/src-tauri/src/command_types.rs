use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::onboarding::TutorialStep;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DesktopCommand {
    WorkApprove,
    WorkConfigure,
    ChatSend,
    ChatCancel,
    ChatRetry,
    CaptureStartImage,
    CaptureStartText,
    CaptureSendImage,
    CaptureSendText,
    SpeechStart,
    SpeechFinish,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "command authorization matrix retains cancel policy"
        )
    )]
    SpeechCancel,
    SpeechConfirm,
    VoiceOutputTest,
    VoiceOutputStop,
    SettingsAppearancePreview,
    ConfigDisplayUpdate,
    ConfigProviderUpdate,
    ProviderApiKeyUpdate,
    ConfigWatchUpdate,
    ConfigKeymapUpdate,
    WatchTargetUpdate,
    PersonaSelect,
    SetupPersonaSelect,
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
    ConversationSelect,
    CompanionEmotionsReset,
    ConversationResetDismiss,
    BubbleDismiss,
    TutorialInteract,
    BubbleFastForward,
    BubbleNavigate,
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
    LicenseDocumentOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandSource {
    IpcMain,
    IpcBubble,
    IpcCapturePopup,
    IpcSpeechPopup,
    IpcModelPopup,
    IpcDetails,
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
    SwitchConversation,
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
    pub(crate) fn message_for_locale(self, locale: Locale) -> &'static str {
        match self {
            Self::ShuttingDown => text(TextKey::CommandShuttingDown, locale),
            Self::TransitionInProgress => text(TextKey::CommandTransitionInProgress, locale),
            Self::TutorialFinishing => text(TextKey::CommandTutorialFinishing, locale),
            Self::SetupRequired => text(TextKey::CommandSetupRequired, locale),
            Self::TutorialOperationNotAllowed => {
                text(TextKey::CommandTutorialOperationNotAllowed, locale)
            }
            Self::StaleGeneration => text(TextKey::CommandStaleGeneration, locale),
            Self::RuntimeUnavailable => text(TextKey::CommandRuntimeUnavailable, locale),
            Self::InvalidInput => text(TextKey::CommandInvalidInput, locale),
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

    #[allow(dead_code)]
    pub(crate) fn format_for_user(&self) -> String {
        self.format_for_locale(Locale::Ja)
    }

    pub(crate) fn format_for_locale(&self, locale: Locale) -> String {
        match self {
            Self::Rejected(reason) => reason.message_for_locale(locale).to_owned(),
            Self::Failed(message) | Self::Indeterminate(message) => {
                if message == text(TextKey::CommandResultUnavailable, Locale::Ja) {
                    text(TextKey::CommandResultUnavailable, locale).to_owned()
                } else {
                    message.clone()
                }
            }
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
