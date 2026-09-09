use std::sync::Arc;
use tauri::Manager;

pub(crate) enum SpeechViewCommand {
    Load(Arc<crate::speech::SpeechPopupSnapshot>),
    Show {
        focus: bool,
    },
    Hide,
    Focus,
    Input {
        generation: u64,
        edit_revision: u64,
        text: String,
        sending: bool,
        can_send: bool,
        error: Option<String>,
    },
}

#[derive(Clone)]
pub(crate) struct SpeechPopupView {
    app: tauri::AppHandle,
}

impl SpeechPopupView {
    pub(crate) fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }

    pub(crate) fn apply(&self, command: SpeechViewCommand) -> Result<(), String> {
        match command {
            SpeechViewCommand::Load(snapshot) => crate::webview_event::emit_to(&self.app, "speech-popup", "coosenpai:speech-popup:load", snapshot.as_ref()).map_err(|error| error.to_string()),
            SpeechViewCommand::Input { generation, edit_revision, text, sending, can_send, error } => crate::webview_event::emit_to(&self.app, "speech-popup", "coosenpai:speech-popup:input", &serde_json::json!({"generation": generation, "editRevision": edit_revision, "text": text, "sending": sending, "canSend": can_send, "error": error})).map_err(|error| error.to_string()),
            SpeechViewCommand::Focus => crate::webview_event::emit_to(&self.app, "speech-popup", "coosenpai:speech-popup:focus", &()).map_err(|error| error.to_string()),
            SpeechViewCommand::Show { focus } => {
                let window = self.app.get_webview_window("speech-popup").ok_or("音声入力ウインドウがありません")?;
                window.set_focusable(focus).map_err(|error| error.to_string())?;
                crate::windows::position_speech_popup(&window).map_err(|error| error.to_string())?;
                window.show().map_err(|error| error.to_string())?;
                if focus { window.set_focus().map_err(|error| error.to_string())?; }
                Ok(())
            }
            SpeechViewCommand::Hide => self.app.get_webview_window("speech-popup").ok_or("音声入力ウインドウがありません")?.hide().map_err(|error| error.to_string()),
        }
    }
}
