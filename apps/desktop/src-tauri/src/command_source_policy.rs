use crate::command_guard::{CommandSource, DesktopCommand};

pub(crate) fn source_allows(source: CommandSource, command: DesktopCommand) -> bool {
    match source {
        CommandSource::IpcMain => main_allows(command),
        CommandSource::IpcBubble => matches!(
            command,
            DesktopCommand::TutorialInteract
                | DesktopCommand::TutorialAdvance
                | DesktopCommand::TutorialFastForward
                | DesktopCommand::MemoryConfirm
                | DesktopCommand::MemoryReject
                | DesktopCommand::ConversationReset
                | DesktopCommand::ConversationResetDismiss
                | DesktopCommand::SetupRestart
                | DesktopCommand::BubbleDismiss
                | DesktopCommand::ConfigWatchUpdate
                | DesktopCommand::SettingsOpen
        ),
        CommandSource::IpcCapturePopup => matches!(
            command,
            DesktopCommand::CaptureSendImage
                | DesktopCommand::CaptureSendText
                | DesktopCommand::CaptureCancel
        ),
        CommandSource::IpcSpeechPopup => matches!(
            command,
            DesktopCommand::SpeechConfirm | DesktopCommand::SpeechCancel
        ),
        CommandSource::IpcModelPopup => matches!(
            command,
            DesktopCommand::ConfigDisplayUpdate | DesktopCommand::ConfigProviderUpdate
        ),
        CommandSource::Tray => matches!(
            command,
            DesktopCommand::SettingsOpen | DesktopCommand::WatchStart | DesktopCommand::WatchStop
        ),
        CommandSource::GlobalShortcut => matches!(
            command,
            DesktopCommand::CaptureStartImage
                | DesktopCommand::CaptureStartText
                | DesktopCommand::SpeechStart
                | DesktopCommand::SpeechFinish
                | DesktopCommand::SpeechCancel
                | DesktopCommand::WatchStart
                | DesktopCommand::WatchStop
                | DesktopCommand::CopyLastReply
        ),
        CommandSource::SpeechCallback => command == DesktopCommand::SpeechConfirm,
        CommandSource::TutorialAutomation => matches!(
            command,
            DesktopCommand::TutorialInteract
                | DesktopCommand::TutorialAdvance
                | DesktopCommand::TutorialFinish
                | DesktopCommand::TutorialResume
                | DesktopCommand::SetupPrompt
        ),
        CommandSource::PowerEvent => matches!(
            command,
            DesktopCommand::WatchPowerSuspend | DesktopCommand::WatchPowerResume
        ),
        CommandSource::RuntimeMonitor => matches!(
            command,
            DesktopCommand::PresentTutorialResponse | DesktopCommand::CompanionPresence
        ),
        CommandSource::Startup => command == DesktopCommand::WatchStart,
    }
}

fn main_allows(command: DesktopCommand) -> bool {
    match command {
        DesktopCommand::ChatSend
        | DesktopCommand::ChatCancel
        | DesktopCommand::ChatRetry
        | DesktopCommand::SpeechStart
        | DesktopCommand::SpeechFinish
        | DesktopCommand::SpeechCancel
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
        | DesktopCommand::MemoryConsolidate
        | DesktopCommand::ConversationReset
        | DesktopCommand::TutorialAdvance
        | DesktopCommand::TutorialSettingsPresented
        | DesktopCommand::TutorialFinish
        | DesktopCommand::TutorialRestart
        | DesktopCommand::SetupPrompt
        | DesktopCommand::SetupRestart
        | DesktopCommand::SettingsOpen
        | DesktopCommand::WatchStart
        | DesktopCommand::WatchStop => true,
        DesktopCommand::CaptureStartImage
        | DesktopCommand::CaptureStartText
        | DesktopCommand::CaptureSendImage
        | DesktopCommand::CaptureSendText
        | DesktopCommand::CaptureCancel
        | DesktopCommand::SpeechConfirm
        | DesktopCommand::TutorialInteract
        | DesktopCommand::TutorialFastForward
        | DesktopCommand::TutorialResume
        | DesktopCommand::WatchPowerSuspend
        | DesktopCommand::WatchPowerResume
        | DesktopCommand::PresentTutorialResponse
        | DesktopCommand::CompanionPresence
        | DesktopCommand::CopyLastReply
        | DesktopCommand::BubbleDismiss
        | DesktopCommand::ConversationResetDismiss => false,
    }
}

