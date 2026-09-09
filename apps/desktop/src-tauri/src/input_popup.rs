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
    CancelThenStart,
    #[cfg(test)]
    FinishSpeech,
}

pub(crate) fn speech_start_action(current: Option<InputPopupKind>) -> InputPopupStartAction {
    match current {
        None => InputPopupStartAction::Start,
        Some(InputPopupKind::Speech) => InputPopupStartAction::Focus,
        Some(InputPopupKind::CaptureImage | InputPopupKind::CaptureText) => {
            InputPopupStartAction::CancelThenStart
        }
    }
}

