pub use coosenpai_platform_macos::*;

pub fn clipboard_reader() -> std::sync::Arc<dyn coosenpai_core::ports::ClipboardReader> {
    std::sync::Arc::new(coosenpai_platform_macos::MacClipboardReader)
}

pub fn clipboard_writer() -> std::sync::Arc<dyn coosenpai_core::ports::ClipboardWriter> {
    std::sync::Arc::new(coosenpai_platform_macos::MacClipboardWriter)
}

pub fn selected_text_copier() -> std::sync::Arc<dyn coosenpai_core::ports::SelectedTextCopyPort> {
    std::sync::Arc::new(coosenpai_platform_macos::MacSelectedTextCopier)
}

pub fn bubble_display() -> std::sync::Arc<dyn coosenpai_core::ports::BubbleDisplayPort> {
    std::sync::Arc::new(coosenpai_platform_macos::MacBubbleDisplay)
}

pub fn speech_key_state() -> std::sync::Arc<dyn coosenpai_core::ports::SpeechKeyStatePort> {
    std::sync::Arc::new(coosenpai_platform_macos::MacSpeechKeyState)
}

pub fn speech_input_devices() -> std::sync::Arc<dyn coosenpai_core::ports::SpeechInputDevicePort> {
    std::sync::Arc::new(coosenpai_platform_macos::MacSpeechInputDevices)
}

pub fn speech_port(
    helper: std::path::PathBuf,
) -> std::sync::Arc<dyn coosenpai_core::ports::SpeechPort> {
    std::sync::Arc::new(coosenpai_platform_macos::MacSpeech::new(helper))
}

pub fn hearing_port(
    helper: std::path::PathBuf,
    logger: std::sync::Arc<dyn coosenpai_core::ports::RuntimeLogger>,
) -> std::sync::Arc<dyn coosenpai_core::ports::HearingPort> {
    std::sync::Arc::new(coosenpai_platform_macos::MacHearing::new(helper, logger))
}

pub fn provider_api_key_store() -> std::sync::Arc<dyn coosenpai_core::ports::ProviderApiKeyStore> {
    std::sync::Arc::new(coosenpai_platform_macos::MacKeychain)
}
