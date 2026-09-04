use crate::command_source_policy::source_allows;
use crate::command_types::*;
use coosenpai_core::onboarding::TutorialStep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermitClass {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandClass {
    Chat,
    TextAttachment,
    ImageAttachment,
    SpeechCapture,
    Voice,
    Cleanup,
    ConfigRestricted,
    ProviderCredential,
    WatchTargetConfig,
    ConfigDisplay,
    Persona,
    Memory,
    ConversationReset,
    PresentationDismiss,
    TutorialInteract,
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
    WatchPower,
    Presentation,
    AppearancePreview,
    Presence,
    Clipboard,
}

pub(crate) fn permit_class(command: DesktopCommand) -> PermitClass {
    match command {
        DesktopCommand::ConfigProviderUpdate
        | DesktopCommand::ProviderApiKeyUpdate
        | DesktopCommand::ConfigWatchUpdate
        | DesktopCommand::ConfigKeymapUpdate
        | DesktopCommand::WatchTargetUpdate
        | DesktopCommand::PersonaSelect
        | DesktopCommand::PersonaSave
        | DesktopCommand::PersonaDelete
        | DesktopCommand::PersonaRestore
        | DesktopCommand::PersonaReload
        | DesktopCommand::ConversationReset
        | DesktopCommand::TutorialFinish
        | DesktopCommand::TutorialRestart
        | DesktopCommand::SetupRestart
        | DesktopCommand::TutorialInteract
        | DesktopCommand::TutorialAdvance => PermitClass::Exclusive,
        DesktopCommand::ChatSend
        | DesktopCommand::ChatCancel
        | DesktopCommand::ChatRetry
        | DesktopCommand::CaptureStartImage
        | DesktopCommand::CaptureStartText
        | DesktopCommand::CaptureSendImage
        | DesktopCommand::CaptureSendText
        | DesktopCommand::CaptureCancel
        | DesktopCommand::SpeechStart
        | DesktopCommand::SpeechFinish
        | DesktopCommand::SpeechCancel
        | DesktopCommand::SpeechConfirm
        | DesktopCommand::SettingsAppearancePreview
        | DesktopCommand::ConfigDisplayUpdate
        | DesktopCommand::MemoryConfirm
        | DesktopCommand::MemoryReject
        | DesktopCommand::MemoryConfirmUpdate
        | DesktopCommand::MemoryRejectUpdate
        | DesktopCommand::MemoryDelete
        | DesktopCommand::MemoryConsolidate
        | DesktopCommand::ConversationResetDismiss
        | DesktopCommand::BubbleDismiss
        | DesktopCommand::TutorialFastForward
        | DesktopCommand::TutorialSettingsPresented
        | DesktopCommand::TutorialResume
        | DesktopCommand::SetupPrompt
        | DesktopCommand::SettingsOpen
        | DesktopCommand::WatchStart
        | DesktopCommand::WatchStop
        | DesktopCommand::WatchPowerSuspend
        | DesktopCommand::WatchPowerResume
        | DesktopCommand::PresentTutorialResponse
        | DesktopCommand::CompanionPresence
        | DesktopCommand::CopyLastReply => PermitClass::Shared,
    }
}

pub(crate) fn admit_command(context: &PolicyContext, envelope: &CommandEnvelope) -> Admission {
    if !source_allows(envelope.source, envelope.command) {
        return Admission::Reject(RejectReason::InvalidInput);
    }
    let manager = context.manager;
    if manager.lifecycle == LifecyclePhase::ShuttingDown {
        return Admission::Reject(RejectReason::ShuttingDown);
    }
    if matches!(manager.transition, ExclusiveTransition::InProgress(_)) {
        return Admission::Reject(RejectReason::TransitionInProgress);
    }
    let class = command_class(envelope.command);
    match manager.onboarding {
        OnboardingPhase::TutorialFinishing => finish_decision(class),
        OnboardingPhase::Setup => setup_decision(class),
        OnboardingPhase::Tutorial {
            step,
            chat_input_enabled,
        } => tutorial_decision(step, chat_input_enabled, envelope.source, class),
        OnboardingPhase::Normal => normal_decision(class),
    }
    .and_then_runtime(manager.resources.runtime_available, envelope.command)
}

trait RuntimeGate {
    fn and_then_runtime(self, available: bool, command: DesktopCommand) -> Admission;
}

impl RuntimeGate for Admission {
    fn and_then_runtime(self, available: bool, command: DesktopCommand) -> Admission {
        if matches!(self, Admission::Accept { .. }) && !available && requires_runtime(command) {
            Admission::Reject(RejectReason::RuntimeUnavailable)
        } else {
            self
        }
    }
}

fn accept(class: CommandClass, tutorial_response: Option<&'static str>) -> Admission {
    Admission::Accept {
        reservation: Reservation {
            transition: transition_for(class),
            completion: completion_for(class),
            tutorial_response,
        },
        fences: GenerationFences::default(),
    }
}

fn finish_decision(class: CommandClass) -> Admission {
    match class {
        CommandClass::PresentationDismiss | CommandClass::TutorialFinish => accept(class, None),
        CommandClass::Chat
        | CommandClass::TextAttachment
        | CommandClass::ImageAttachment
        | CommandClass::SpeechCapture
        | CommandClass::Voice
        | CommandClass::Cleanup
        | CommandClass::ConfigRestricted
        | CommandClass::ProviderCredential
        | CommandClass::WatchTargetConfig
        | CommandClass::ConfigDisplay
        | CommandClass::Persona
        | CommandClass::Memory
        | CommandClass::ConversationReset
        | CommandClass::TutorialInteract
        | CommandClass::TutorialAdvance
        | CommandClass::TutorialSettingsPresented
        | CommandClass::TutorialResume
        | CommandClass::TutorialRestart
        | CommandClass::SetupPrompt
        | CommandClass::SetupRestart
        | CommandClass::SettingsOpen
        | CommandClass::WatchStart
        | CommandClass::WatchStop
        | CommandClass::WatchPower
        | CommandClass::Presentation
        | CommandClass::AppearancePreview
        | CommandClass::Presence
        | CommandClass::Clipboard => Admission::Reject(RejectReason::TutorialFinishing),
    }
}

fn setup_decision(class: CommandClass) -> Admission {
    match class {
        CommandClass::TutorialInteract
        | CommandClass::PresentationDismiss
        | CommandClass::SetupPrompt
        | CommandClass::SetupRestart
        | CommandClass::SettingsOpen
        | CommandClass::ProviderCredential
        | CommandClass::AppearancePreview => accept(class, None),
        CommandClass::Cleanup | CommandClass::WatchStop | CommandClass::WatchPower => {
            accept(class, None)
        }
        CommandClass::Chat
        | CommandClass::TextAttachment
        | CommandClass::ImageAttachment
        | CommandClass::SpeechCapture
        | CommandClass::Voice
        | CommandClass::ConfigRestricted
        | CommandClass::WatchTargetConfig
        | CommandClass::ConfigDisplay
        | CommandClass::Persona
        | CommandClass::Memory
        | CommandClass::ConversationReset
        | CommandClass::TutorialAdvance
        | CommandClass::TutorialSettingsPresented
        | CommandClass::TutorialFinish
        | CommandClass::TutorialResume
        | CommandClass::TutorialRestart
        | CommandClass::WatchStart
        | CommandClass::Presentation
        | CommandClass::Presence
        | CommandClass::Clipboard => Admission::Reject(RejectReason::SetupRequired),
    }
}

fn tutorial_decision(
    step: TutorialStep,
    chat_input_enabled: bool,
    source: CommandSource,
    class: CommandClass,
) -> Admission {
    match class {
        CommandClass::Chat if step == TutorialStep::Chat && chat_input_enabled => {
            accept(class, Some("after-chat"))
        }
        CommandClass::SpeechCapture
            if (step == TutorialStep::Chat
                && chat_input_enabled
                && source == CommandSource::IpcMain)
                || step == TutorialStep::Voice =>
        {
            accept(class, None)
        }
        CommandClass::TextAttachment if step == TutorialStep::Text => {
            accept(class, Some("after-text"))
        }
        CommandClass::ImageAttachment if step == TutorialStep::Image => {
            accept(class, Some("after-image"))
        }
        CommandClass::Voice if step == TutorialStep::Voice => accept(class, Some("after-voice")),
        CommandClass::SettingsOpen
            if matches!(step, TutorialStep::Persona | TutorialStep::Watch) =>
        {
            accept(class, None)
        }
        CommandClass::Persona if step == TutorialStep::Persona => accept(class, None),
        CommandClass::TutorialSettingsPresented
            if matches!(step, TutorialStep::Persona | TutorialStep::Watch) =>
        {
            accept(class, None)
        }
        CommandClass::WatchStart if source == CommandSource::Startup => {
            Admission::Reject(RejectReason::TutorialOperationNotAllowed)
        }
        CommandClass::WatchStart if step == TutorialStep::Watch => accept(class, None),
        CommandClass::WatchTargetConfig if step == TutorialStep::Watch => accept(class, None),
        CommandClass::Cleanup
        | CommandClass::ConfigDisplay
        | CommandClass::TutorialInteract
        | CommandClass::TutorialAdvance
        | CommandClass::TutorialResume
        | CommandClass::WatchStop
        | CommandClass::WatchPower
        | CommandClass::PresentationDismiss
        | CommandClass::SetupRestart
        | CommandClass::Presentation
        | CommandClass::AppearancePreview => accept(class, None),
        CommandClass::TutorialFinish => accept(class, None),
        CommandClass::ProviderCredential
            if matches!(step, TutorialStep::Persona | TutorialStep::Watch) =>
        {
            accept(class, None)
        }
        CommandClass::Chat
        | CommandClass::TextAttachment
        | CommandClass::ImageAttachment
        | CommandClass::SpeechCapture
        | CommandClass::Voice
        | CommandClass::ConfigRestricted
        | CommandClass::ProviderCredential
        | CommandClass::WatchTargetConfig
        | CommandClass::Persona
        | CommandClass::Memory
        | CommandClass::ConversationReset
        | CommandClass::TutorialSettingsPresented
        | CommandClass::TutorialRestart
        | CommandClass::SetupPrompt
        | CommandClass::SettingsOpen
        | CommandClass::WatchStart
        | CommandClass::Presence
        | CommandClass::Clipboard => Admission::Reject(RejectReason::TutorialOperationNotAllowed),
    }
}

fn normal_decision(class: CommandClass) -> Admission {
    match class {
        CommandClass::Chat
        | CommandClass::TextAttachment
        | CommandClass::ImageAttachment
        | CommandClass::SpeechCapture
        | CommandClass::Voice
        | CommandClass::Cleanup
        | CommandClass::ConfigRestricted
        | CommandClass::ProviderCredential
        | CommandClass::WatchTargetConfig
        | CommandClass::ConfigDisplay
        | CommandClass::Persona
        | CommandClass::Memory
        | CommandClass::ConversationReset
        | CommandClass::PresentationDismiss
        | CommandClass::TutorialRestart
        | CommandClass::SetupRestart
        | CommandClass::SettingsOpen
        | CommandClass::WatchStart
        | CommandClass::WatchStop
        | CommandClass::WatchPower
        | CommandClass::AppearancePreview
        | CommandClass::Presence
        | CommandClass::Clipboard => accept(class, None),
        CommandClass::TutorialInteract
        | CommandClass::TutorialAdvance
        | CommandClass::TutorialSettingsPresented
        | CommandClass::TutorialFinish
        | CommandClass::TutorialResume
        | CommandClass::SetupPrompt
        | CommandClass::Presentation => Admission::Reject(RejectReason::InvalidInput),
    }
}

fn command_class(command: DesktopCommand) -> CommandClass {
    match command {
        DesktopCommand::ChatSend | DesktopCommand::ChatRetry => CommandClass::Chat,
        DesktopCommand::CaptureStartText | DesktopCommand::CaptureSendText => {
            CommandClass::TextAttachment
        }
        DesktopCommand::CaptureStartImage | DesktopCommand::CaptureSendImage => {
            CommandClass::ImageAttachment
        }
        DesktopCommand::SpeechStart => CommandClass::SpeechCapture,
        DesktopCommand::SpeechConfirm => CommandClass::Voice,
        DesktopCommand::ChatCancel
        | DesktopCommand::CaptureCancel
        | DesktopCommand::SpeechFinish
        | DesktopCommand::SpeechCancel => CommandClass::Cleanup,
        DesktopCommand::ConfigProviderUpdate | DesktopCommand::ConfigKeymapUpdate => {
            CommandClass::ConfigRestricted
        }
        DesktopCommand::ProviderApiKeyUpdate => CommandClass::ProviderCredential,
        DesktopCommand::ConfigWatchUpdate | DesktopCommand::WatchTargetUpdate => {
            CommandClass::WatchTargetConfig
        }
        DesktopCommand::ConfigDisplayUpdate => CommandClass::ConfigDisplay,
        DesktopCommand::PersonaSelect
        | DesktopCommand::PersonaSave
        | DesktopCommand::PersonaDelete
        | DesktopCommand::PersonaRestore
        | DesktopCommand::PersonaReload => CommandClass::Persona,
        DesktopCommand::MemoryConfirm
        | DesktopCommand::MemoryReject
        | DesktopCommand::MemoryConfirmUpdate
        | DesktopCommand::MemoryRejectUpdate
        | DesktopCommand::MemoryDelete
        | DesktopCommand::MemoryConsolidate => CommandClass::Memory,
        DesktopCommand::ConversationReset => CommandClass::ConversationReset,
        DesktopCommand::ConversationResetDismiss | DesktopCommand::BubbleDismiss => {
            CommandClass::PresentationDismiss
        }
        DesktopCommand::TutorialInteract => CommandClass::TutorialInteract,
        DesktopCommand::TutorialFastForward => CommandClass::Presentation,
        DesktopCommand::SettingsAppearancePreview => CommandClass::AppearancePreview,
        DesktopCommand::TutorialAdvance => CommandClass::TutorialAdvance,
        DesktopCommand::TutorialSettingsPresented => CommandClass::TutorialSettingsPresented,
        DesktopCommand::TutorialFinish => CommandClass::TutorialFinish,
        DesktopCommand::TutorialResume => CommandClass::TutorialResume,
        DesktopCommand::TutorialRestart => CommandClass::TutorialRestart,
        DesktopCommand::SetupPrompt => CommandClass::SetupPrompt,
        DesktopCommand::SetupRestart => CommandClass::SetupRestart,
        DesktopCommand::SettingsOpen => CommandClass::SettingsOpen,
        DesktopCommand::WatchStart => CommandClass::WatchStart,
        DesktopCommand::WatchStop => CommandClass::WatchStop,
        DesktopCommand::WatchPowerSuspend | DesktopCommand::WatchPowerResume => {
            CommandClass::WatchPower
        }
        DesktopCommand::PresentTutorialResponse => CommandClass::Presentation,
        DesktopCommand::CompanionPresence => CommandClass::Presence,
        DesktopCommand::CopyLastReply => CommandClass::Clipboard,
    }
}

fn requires_runtime(command: DesktopCommand) -> bool {
    match command {
        DesktopCommand::ChatSend
        | DesktopCommand::ChatRetry
        | DesktopCommand::CaptureSendImage
        | DesktopCommand::CaptureSendText
        | DesktopCommand::SpeechConfirm
        | DesktopCommand::MemoryConsolidate
        | DesktopCommand::WatchStart
        | DesktopCommand::WatchPowerResume
        | DesktopCommand::CompanionPresence => true,
        DesktopCommand::ChatCancel
        | DesktopCommand::CaptureStartImage
        | DesktopCommand::CaptureStartText
        | DesktopCommand::CaptureCancel
        | DesktopCommand::SpeechStart
        | DesktopCommand::SpeechFinish
        | DesktopCommand::SpeechCancel
        | DesktopCommand::TutorialFastForward
        | DesktopCommand::SettingsAppearancePreview
        | DesktopCommand::ConfigDisplayUpdate
        | DesktopCommand::ConfigProviderUpdate
        | DesktopCommand::ProviderApiKeyUpdate
        | DesktopCommand::ConfigWatchUpdate
        | DesktopCommand::ConfigKeymapUpdate
        | DesktopCommand::WatchTargetUpdate
        | DesktopCommand::PersonaSelect
        | DesktopCommand::PersonaSave
        | DesktopCommand::PersonaDelete
        | DesktopCommand::PersonaRestore
        | DesktopCommand::PersonaReload
        | DesktopCommand::MemoryConfirm
        | DesktopCommand::MemoryReject
        | DesktopCommand::MemoryConfirmUpdate
        | DesktopCommand::MemoryRejectUpdate
        | DesktopCommand::MemoryDelete
        | DesktopCommand::ConversationReset
        | DesktopCommand::ConversationResetDismiss
        | DesktopCommand::BubbleDismiss
        | DesktopCommand::TutorialInteract
        | DesktopCommand::TutorialAdvance
        | DesktopCommand::TutorialSettingsPresented
        | DesktopCommand::TutorialFinish
        | DesktopCommand::TutorialResume
        | DesktopCommand::TutorialRestart
        | DesktopCommand::SetupPrompt
        | DesktopCommand::SetupRestart
        | DesktopCommand::SettingsOpen
        | DesktopCommand::WatchStop
        | DesktopCommand::WatchPowerSuspend
        | DesktopCommand::PresentTutorialResponse => false,
        DesktopCommand::CopyLastReply => false,
    }
}

fn transition_for(class: CommandClass) -> Option<TransitionOperation> {
    match class {
        CommandClass::ConfigRestricted
        | CommandClass::ProviderCredential
        | CommandClass::WatchTargetConfig
        | CommandClass::Persona
        | CommandClass::TutorialRestart
        | CommandClass::SetupRestart => Some(TransitionOperation::ReplaceConfig),
        CommandClass::ConversationReset => Some(TransitionOperation::ResetConversation),
        CommandClass::TutorialFinish => Some(TransitionOperation::FinishTutorial),
        CommandClass::Chat
        | CommandClass::TextAttachment
        | CommandClass::ImageAttachment
        | CommandClass::SpeechCapture
        | CommandClass::Voice
        | CommandClass::Cleanup
        | CommandClass::Memory
        | CommandClass::PresentationDismiss
        | CommandClass::TutorialInteract
        | CommandClass::TutorialAdvance
        | CommandClass::TutorialSettingsPresented
        | CommandClass::TutorialResume
        | CommandClass::SetupPrompt
        | CommandClass::SettingsOpen
        | CommandClass::WatchStart
        | CommandClass::WatchStop
        | CommandClass::WatchPower
        | CommandClass::ConfigDisplay
        | CommandClass::Presentation
        | CommandClass::AppearancePreview
        | CommandClass::Presence => None,
        CommandClass::Clipboard => None,
    }
}

fn completion_for(class: CommandClass) -> CompletionPoint {
    match class {
        CommandClass::Chat
        | CommandClass::TextAttachment
        | CommandClass::ImageAttachment
        | CommandClass::Voice => CompletionPoint::DurableCoreAcceptance,
        CommandClass::ConfigRestricted
        | CommandClass::ProviderCredential
        | CommandClass::WatchTargetConfig
        | CommandClass::ConfigDisplay
        | CommandClass::SpeechCapture
        | CommandClass::Persona
        | CommandClass::Memory
        | CommandClass::ConversationReset
        | CommandClass::TutorialInteract
        | CommandClass::TutorialAdvance
        | CommandClass::TutorialSettingsPresented
        | CommandClass::TutorialFinish
        | CommandClass::TutorialResume
        | CommandClass::TutorialRestart
        | CommandClass::SetupRestart
        | CommandClass::WatchStart
        | CommandClass::WatchStop
        | CommandClass::WatchPower
        | CommandClass::Presence => CompletionPoint::DomainCommit,
        CommandClass::Cleanup
        | CommandClass::PresentationDismiss
        | CommandClass::SetupPrompt
        | CommandClass::SettingsOpen
        | CommandClass::Presentation
        | CommandClass::AppearancePreview
        | CommandClass::Clipboard => CompletionPoint::HandlerReturn,
    }
}
