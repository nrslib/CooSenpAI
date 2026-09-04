use crate::command_types::CommandSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputPopupKind {
    CaptureImage,
    CaptureText,
    Speech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputPopupStartAction {
    Start,
    Focus,
    Cancel,
    CancelThenStart,
    FinishSpeech,
}

pub(crate) fn start_action(
    current: Option<InputPopupKind>,
    requested: InputPopupKind,
    source: CommandSource,
) -> InputPopupStartAction {
    match current {
        None => InputPopupStartAction::Start,
        Some(kind)
            if kind == requested
                && source == CommandSource::GlobalShortcut
                && matches!(
                    requested,
                    InputPopupKind::CaptureImage | InputPopupKind::CaptureText
                ) =>
        {
            InputPopupStartAction::Cancel
        }
        Some(kind) if kind == requested => InputPopupStartAction::Focus,
        Some(_) => InputPopupStartAction::CancelThenStart,
    }
}

pub(crate) fn microphone_action(mode: &str, recording: bool) -> InputPopupStartAction {
    if mode == "toggle" && recording {
        InputPopupStartAction::FinishSpeech
    } else {
        InputPopupStartAction::Start
    }
}

