use super::{CaptureKind, ReadyCapture};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(crate) enum CaptureEffect {
    OpenSelection {
        kind: CaptureKind,
        source: crate::command_guard::CommandSource,
        generation: u64,
        origin: super::CaptureOrigin,
    },
    CloseSelection {
        generation: u64,
        shutdown: bool,
    },
    LoadPopup {
        generation: u64,
        content: Arc<ReadyCapture>,
    },
    PopupSession(u64),
    ArmPopup(u64),
    RenderPopup(Arc<super::CapturePopupSnapshot>),
    HidePopup {
        content: Arc<ReadyCapture>,
        sent: bool,
    },
    Send {
        content: Arc<ReadyCapture>,
        message: String,
    },
    Voice {
        action: crate::ui_events::VoiceAction,
        cancellation: CancellationToken,
    },
    VoiceSend {
        generation: u64,
        text: String,
    },
    SpeechView(crate::speech_view::SpeechViewCommand),
    Input {
        generation: u64,
        edit_revision: u64,
        message: String,
        sending: bool,
        can_send: bool,
        preview_state: super::manager::PreviewState,
        error: Option<String>,
    },
    OpenAccessibilitySettings,
    Error(String),
}

pub(crate) enum CaptureResult {
    Selected(Option<Arc<ReadyCapture>>),
    LoadedPopup(Box<super::CapturePopupSnapshot>),
    VoiceStarted(u64),
    Sent(String),
    Done,
}

// 取消を一度開始した後は、要求側のtokenでhelper回収のfutureを破棄しない。
pub(super) async fn voice_control<T>(
    action: crate::ui_events::VoiceAction,
    cancellation: CancellationToken,
    operation: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if action == crate::ui_events::VoiceAction::Cancel {
        return operation.await;
    }
    tokio::select! { biased;
        () = cancellation.cancelled() => Err("音声入力の操作を取り消しました".into()),
        result = operation => result,
    }
}

impl std::fmt::Debug for CaptureEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CaptureEffect")
    }
}

#[derive(Debug)]
pub(crate) enum CaptureTask {
    Selection(u64),
    Popup(u64),
    Voice(u64),
    VoiceSend(u64),
    Send(u64),
}
impl CaptureTask {
    pub(crate) fn completed(self, result: Result<CaptureResult, String>) -> super::CaptureEvent {
        use super::CaptureEvent;
        match self {
            Self::Selection(generation) => CaptureEvent::Selection {
                generation,
                event: result
                    .and_then(|result| match result {
                        CaptureResult::Selected(content) => Ok(content),
                        _ => Err("取得イベントの応答形式が一致しません".into()),
                    })
                    .into(),
            },
            Self::Popup(generation) => CaptureEvent::PopupLoaded { generation, result },
            Self::Voice(generation) => CaptureEvent::VoiceControlFinished { generation, result },
            Self::VoiceSend(generation) => CaptureEvent::VoiceSent {
                generation,
                result: sent(result),
            },
            Self::Send(generation) => CaptureEvent::SendFinished {
                generation,
                result: sent(result),
            },
        }
    }
}
fn sent(result: Result<CaptureResult, String>) -> Result<String, String> {
    result.and_then(|result| match result {
        CaptureResult::Sent(id) => Ok(id),
        _ => Err("送信イベントの応答形式が一致しません".into()),
    })
}

#[derive(Debug)]
pub(crate) struct CloseReply {
    pub(crate) reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    pub(crate) sent: Result<String, String>,
}
#[derive(Debug)]
pub(crate) enum CaptureOutput {
    Reported,
    PopupFramePrepared {
        generation: u64,
        content: Arc<ReadyCapture>,
    },
    Presented {
        view: crate::ui_events::UiView,
        generation: u64,
    },
    Hidden {
        kind: CaptureKind,
        generation: u64,
        content: Arc<ReadyCapture>,
        sent: bool,
        reply: Option<CloseReply>,
        open_main: bool,
    },
}
#[derive(Debug)]
pub(crate) struct CaptureApplied {
    pub(crate) completion: CaptureOutput,
    pub(crate) result: Result<(), String>,
}

pub(crate) async fn apply(
    port: &impl super::manager::CapturePort,
    effects: Vec<CaptureEffect>,
    completion: CaptureOutput,
) -> crate::ui_events::EffectResult {
    let mut result = Ok(());
    for effect in effects {
        result = port.execute(effect).await.and_then(|result| match result {
            CaptureResult::Done => Ok(()),
            _ => Err("表示イベントの応答形式が一致しません".into()),
        });
        if result.is_err() {
            break;
        }
    }
    if matches!(completion, CaptureOutput::Reported) && result.is_ok() {
        return crate::ui_events::EffectResult {
            value: None,
            events: Vec::new(),
        };
    }
    crate::ui_events::EffectResult {
        value: None,
        events: vec![crate::ui_events::UiEvent::CaptureCompleted(Box::new(
            super::CaptureEvent::Applied(CaptureApplied { completion, result }),
        ))],
    }
}
