use super::effects::{
    CaptureApplied, CaptureEffect, CaptureOutput, CaptureResult, CaptureTask, CloseReply,
};
use super::{CaptureKind, ReadyCapture};
use crate::presentation::{Presentation, PresentationAction, PresentationEvent, PresentationState};
use crate::ui_events::UiView;
use crate::ui_events::{Handling, PresenterId, UiEffect, UiEvent, UiTask};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

pub(crate) type CaptureFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'static>>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CancelSource {
    Esc,
    CloseButton,
    ActivationFailure,
    OutsideClick(crate::platform::MouseClick),
}

impl CancelSource {
    fn label(self) -> &'static str {
        match self {
            Self::Esc => "Esc",
            Self::CloseButton => "CloseButton",
            Self::ActivationFailure => "ActivationFailure",
            Self::OutsideClick(_) => "OutsideClick",
        }
    }

    fn should_cancel(self) -> bool {
        match self {
            Self::OutsideClick(click) => {
                !(click.x >= click.window_x
                    && click.x < click.window_x + click.window_width
                    && click.y >= click.window_y
                    && click.y < click.window_y + click.window_height)
            }
            Self::Esc | Self::CloseButton | Self::ActivationFailure => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapturePhase {
    Idle,
    Selecting,
    Popup,
    Sending,
    Closing,
    CloseFailed,
}

/// 管理者が公開する読み取り専用の投影。状態を変更する入口はイベントキューだけ。
#[derive(Clone)]
pub(crate) struct CaptureView {
    pub phase: CapturePhase,
    pub kind: Option<CaptureKind>,
    pub generation: u64,
    pub content: Option<Arc<ReadyCapture>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreviewState {
    #[default]
    Loading,
    Ready,
    Failed,
}

#[derive(Debug)]
pub(crate) enum SelectionEvent {
    Opened,
    Closed,
    Captured(Arc<ReadyCapture>),
    Failed(String),
}

impl From<Result<Option<Arc<ReadyCapture>>, String>> for SelectionEvent {
    fn from(result: Result<Option<Arc<ReadyCapture>>, String>) -> Self {
        match result {
            Ok(Some(content)) => Self::Captured(content),
            Ok(None) => Self::Closed,
            Err(error) => Self::Failed(error),
        }
    }
}

pub(crate) enum CaptureEvent {
    PopupRetry {
        generation: u64,
    },
    Applied(CaptureApplied),
    PopupLoaded {
        generation: u64,
        result: Result<CaptureResult, String>,
    },
    VoiceControlFinished {
        generation: u64,
        result: Result<CaptureResult, String>,
    },
    VoiceProgress(Arc<crate::speech::SpeechPopupSnapshot>),
    VoiceReleaseObserved {
        generation: u64,
    },
    VoiceEdit {
        generation: u64,
        edit_revision: u64,
        text: String,
    },
    VoiceKey {
        generation: u64,
        input: PopupKeyInput,
    },
    VoiceCancel {
        generation: u64,
    },
    VoiceSend {
        generation: u64,
        reply: oneshot::Sender<Result<String, String>>,
    },
    VoiceSent {
        generation: u64,
        result: Result<String, String>,
    },
    Activate {
        kind: CaptureKind,
        source: crate::command_guard::CommandSource,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    Ui(crate::ui_events::UiEvent),
    PopupSubmit {
        capture_id: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    PopupEdit {
        generation: u64,
        edit_revision: u64,
        message: String,
    },
    PopupKey {
        generation: u64,
        input: PopupKeyInput,
    },
    PopupAction {
        generation: u64,
        index: usize,
    },
    Shortcut(CaptureKind),
    Voice {
        action: crate::ui_events::VoiceAction,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    Selection {
        generation: u64,
        event: SelectionEvent,
    },
    PopupSend {
        capture_id: String,
        message: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    PopupCancel {
        generation: u64,
        source: CancelSource,
    },
    SendFinished {
        generation: u64,
        result: Result<String, String>,
    },
    // 会話切替・音声への切替・スリープは、従来のライフサイクルから明示的に通知する。
    Interrupt {
        selection_only: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

impl std::fmt::Debug for CaptureEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

impl CaptureEvent {
    fn label(&self) -> String {
        match self {
            Self::PopupRetry { generation } => format!("PopupRetry(generation={generation})"),
            Self::Applied(applied) => format!(
                "CaptureApplied({})",
                match &applied.completion {
                    CaptureOutput::Hidden { .. } => "Hidden",
                    CaptureOutput::Presented { .. } => "Presented",
                    CaptureOutput::Reported => "Reported",
                    CaptureOutput::PopupFramePrepared { .. } => "PopupFramePrepared",
                }
            ),
            Self::PopupLoaded { generation, .. } => format!("PopupLoaded(generation={generation})"),
            Self::PopupSubmit { .. } => "PopupSubmit".into(),
            Self::VoiceControlFinished { generation, .. } => {
                format!("VoiceControlFinished(generation={generation})")
            }
            Self::VoiceEdit { generation, .. } => format!("VoiceEdit(generation={generation})"),
            Self::VoiceKey { generation, .. } => format!("VoiceKey(generation={generation})"),
            Self::VoiceCancel { generation } => format!("VoiceCancel(generation={generation})"),
            Self::VoiceSend { generation, .. } => format!("VoiceSend(generation={generation})"),
            Self::VoiceSent { generation, .. } => format!("VoiceSent(generation={generation})"),
            Self::VoiceReleaseObserved { generation } => {
                format!("VoiceReleaseObserved({generation})")
            }
            Self::VoiceProgress(snapshot) => format!(
                "VoiceProgress(generation={},phase={})",
                snapshot.speech.generation, snapshot.speech.phase
            ),
            Self::Activate { kind, .. } => format!("Activate({kind:?})"),
            Self::Ui(event) => event.label(),
            Self::PopupEdit { generation, .. } => format!("PopupEdit(generation={generation})"),
            Self::PopupKey { generation, .. } => format!("PopupKey(generation={generation})"),
            Self::PopupAction { generation, .. } => format!("PopupAction(generation={generation})"),
            Self::Shortcut(kind) => format!("Shortcut({kind:?})"),
            Self::Voice { action, .. } => format!("Voice({action:?})"),
            Self::Selection { generation, event } => format!(
                "Selection(generation={generation},event={})",
                match event {
                    SelectionEvent::Opened => "Opened",
                    SelectionEvent::Closed => "Closed",
                    SelectionEvent::Captured(_) => "Captured",
                    SelectionEvent::Failed(_) => "Failed",
                }
            ),
            Self::PopupSend { .. } => "PopupSend".to_owned(),
            Self::PopupCancel { generation, source } => {
                format!("PopupCancel({},generation={generation})", source.label())
            }
            Self::SendFinished { generation, result } => format!(
                "SendFinished(generation={generation},result={})",
                if result.is_ok() { "ok" } else { "err" }
            ),
            Self::Interrupt { selection_only, .. } => {
                format!("Interrupt(selection_only={selection_only})")
            }
            Self::Shutdown => "Shutdown".to_owned(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PopupKeyInput {
    pub key: String,
    pub meta_key: bool,
    pub shift_key: bool,
    pub composing: bool,
    pub key_code: u32,
}

pub(crate) trait CapturePort: Send + Sync + 'static {
    fn execute(&self, event: CaptureEffect) -> CaptureFuture<CaptureResult>;
}

enum SelectionOperation {
    Open,
    Close,
    Reopen,
}

struct SendOperation {
    reply: oneshot::Sender<Result<String, String>>,
}

enum CaptureState {
    Idle,
    PopupClosing {
        kind: CaptureKind,
        generation: u64,
        content: Arc<ReadyCapture>,
    },
    PopupCloseFailed {
        kind: CaptureKind,
        generation: u64,
        content: Arc<ReadyCapture>,
        sent: bool,
    },
    Voice {
        generation: u64,
        speech_generation: Option<u64>,
        phase: CapturePhase,
    },
    Selecting {
        kind: CaptureKind,
        generation: u64,
        operation: SelectionOperation,
    },
    Popup {
        kind: CaptureKind,
        generation: u64,
        content: Arc<ReadyCapture>,
    },
    Sending {
        kind: CaptureKind,
        generation: u64,
        content: Arc<ReadyCapture>,
        operation: SendOperation,
    },
}

impl CaptureState {
    fn view(&self) -> CaptureView {
        match self {
            Self::PopupClosing {
                kind,
                generation,
                content,
            } => CaptureView {
                phase: CapturePhase::Closing,
                kind: Some(*kind),
                generation: *generation,
                content: Some(content.clone()),
            },
            Self::PopupCloseFailed {
                kind,
                generation,
                content,
                ..
            } => CaptureView {
                phase: CapturePhase::CloseFailed,
                kind: Some(*kind),
                generation: *generation,
                content: Some(content.clone()),
            },
            Self::Voice {
                generation, phase, ..
            } => CaptureView {
                phase: *phase,
                kind: Some(CaptureKind::Voice),
                generation: *generation,
                content: None,
            },
            Self::Idle => CaptureView {
                phase: CapturePhase::Idle,
                kind: None,
                generation: 0,
                content: None,
            },
            Self::Selecting {
                kind, generation, ..
            } => CaptureView {
                phase: CapturePhase::Selecting,
                kind: Some(*kind),
                generation: *generation,
                content: None,
            },
            Self::Popup {
                kind,
                generation,
                content,
            } => CaptureView {
                phase: CapturePhase::Popup,
                kind: Some(*kind),
                generation: *generation,
                content: Some(content.clone()),
            },
            Self::Sending {
                kind, generation, ..
            } => CaptureView {
                phase: CapturePhase::Sending,
                kind: Some(*kind),
                generation: *generation,
                content: None,
            },
        }
    }
}

#[derive(Clone)]
pub(crate) struct CaptureHandle {
    ui: crate::ui_root::UiHandle,
    view: watch::Receiver<CaptureView>,
}

impl CaptureHandle {

    pub(crate) async fn start_from(
        &self,
        kind: CaptureKind,
        source: crate::command_guard::CommandSource,
    ) -> Result<u64, String> {
        let (reply, result) = oneshot::channel();
        self.post(CaptureEvent::Activate {
            kind,
            source,
            reply,
        });
        result
            .await
            .map_err(|_| "入力の開始が完了しませんでした".to_owned())?
    }

    pub(crate) async fn voice(&self, action: crate::ui_events::VoiceAction) -> Result<u64, String> {
        let (reply, result) = oneshot::channel();
        self.post(CaptureEvent::Voice { action, reply });
        result
            .await
            .map_err(|_| "音声入力の処理が完了しませんでした".to_owned())?
    }
    pub(crate) fn post(&self, event: CaptureEvent) {
        self.ui.input(
            UiView::CapturePopup,
            UiEvent::CaptureCompleted(Box::new(event)),
        );
    }

    pub(crate) fn view(&self) -> CaptureView {
        self.view.borrow().clone()
    }

    pub(crate) async fn interrupt(&self, selection_only: bool) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.post(CaptureEvent::Interrupt {
            selection_only,
            reply,
        });
        result
            .await
            .map_err(|_| "取得の管理者は終了しました".to_owned())?
    }

    pub(crate) async fn shutdown(&self) {
        self.post(CaptureEvent::Shutdown);
        let mut view = self.view.clone();
        while view.changed().await.is_ok() {}
    }
}

pub(crate) struct CapturePresenter {
    effects: Vec<UiEffect>,
    state: CaptureState,
    generation: u64,
    popup_snapshot: Option<Arc<super::CapturePopupSnapshot>>,

    draft: String,
    edit_revision: u64,
    send_error: Option<String>,
    popup_ready: bool,
    preview_state: PreviewState,
    popup_presentation: Presentation,
    selection_interrupts: Vec<oneshot::Sender<Result<(), String>>>,
    popup_restart: Option<CaptureKind>,
    start_source: crate::command_guard::CommandSource,
    voice_control: Option<VoiceControl>,
    deferred_voice_result: Option<Result<CaptureResult, String>>,
    voice_send: Option<(u64, oneshot::Sender<Result<String, String>>)>,
    voice_snapshot: Option<Arc<crate::speech::SpeechPopupSnapshot>>,
    pending_transcript: Option<(u64, String)>,
    pending_voice_finish: Option<u64>,
    voice_ready: bool,
    voice_presentation: Presentation,
    voice_focus: bool,
    voice_draft_generation: Option<u64>,
    voice_interrupts: Vec<oneshot::Sender<Result<(), String>>>,
    closing: bool,
    view: Option<watch::Sender<CaptureView>>,
}

struct VoiceControl {
    generation: u64,
    action: crate::ui_events::VoiceAction,
    reply: VoiceControlReply,
    cancellation: CancellationToken,
}

enum VoiceControlReply {
    User(oneshot::Sender<Result<u64, String>>),
    Internal,
    FailedStart {
        reply: oneshot::Sender<Result<u64, String>>,
        error: String,
    },
}

pub(crate) fn channel(ui: crate::ui_root::UiHandle) -> (CaptureHandle, CapturePresenter) {
    let (view, projection) = watch::channel(CaptureState::Idle.view());
    (
        CaptureHandle {
            ui,
            view: projection,
        },
        CapturePresenter::new(view),
    )
}

impl CapturePresenter {
    fn start_voice_control(
        &mut self,
        action: crate::ui_events::VoiceAction,
        reply: VoiceControlReply,
    ) {
        if action == crate::ui_events::VoiceAction::Cancel {
            self.present(UiView::SpeechPopup, PresentationEvent::Hide);
        }
        let cancellation = CancellationToken::new();
        self.run(
            CaptureEffect::Voice {
                action,
                cancellation: cancellation.clone(),
            },
            CaptureTask::Voice(self.generation),
        );
        self.voice_control = Some(VoiceControl {
            generation: self.generation,
            action,
            reply,
            cancellation,
        });
    }

    fn finish_voice_control(&mut self, result: Result<CaptureResult, String>) {
        use crate::ui_events::VoiceAction;
        if self.voice_send.is_some()
            && self
                .voice_control
                .as_ref()
                .is_some_and(|control| control.action == VoiceAction::Cancel)
        {
            self.deferred_voice_result = Some(result);
            return;
        }
        let control = self.voice_control.take().expect("voice control completion");
        if matches!(control.action, VoiceAction::Start(_)) {
            if let Err(error) = result {
                if let VoiceControlReply::User(reply) = control.reply {
                    self.start_voice_control(
                        VoiceAction::Cancel,
                        VoiceControlReply::FailedStart { reply, error },
                    );
                }
                return;
            }
        }
        let result = match result {
            Ok(CaptureResult::VoiceStarted(speech_generation)) => {
                self.set_state(CaptureState::Voice {
                    generation: self.generation,
                    speech_generation: Some(speech_generation),
                    phase: CapturePhase::Selecting,
                });
                if let Some(generation) = self.pending_voice_finish.take() {
                    self.post(CaptureEvent::VoiceReleaseObserved { generation });
                }
                if let Some((generation, text)) = self.pending_transcript.take() {
                    self.post(CaptureEvent::Ui(
                        crate::ui_events::UiEvent::SpeechTranscript { generation, text },
                    ));
                }
                if let Some(snapshot) = &self.voice_snapshot {
                    self.post(CaptureEvent::VoiceProgress(snapshot.clone()));
                }
                Ok(self.generation)
            }
            Ok(CaptureResult::Done) => {
                if control.action == VoiceAction::Cancel {
                    self.voice_snapshot = None;
                    self.pending_transcript = None;
                    self.pending_voice_finish = None;
                    self.draft.clear();
                    self.send_error = None;
                    self.set_state(CaptureState::Idle);
                }
                Ok(self.generation)
            }
            Ok(_) => Err("音声操作の応答形式が一致しません".to_owned()),
            Err(error) => Err(error),
        };
        if !self.voice_interrupts.is_empty() || self.closing {
            if matches!(self.state, CaptureState::Idle) || control.action == VoiceAction::Cancel {
                for reply in self.voice_interrupts.drain(..) {
                    let _ = reply.send(result.clone().map(|_| ()));
                }
            } else {
                self.start_voice_control(VoiceAction::Cancel, VoiceControlReply::Internal);
            }
        }
        match control.reply {
            VoiceControlReply::User(reply) => {
                let _ = reply.send(result);
            }
            VoiceControlReply::FailedStart { reply, error } => {
                let _ = reply.send(Err(result.err().unwrap_or(error)));
            }
            VoiceControlReply::Internal => {}
        }
    }

    fn presentation_transition(
        &mut self,
        view: UiView,
        event: PresentationEvent<(), ()>,
    ) -> PresentationAction<(), ()> {
        let action = match view {
            UiView::CapturePopup => self.popup_presentation.transition(event),
            UiView::SpeechPopup => self.voice_presentation.transition(event),
            _ => unreachable!("capture-owned window"),
        };
        if let PresentationAction::Ignored { received, expected } = &action {
            self.log(&format!("ui: presenter={view:?} event=Loaded({received}) ignored=true reason=stale-generation expected={expected:?}"));
        }
        action
    }

    fn queue_presentation_loaded(&mut self, view: UiView) {
        let presentation = match view {
            UiView::CapturePopup if self.popup_ready => &self.popup_presentation,
            UiView::SpeechPopup if self.voice_ready && self.voice_snapshot.as_ref().is_some_and(|snapshot| {
                matches!(self.state, CaptureState::Voice { speech_generation: Some(generation), .. } if generation == snapshot.speech.generation)
            }) => &self.voice_presentation,
            _ => return,
        };
        if let PresentationState::Loading { generation } = presentation.state() {
            self.bubble(crate::ui_events::UiEvent::PopupWindow {
                view,
                event: PresentationEvent::Loaded {
                    generation,
                    result: Ok(Some(())),
                },
            });
        }
    }

    fn present(&mut self, view: UiView, event: PresentationEvent<(), ()>) {
        match self.presentation_transition(view, event) {
            PresentationAction::Load { .. } => self.queue_presentation_loaded(view),
            PresentationAction::Show(()) => {
                let mut effects = match view {
                    UiView::CapturePopup => {
                        let Some(content) = self.state.view().content else {
                            self.presentation_transition(view, PresentationEvent::Hide);
                            self.log("capture: no popup content");
                            return;
                        };
                        let generation = self.state.view().generation;
                        self.effects.push(UiEffect::Activation(
                            crate::activation_policy::ActivationInput::Popup {
                                generation,
                                content,
                                frame: self.popup_frame(generation),
                            },
                        ));
                        return;
                    }
                    UiView::SpeechPopup => {
                        let mut effects = self.voice_frame();
                        effects.push(CaptureEffect::SpeechView(
                            crate::speech_view::SpeechViewCommand::Show {
                                focus: self.voice_focus,
                            },
                        ));
                        if self.voice_focus {
                            effects.push(CaptureEffect::SpeechView(
                                crate::speech_view::SpeechViewCommand::Focus,
                            ));
                        }
                        effects
                    }
                    _ => unreachable!("capture-owned window"),
                };
                self.effects.push(UiEffect::CaptureView {
                    effects: std::mem::take(&mut effects),
                    completion: CaptureOutput::Presented {
                        view,
                        generation: self.generation,
                    },
                });
            }
            PresentationAction::Hide => {
                assert_eq!(view, UiView::SpeechPopup);
                self.output(CaptureEffect::SpeechView(
                    crate::speech_view::SpeechViewCommand::Hide,
                ));
            }
            PresentationAction::Ignored { .. } => {}
            PresentationAction::Unavailable | PresentationAction::Failed(_) => {
                unreachable!("popup frame readiness has no model payload")
            }
        }
    }

    fn voice_frame(&self) -> Vec<CaptureEffect> {
        let Some(snapshot) = self.voice_snapshot.clone() else {
            return Vec::new();
        };
        if !matches!(&self.state, CaptureState::Voice { speech_generation: Some(generation), .. } if *generation == snapshot.speech.generation)
        {
            return Vec::new();
        }
        vec![
            CaptureEffect::SpeechView(crate::speech_view::SpeechViewCommand::Load(
                snapshot.clone(),
            )),
            CaptureEffect::SpeechView(crate::speech_view::SpeechViewCommand::Input {
                generation: snapshot.speech.generation,
                edit_revision: self.edit_revision,
                text: self.draft.clone(),
                sending: self.voice_send.is_some() || snapshot.speech.phase == "sending",
                can_send: self.voice_send.is_none()
                    && snapshot.speech.phase == "confirming"
                    && !self.draft.trim().is_empty(),
                error: self.send_error.clone(),
            }),
        ]
    }

    fn present_voice(&mut self) {
        match self.voice_presentation.state() {
            PresentationState::Hidden => {}
            PresentationState::Loading { .. } => {
                self.queue_presentation_loaded(UiView::SpeechPopup)
            }
            PresentationState::Shown | PresentationState::CloseFailed => {
                let effects = self.voice_frame();
                self.effects.push(UiEffect::CaptureView {
                    effects,
                    completion: CaptureOutput::Reported,
                });
            }
        }
    }

    fn log(&mut self, message: &str) {
        self.effects.push(UiEffect::Log(message.to_owned()));
    }
    fn bubble(&mut self, event: UiEvent) {
        self.log(&format!("bubble:{}", event.label()));
        self.effects.push(UiEffect::Deliver {
            child: PresenterId::Root,
            event,
        });
    }
    fn post(&mut self, event: CaptureEvent) {
        self.effects.push(UiEffect::Deliver {
            child: PresenterId::Capture,
            event: UiEvent::CaptureCompleted(Box::new(event)),
        });
    }
    fn run(&mut self, effect: CaptureEffect, completion: CaptureTask) {
        self.effects
            .push(UiEffect::Spawn(UiTask::Capture { effect, completion }));
    }
    fn output(&mut self, effect: CaptureEffect) {
        if matches!(&effect, CaptureEffect::Error(error) if error == crate::platform::SCREENSHOT_ACCESS_REQUIRED)
        {
            self.effects.push(UiEffect::CaptureView {
                effects: vec![CaptureEffect::OpenAccessibilitySettings],
                completion: CaptureOutput::Reported,
            });
        }
        self.effects.push(UiEffect::CaptureView {
            effects: vec![effect],
            completion: CaptureOutput::Reported,
        });
    }
    fn show(&mut self, generation: u64, content: Arc<ReadyCapture>) {
        self.run(
            CaptureEffect::LoadPopup {
                generation,
                content,
            },
            CaptureTask::Popup(generation),
        );
        self.present(UiView::CapturePopup, PresentationEvent::Open(()));
    }
    fn popup_frame(&self, generation: u64) -> Vec<CaptureEffect> {
        let mut effects = vec![CaptureEffect::PopupSession(generation)];
        if let Some(snapshot) = self.popup_snapshot.clone() {
            effects.push(CaptureEffect::RenderPopup(snapshot));
        }
        effects.push(self.input_frame());
        effects
    }
    fn render_popup(&mut self, generation: u64) {
        self.effects.push(UiEffect::CaptureView {
            effects: self.popup_frame(generation),
            completion: CaptureOutput::Reported,
        });
    }
    fn input_frame(&self) -> CaptureEffect {
        let view = self.state.view();
        let can_send = matches!(&self.state, CaptureState::Popup { content, .. } if !matches!(content.attachment, super::ReadyAttachment::Text(None)))
            && self.popup_snapshot.is_some();
        CaptureEffect::Input {
            generation: view.generation,
            edit_revision: self.edit_revision,
            message: self.draft.clone(),
            sending: view.phase == CapturePhase::Sending,
            can_send,
            preview_state: self.preview_state,
            error: self.send_error.clone(),
        }
    }
    fn render_input(&mut self) {
        self.output(self.input_frame());
    }

    fn key_input(&mut self, generation: u64, input: PopupKeyInput) {
        if input.key == "Escape" {
            self.transition(CaptureEvent::PopupCancel {
                generation,
                source: CancelSource::Esc,
            });
            return;
        }
        let Some(snapshot) = self.popup_snapshot.clone() else {
            self.log("capture: event=PopupKey ignored=true reason=no-popup");
            return;
        };
        if snapshot.generation != generation || !matches!(self.state, CaptureState::Popup { .. }) {
            self.log("capture: event=PopupKey ignored=true reason=stale-or-busy");
            return;
        }
        if input.key == "Escape" {
            self.transition(CaptureEvent::PopupCancel {
                generation,
                source: CancelSource::Esc,
            });
        } else if input.key == "Enter" && !input.composing && input.key_code != 229 {
            let send = !input.shift_key && (snapshot.send_key == "enter" || input.meta_key);
            if send {
                let (reply, _) = oneshot::channel();
                self.transition(CaptureEvent::PopupSend {
                    capture_id: snapshot.capture_id.clone(),
                    message: self.draft.clone(),
                    reply,
                });
            }
        } else {
            self.log("capture: event=PopupKey ignored=true");
        }
    }

    fn handle_event(&mut self, event: CaptureEvent) -> crate::ui_events::Handling {
        let event = match event {
            CaptureEvent::Ui(crate::ui_events::UiEvent::CaptureCompleted(event)) => *event,
            event => event,
        };
        if matches!(&event, CaptureEvent::VoiceProgress(_))
            && !matches!(self.state, CaptureState::Voice { .. })
        {
            return crate::ui_events::Handling::Handled(Vec::new());
        }
        if self.closing {
            match event {
                CaptureEvent::Activate { reply, .. } | CaptureEvent::Voice { reply, .. } => {
                    let _ = reply.send(Err("入力の管理者は終了処理中です".to_owned()));
                    return crate::ui_events::Handling::Handled(Vec::new());
                }
                CaptureEvent::Shortcut(_) => {
                    self.log("capture: event=Shortcut ignored=true reason=shutdown");
                    return crate::ui_events::Handling::Handled(Vec::new());
                }
                _ => {}
            }
        }
        match event {
            CaptureEvent::Ui(UiEvent::UserCommand(command))
                if command.owner() == PresenterId::Capture =>
            {
                self.effects
                    .push(UiEffect::Spawn(UiTask::UserCommand(command)));
            }
            CaptureEvent::Ui(UiEvent::CommandFinished {
                owner: PresenterId::Capture,
                completion,
            }) => completion.reply(),
            CaptureEvent::Applied(applied) => self.applied(applied),
            CaptureEvent::Ui(UiEvent::PopupWindow { view, event }) => self.present(view, event),
            CaptureEvent::PopupRetry { generation } => {
                if let CaptureState::Popup {
                    generation: current,
                    content,
                    ..
                } = &self.state
                {
                    if *current == generation && self.preview_state == PreviewState::Failed {
                        let content = content.clone();
                        self.preview_state = PreviewState::Loading;
                        self.send_error = None;
                        self.render_input();
                        self.run(
                            CaptureEffect::LoadPopup {
                                generation,
                                content,
                            },
                            CaptureTask::Popup(generation),
                        );
                    }
                }
            }
            CaptureEvent::PopupLoaded { generation, result } => {
                if matches!(self.state, CaptureState::Popup { generation: current, .. } if current == generation)
                {
                    self.log(&format!("capture: popup loaded generation={generation}"));
                    match result {
                        Ok(CaptureResult::LoadedPopup(snapshot)) => {
                            self.preview_state = PreviewState::Ready;
                            self.popup_snapshot = Some(Arc::new(*snapshot));
                            self.send_error = None;
                        }
                        Ok(_) => {
                            self.preview_state = PreviewState::Failed;
                            self.send_error = Some("プレビュー読込の応答形式が一致しません".into())
                        }
                        Err(error) => {
                            self.preview_state = PreviewState::Failed;
                            self.send_error = Some(error);
                        }
                    }
                    if self.popup_presentation.state() == PresentationState::Shown {
                        self.render_popup(generation);
                    }
                } else {
                    self.log(&format!("capture: event=PopupLoaded({generation}) ignored=true reason=stale-generation"));
                }
            }
            CaptureEvent::Selection {
                generation,
                event: SelectionEvent::Opened,
            } => {
                if generation != self.generation {
                    self.log(&format!("capture: generation={generation} event=Opened ignored=true reason=stale-generation"));
                }
            }
            CaptureEvent::VoiceReleaseObserved { generation } => match self.state {
                CaptureState::Voice {
                    speech_generation: Some(current),
                    ..
                } if current == generation => {
                    return crate::ui_events::Handling::Bubble(crate::ui_events::UiEvent::Voice(
                        crate::ui_events::VoiceAction::Finish,
                    ))
                }
                CaptureState::Voice {
                    speech_generation: None,
                    ..
                } => self.pending_voice_finish = Some(generation),
                _ => self.log("capture: event=VoiceReleaseObserved ignored=true reason=stale"),
            },
            CaptureEvent::VoiceCancel { generation } => {
                if matches!(self.state, CaptureState::Voice { speech_generation: Some(current), .. } if current == generation)
                {
                    return crate::ui_events::Handling::Bubble(crate::ui_events::UiEvent::Voice(
                        crate::ui_events::VoiceAction::Cancel,
                    ));
                }
            }
            CaptureEvent::VoiceEdit {
                generation,
                edit_revision,
                text,
            } => {
                if matches!(self.state, CaptureState::Voice { speech_generation: Some(current), phase: CapturePhase::Popup, .. } if current == generation)
                    && self.voice_send.is_none()
                    && edit_revision >= self.edit_revision
                {
                    self.draft = text;
                    self.edit_revision = edit_revision;
                    self.send_error = None;
                    self.present_voice();
                }
            }
            CaptureEvent::VoiceKey { generation, input } => {
                if matches!(self.state, CaptureState::Voice { speech_generation: Some(current), .. } if current == generation)
                    && !input.composing
                    && input.key_code != 229
                {
                    if input.key == "Escape" {
                        return crate::ui_events::Handling::Bubble(
                            crate::ui_events::UiEvent::Voice(crate::ui_events::VoiceAction::Cancel),
                        );
                    }
                    if input.key == "Enter" && !input.shift_key {
                        let (reply, _) = oneshot::channel();
                        self.post(CaptureEvent::VoiceSend { generation, reply });
                    }
                }
            }
            CaptureEvent::VoiceSend { generation, reply } => {
                if matches!(self.state, CaptureState::Voice { speech_generation: Some(current), phase: CapturePhase::Popup, .. } if current == generation)
                    && self.voice_send.is_none()
                    && !self.draft.trim().is_empty()
                {
                    self.run(
                        CaptureEffect::VoiceSend {
                            generation,
                            text: self.draft.clone(),
                        },
                        CaptureTask::VoiceSend(generation),
                    );
                    self.voice_send = Some((generation, reply));
                    self.present_voice();
                } else {
                    let _ = reply.send(Err("音声入力の確認内容がありません".into()));
                }
            }
            CaptureEvent::VoiceSent { generation, result } => {
                if self
                    .voice_send
                    .as_ref()
                    .is_some_and(|(current, _)| *current == generation)
                {
                    let (_, reply) = self.voice_send.take().expect("voice send");
                    let cancelled = self.closing
                        || !self.voice_interrupts.is_empty()
                        || self.voice_control.as_ref().is_some_and(|control| {
                            control.action == crate::ui_events::VoiceAction::Cancel
                        });
                    self.send_error = result.as_ref().err().cloned();
                    let _ = reply.send(result);
                    if let Some(result) = self.deferred_voice_result.take() {
                        self.finish_voice_control(result);
                    }
                    if !cancelled {
                        self.present_voice();
                    }
                    if !cancelled {
                        return crate::ui_events::Handling::Bubble(
                            crate::ui_events::UiEvent::OpenMain,
                        );
                    }
                }
            }
            CaptureEvent::Activate {
                kind: CaptureKind::Voice,
                reply,
                ..
            } => {
                self.transition(CaptureEvent::Voice {
                    action: crate::ui_events::VoiceAction::Start(
                        crate::speech::SpeechSource::Shortcut,
                    ),
                    reply,
                });
            }
            CaptureEvent::Activate {
                kind,
                source,
                reply,
            } => {
                let previous_source = self.start_source;
                self.start_source = source;
                self.transition(CaptureEvent::Shortcut(kind));
                let view = self.state.view();
                let result = if view.kind == Some(kind)
                    && (view.phase == CapturePhase::Selecting
                        || (view.phase == CapturePhase::Closing
                            && self.popup_restart == Some(kind)))
                {
                    Ok(view.generation)
                } else {
                    self.start_source = previous_source;
                    Err("入力を開始できませんでした".to_owned())
                };
                let _ = reply.send(result);
            }
            CaptureEvent::PopupSubmit { capture_id, reply } => {
                self.transition(CaptureEvent::PopupSend {
                    capture_id,
                    message: self.draft.clone(),
                    reply,
                });
            }
            CaptureEvent::PopupKey { generation, input } => self.key_input(generation, input),
            CaptureEvent::PopupAction { generation, index } => {
                if let Some(snapshot) = &self.popup_snapshot {
                    if snapshot.generation == generation {
                        if let Some(action) = snapshot.quick_actions.get(index) {
                            let message = if self.draft.is_empty() {
                                action.message.clone()
                            } else {
                                format!("{}\n{}", action.message, self.draft)
                            };
                            let capture_id = snapshot.capture_id.clone();
                            let (reply, _) = oneshot::channel();
                            self.transition(CaptureEvent::PopupSend {
                                capture_id,
                                message,
                                reply,
                            });
                        }
                    }
                }
            }
            CaptureEvent::Ui(event @ crate::ui_events::UiEvent::SpeechTranscript { .. }) => {
                let crate::ui_events::UiEvent::SpeechTranscript { generation, text } = event else {
                    unreachable!()
                };
                match self.state {
                    CaptureState::Voice {
                        speech_generation: Some(current),
                        ..
                    } if current == generation => {
                        return crate::ui_events::Handling::Bubble(
                            crate::ui_events::UiEvent::SpeechTranscript { generation, text },
                        )
                    }
                    CaptureState::Voice {
                        speech_generation: None,
                        ..
                    } => self.pending_transcript = Some((generation, text)),
                    _ => self.log("capture: event=SpeechTranscript ignored=true reason=stale"),
                }
            }
            CaptureEvent::Ui(crate::ui_events::UiEvent::CaptureCancel { generation, source }) => {
                self.transition(CaptureEvent::PopupCancel { generation, source });
            }
            CaptureEvent::Ui(crate::ui_events::UiEvent::SnapshotUpdated(snapshot)) => {
                if let Some(current) = &self.popup_snapshot {
                    let mut popup = (**current).clone();
                    popup.revision = snapshot.revision;
                    popup.language = snapshot.config.ui.language.clone();
                    popup.theme = snapshot.config.ui.theme.clone();
                    popup.font = snapshot.config.ui.font.clone();
                    popup.avatar_color = snapshot.config.ui.avatar_color.clone();
                    popup.avatar_image_png = snapshot.avatar_image_png.clone();
                    popup.companion_display_name = snapshot.companion_display_name.clone();
                    popup.send_key = snapshot.config.keymap.send_key.clone();
                    popup.quick_actions = if popup.attachment_kind == "text" {
                        snapshot.config.popup.quick_actions.text.clone()
                    } else {
                        snapshot.config.popup.quick_actions.image.clone()
                    };
                    let popup = Arc::new(popup);
                    self.popup_snapshot = Some(popup.clone());
                    if self.popup_presentation.state() == PresentationState::Shown {
                        self.output(CaptureEffect::RenderPopup(popup));
                    }
                }
            }
            CaptureEvent::Ui(crate::ui_events::UiEvent::Mounted(
                crate::ui_events::UiView::CapturePopup,
            )) => {
                self.popup_ready = true;
                self.queue_presentation_loaded(UiView::CapturePopup);
                if self.popup_presentation.state() == PresentationState::Shown {
                    self.render_popup(self.state.view().generation);
                } else if let CaptureState::PopupCloseFailed { generation, .. } = self.state {
                    self.output(CaptureEffect::PopupSession(generation));
                    self.render_input();
                }
            }
            CaptureEvent::Ui(crate::ui_events::UiEvent::Mounted(
                crate::ui_events::UiView::SpeechPopup,
            )) => {
                self.voice_ready = true;
                self.present_voice();
            }
            CaptureEvent::Ui(event) => return crate::ui_events::Handling::Bubble(event),
            event => self.transition(event),
        }
        crate::ui_events::Handling::Handled(Vec::new())
    }
    fn new(view: watch::Sender<CaptureView>) -> Self {
        Self {
            effects: Vec::new(),
            state: CaptureState::Idle,
            generation: 0,
            popup_snapshot: None,
            draft: String::new(),
            edit_revision: 0,
            send_error: None,
            popup_ready: false,
            preview_state: PreviewState::Loading,
            popup_presentation: Presentation::default(),
            selection_interrupts: Vec::new(),
            popup_restart: None,
            start_source: crate::command_guard::CommandSource::GlobalShortcut,
            voice_control: None,
            deferred_voice_result: None,
            voice_send: None,
            voice_snapshot: None,
            pending_transcript: None,
            pending_voice_finish: None,
            voice_ready: false,
            voice_presentation: Presentation::default(),
            voice_focus: false,
            voice_draft_generation: None,
            voice_interrupts: Vec::new(),
            closing: false,
            view: Some(view),
        }
    }

    fn set_state(&mut self, state: CaptureState) {
        let before = self
            .view
            .as_ref()
            .expect("active capture projection")
            .borrow()
            .clone();
        let after = state.view();
        let protected = |phase| {
            matches!(
                phase,
                CapturePhase::Popup
                    | CapturePhase::Closing
                    | CapturePhase::CloseFailed
                    | CapturePhase::Sending
            )
        };
        if protected(before.phase) != protected(after.phase)
            || (protected(after.phase) && before.generation != after.generation)
        {
            self.bubble(crate::ui_events::UiEvent::PopupProtection {
                generation: if protected(after.phase) {
                    after.generation
                } else {
                    before.generation
                },
                active: protected(after.phase),
            });
        }
        self.view
            .as_ref()
            .expect("active capture projection")
            .send_replace(after);
        self.state = state;
    }

    fn start_selection(&mut self, kind: CaptureKind) {
        self.generation += 1;
        self.open_selection(kind);
    }

    fn open_selection(&mut self, kind: CaptureKind) {
        self.preview_state = PreviewState::Loading;
        self.popup_snapshot = None;
        self.draft.clear();
        self.edit_revision = 0;
        self.send_error = None;
        self.effects.push(UiEffect::Activation(
            crate::activation_policy::ActivationInput::Open(
                crate::activation_policy::SelectionOpen {
                    kind,
                    source: self.start_source,
                    generation: self.generation,
                },
            ),
        ));
        self.set_state(CaptureState::Selecting {
            kind,
            generation: self.generation,
            operation: SelectionOperation::Open,
        });
    }

    fn stop_selection(&mut self) {
        self.close_selection(None);
    }

    fn close_selection(&mut self, reopen: Option<CaptureKind>) {
        if let CaptureState::Selecting { kind, .. } = self.state {
            let kind = reopen.unwrap_or(kind);
            self.generation += 1;
            self.set_state(CaptureState::Selecting {
                kind,
                generation: self.generation,
                operation: if reopen.is_some() {
                    SelectionOperation::Reopen
                } else {
                    SelectionOperation::Close
                },
            });
            self.effects.push(UiEffect::Activation(
                crate::activation_policy::ActivationInput::Supersede(self.generation),
            ));
            self.run(
                CaptureEffect::CloseSelection {
                    generation: self.generation,
                    shutdown: self.closing,
                },
                CaptureTask::Selection(self.generation),
            );
        }
    }

    fn hide(&mut self, kind: CaptureKind, generation: u64, content: Arc<ReadyCapture>, sent: bool) {
        self.close_popup(kind, generation, content, sent, None);
    }

    fn close_popup(
        &mut self,
        kind: CaptureKind,
        generation: u64,
        content: Arc<ReadyCapture>,
        sent: bool,
        reply: Option<CloseReply>,
    ) {
        self.state = CaptureState::PopupClosing {
            kind,
            generation,
            content: content.clone(),
        };
        self.presentation_transition(UiView::CapturePopup, PresentationEvent::Hide);
        self.effects.push(UiEffect::Activation(
            crate::activation_policy::ActivationInput::HidePopup,
        ));
        self.effects.push(UiEffect::CaptureView {
            effects: vec![CaptureEffect::HidePopup {
                content: content.clone(),
                sent,
            }],
            completion: CaptureOutput::Hidden {
                kind,
                generation,
                content,
                sent,
                reply,
                open_main: sent && !self.closing,
            },
        });
    }

    fn applied(&mut self, applied: CaptureApplied) {
        let CaptureApplied { completion, result } = applied;
        match completion {
            CaptureOutput::PopupFramePrepared {
                generation,
                content,
            } => {
                self.effects.push(UiEffect::Activation(
                    crate::activation_policy::ActivationInput::PopupFramePrepared {
                        generation,
                        content,
                        result,
                    },
                ));
            }
            CaptureOutput::Reported => {
                if let Err(error) = result {
                    self.log(&format!("capture: output failed error={error}"));
                }
            }
            CaptureOutput::Presented { view, generation } => {
                if generation != self.generation {
                    return;
                }
                if let Err(error) = result {
                    if view == UiView::CapturePopup {
                        if let CaptureState::Popup { kind, content, .. } = &self.state {
                            self.hide(*kind, generation, content.clone(), false);
                        }
                    } else {
                        self.presentation_transition(view, PresentationEvent::Hide);
                    }
                    self.log(&format!(
                        "capture: presentation failed view={view:?} error={error}"
                    ));
                }
            }
            CaptureOutput::Hidden {
                kind,
                generation,
                content,
                sent,
                reply,
                open_main,
            } => {
                if generation != self.generation {
                    return;
                }
                if !matches!(self.state, CaptureState::PopupClosing { generation: pending, .. } if pending == generation)
                {
                    self.log(&format!("capture: popup hidden completion generation={generation} ignored=true reason=not-closing"));
                    return;
                }
                match &result {
                    Ok(()) => {
                        self.popup_snapshot = None;
                        self.draft.clear();
                        self.send_error = None;
                        self.set_state(CaptureState::Idle);
                        self.log(&format!(
                            "capture: popup hidden generation={generation} -> Idle"
                        ));
                        self.bubble(UiEvent::OperationEnded { kind, generation });
                        let restart = self.popup_restart.take();
                        self.effects.push(UiEffect::Activation(
                            crate::activation_policy::ActivationInput::PopupClosed {
                                generation,
                                origin: content.origin.clone(),
                                sent,
                                restarting: restart.is_some(),
                            },
                        ));
                        if let Some(kind) = restart.filter(|_| !self.closing) {
                            self.start_selection(kind);
                            self.bubble(UiEvent::OperationStarted {
                                kind,
                                generation: self.generation,
                            });
                        }
                        if open_main {
                            self.bubble(UiEvent::OpenMain);
                        }
                    }
                    Err(error) => {
                        self.popup_restart = None;
                        self.send_error = Some(error.clone());
                        self.set_state(CaptureState::PopupCloseFailed {
                            kind,
                            generation,
                            content,
                            sent,
                        });
                        self.effects.push(UiEffect::Activation(
                            crate::activation_policy::ActivationInput::PopupHideFailed(generation),
                        ));
                        self.output(CaptureEffect::ArmPopup(generation));
                        self.render_input();
                        self.output(CaptureEffect::Error(error.clone()));
                    }
                }
                if let Some(CloseReply {
                    reply,
                    sent: accepted,
                }) = reply
                {
                    let response = match (accepted, result) {
                        (Ok(_), Err(error)) => Err(format!(
                            "送信は受理されましたが、ポップアップの終了に失敗しました: {error}"
                        )),
                        (accepted, _) => accepted,
                    };
                    let _ = reply.send(response);
                }
            }
        }
    }

    pub(crate) fn handle(&mut self, event: UiEvent) -> Handling {
        if self.view.is_none() {
            return Handling::Handled(vec![UiEffect::Fail("取得の管理者は終了しました".into())]);
        }
        let handling = self.handle_event(CaptureEvent::Ui(event));
        if self.closing
            && matches!(self.state, CaptureState::Idle)
            && self.voice_control.is_none()
            && self.voice_send.is_none()
        {
            self.view.take();
        }
        match handling {
            Handling::Handled(_) => Handling::Handled(std::mem::take(&mut self.effects)),
            Handling::Bubble(event) if self.effects.is_empty() => Handling::Bubble(event),
            Handling::Bubble(event) => {
                self.bubble(event);
                Handling::Handled(std::mem::take(&mut self.effects))
            }
        }
    }

    // 状態とイベントの遷移表。状態の変更と副作用の開始はここだけで行う。
    fn transition(&mut self, event: CaptureEvent) {
        if matches!(
            self.state,
            CaptureState::Popup { .. } | CaptureState::PopupCloseFailed { .. }
        ) {
            if let CaptureEvent::Voice { reply, .. } = event {
                let _ = reply.send(Err("未送信のポップアップがあります".into()));
                return;
            }
        }
        let from = self.state.view().phase;
        let previous = self.state.view();
        let label = event.label();
        let current = std::mem::replace(&mut self.state, CaptureState::Idle);
        let mut ignored = false;
        match (current, event) {
            (
                current @ CaptureState::Voice {
                    speech_generation: None,
                    ..
                },
                CaptureEvent::VoiceProgress(snapshot),
            ) => {
                self.set_state(current);
                self.voice_snapshot = Some(snapshot);
            }
            (
                CaptureState::Voice {
                    generation,
                    speech_generation: Some(expected),
                    ..
                },
                CaptureEvent::VoiceProgress(snapshot),
            ) if expected == snapshot.speech.generation => {
                let phase = match snapshot.speech.phase.as_str() {
                    "idle" => CapturePhase::Idle,
                    "confirming" => CapturePhase::Popup,
                    "sending" => CapturePhase::Sending,
                    _ => CapturePhase::Selecting,
                };
                if phase == CapturePhase::Popup && self.voice_draft_generation != Some(expected) {
                    self.draft = snapshot.speech.partial.clone();
                    self.voice_draft_generation = Some(expected);
                    if snapshot.speech.source.as_deref() == Some("shortcut")
                        && self.voice_control.as_ref().is_none_or(|control| {
                            control.action != crate::ui_events::VoiceAction::Cancel
                        })
                        && !self.closing
                        && self.voice_interrupts.is_empty()
                    {
                        self.voice_focus = true;
                        self.present(UiView::SpeechPopup, PresentationEvent::Open(()));
                    }
                }
                self.voice_snapshot = Some(snapshot);
                if phase == CapturePhase::Idle && self.voice_control.is_none() {
                    self.present(UiView::SpeechPopup, PresentationEvent::Hide);
                    self.set_state(CaptureState::Idle);
                } else {
                    self.set_state(CaptureState::Voice {
                        generation,
                        speech_generation: Some(expected),
                        phase,
                    });
                    self.present_voice();
                }
            }
            (
                current @ CaptureState::Popup { generation, .. },
                CaptureEvent::PopupEdit {
                    generation: edited,
                    edit_revision,
                    message,
                },
            ) if generation == edited && edit_revision >= self.edit_revision => {
                self.set_state(current);
                self.draft = message;
                self.edit_revision = edit_revision;
                self.send_error = None;
                self.render_input();
            }
            (
                current @ CaptureState::Voice { generation, .. },
                CaptureEvent::Voice {
                    action: crate::ui_events::VoiceAction::Start(_),
                    reply,
                },
            ) => {
                self.set_state(current);
                self.voice_focus = true;
                self.present(UiView::SpeechPopup, PresentationEvent::Open(()));
                self.present_voice();
                let result = Ok(generation);
                let _ = reply.send(result);
            }
            (
                CaptureState::Idle,
                CaptureEvent::Voice {
                    action: action @ crate::ui_events::VoiceAction::Start(source),
                    reply,
                },
            ) => {
                self.generation += 1;
                self.draft.clear();
                self.send_error = None;
                self.voice_draft_generation = None;
                self.edit_revision = 0;
                self.voice_focus = false;
                if source == crate::speech::SpeechSource::Shortcut {
                    self.present(UiView::SpeechPopup, PresentationEvent::Open(()));
                }
                self.set_state(CaptureState::Voice {
                    generation: self.generation,
                    speech_generation: None,
                    phase: CapturePhase::Selecting,
                });
                self.start_voice_control(action, VoiceControlReply::User(reply));
            }
            (current @ CaptureState::Voice { .. }, CaptureEvent::Voice { action, reply })
                if self.voice_control.is_none() =>
            {
                self.set_state(current);
                self.start_voice_control(action, VoiceControlReply::User(reply));
            }
            (current, CaptureEvent::Voice { action, reply }) => {
                self.set_state(current);
                ignored = true;
                let result = if matches!(action, crate::ui_events::VoiceAction::Start(_)) {
                    Err("別の入力を終了してから音声入力を開始してください".to_owned())
                } else {
                    Ok(self.generation)
                };
                let _ = reply.send(result);
            }
            (current, CaptureEvent::VoiceControlFinished { generation, result })
                if self
                    .voice_control
                    .as_ref()
                    .is_some_and(|control| control.generation == generation) =>
            {
                self.set_state(current);
                self.finish_voice_control(result);
            }
            (
                current @ CaptureState::Voice { .. },
                CaptureEvent::Interrupt {
                    selection_only: true,
                    reply,
                },
            ) => {
                self.set_state(current);
                let _ = reply.send(Ok(()));
            }
            (
                current @ CaptureState::Voice { .. },
                CaptureEvent::Interrupt {
                    selection_only: false,
                    reply,
                },
            ) if matches!(
                current.view().phase,
                CapturePhase::Popup | CapturePhase::Sending
            ) || self.voice_send.is_some() =>
            {
                self.set_state(current);
                let _ = reply.send(Err(
                    "音声入力の送信または明示取消を先に完了してください".into()
                ));
            }
            (current @ CaptureState::Voice { .. }, CaptureEvent::Interrupt { reply, .. }) => {
                self.set_state(current);
                self.voice_interrupts.push(reply);
                if let Some(control) = &self.voice_control {
                    if !matches!(control.action, crate::ui_events::VoiceAction::Cancel) {
                        control.cancellation.cancel();
                    }
                } else {
                    self.start_voice_control(
                        crate::ui_events::VoiceAction::Cancel,
                        VoiceControlReply::Internal,
                    );
                }
            }
            (
                current @ CaptureState::Sending { .. },
                CaptureEvent::Interrupt {
                    selection_only: false,
                    reply,
                },
            ) => {
                self.set_state(current);
                let _ = reply.send(Err("送信の完了後に切り替えてください".into()));
            }
            (CaptureState::Idle, CaptureEvent::Shortcut(kind)) => self.start_selection(kind),
            (
                current @ CaptureState::Selecting {
                    kind: CaptureKind::Image,
                    ..
                },
                CaptureEvent::Shortcut(CaptureKind::Image),
            ) => {
                let generation = current.view().generation;
                self.set_state(current);
                self.log(&format!("範囲選択: 再押下を無視 generation={generation}"));
                ignored = true;
            }
            (current @ CaptureState::Selecting { .. }, CaptureEvent::Shortcut(kind)) => {
                self.set_state(current);
                self.close_selection(Some(kind));
            }
            (
                CaptureState::Popup {
                    kind,
                    generation,
                    content,
                }
                | CaptureState::PopupCloseFailed {
                    kind,
                    generation,
                    content,
                    sent: false,
                },
                CaptureEvent::Shortcut(requested),
            ) if kind == requested => {
                self.popup_restart = Some(kind);
                self.hide(kind, generation, content, false);
            }
            (
                current @ CaptureState::PopupClosing { kind, .. },
                CaptureEvent::Shortcut(requested),
            ) if kind == requested && self.popup_restart == Some(kind) => {
                self.set_state(current);
                self.log("capture: restart already pending");
            }
            (
                CaptureState::Selecting {
                    kind,
                    generation,
                    operation,
                },
                CaptureEvent::Selection {
                    generation: event_generation,
                    event,
                },
            ) if generation == event_generation => {
                let result = match event {
                    SelectionEvent::Closed => Ok(None),
                    SelectionEvent::Captured(content) => Ok(Some(content)),
                    SelectionEvent::Failed(error) => Err(error),
                    SelectionEvent::Opened => unreachable!("opened handled above"),
                };
                let cancelled = !matches!(operation, SelectionOperation::Open);
                if cancelled {
                    let completion = Ok(());
                    self.set_state(CaptureState::Idle);
                    for reply in self.selection_interrupts.drain(..) {
                        let _ = reply.send(completion.clone());
                    }
                    if let Err(error) = &result {
                        self.output(CaptureEffect::Error(error.clone()));
                    } else if matches!(operation, SelectionOperation::Reopen) && !self.closing {
                        self.open_selection(kind);
                    }
                } else {
                    match result {
                        Ok(Some(content))
                            if matches!(content.attachment, super::ReadyAttachment::Text(None))
                                && !content.accessibility_permission_required =>
                        {
                            self.set_state(CaptureState::Idle);
                            self.bubble(crate::ui_events::UiEvent::EmptyClipboard);
                        }
                        Ok(Some(content)) => {
                            self.set_state(CaptureState::Popup {
                                kind,
                                generation,
                                content: content.clone(),
                            });
                            self.show(generation, content);
                        }
                        Ok(None) => self.set_state(CaptureState::Idle),
                        Err(error) => {
                            self.set_state(CaptureState::Idle);
                            self.output(CaptureEffect::Error(error));
                        }
                    }
                }
            }
            (
                CaptureState::Popup {
                    kind,
                    generation,
                    content,
                },
                CaptureEvent::PopupCancel {
                    generation: event_generation,
                    source,
                },
            ) if generation == event_generation && source.should_cancel() => {
                self.hide(kind, generation, content, false);
            }
            (
                CaptureState::PopupCloseFailed {
                    kind,
                    generation,
                    content,
                    sent,
                },
                CaptureEvent::PopupCancel {
                    generation: target,
                    source,
                },
            ) if generation == target && source.should_cancel() => {
                self.hide(kind, generation, content, sent);
            }
            (
                CaptureState::Popup {
                    kind,
                    generation,
                    content,
                },
                CaptureEvent::PopupSend {
                    capture_id,
                    message,
                    reply,
                },
            ) if content.id == capture_id && self.popup_snapshot.is_some() => {
                self.draft = message.clone();
                self.send_error = None;
                self.run(
                    CaptureEffect::Send {
                        content: content.clone(),
                        message,
                    },
                    CaptureTask::Send(generation),
                );
                self.set_state(CaptureState::Sending {
                    kind,
                    generation,
                    content,
                    operation: SendOperation { reply },
                });
                self.render_input();
            }
            (
                CaptureState::Sending {
                    kind,
                    generation,
                    content,
                    operation,
                },
                CaptureEvent::SendFinished {
                    generation: event_generation,
                    result,
                },
            ) if generation == event_generation => {
                if result.is_ok() || self.closing {
                    self.close_popup(
                        kind,
                        generation,
                        content,
                        true,
                        Some(CloseReply {
                            reply: operation.reply,
                            sent: result,
                        }),
                    );
                } else {
                    self.send_error = result.as_ref().err().cloned();
                    self.set_state(CaptureState::Popup {
                        kind,
                        generation,
                        content,
                    });
                    self.render_input();
                    let _ = operation.reply.send(result);
                    self.bubble(UiEvent::OpenMain);
                }
            }
            (current @ CaptureState::Selecting { .. }, CaptureEvent::Interrupt { reply, .. }) => {
                self.set_state(current);
                self.popup_restart = None;
                self.selection_interrupts.push(reply);
                self.stop_selection();
            }
            (
                current @ (CaptureState::Popup { .. }
                | CaptureState::PopupClosing { .. }
                | CaptureState::PopupCloseFailed { .. }),
                CaptureEvent::Interrupt {
                    selection_only: false,
                    reply,
                },
            ) => {
                self.set_state(current);
                let _ = reply.send(Err(
                    "送信ポップアップを送信するか、明示的に閉じてから切り替えてください".into(),
                ));
            }
            (current, CaptureEvent::Interrupt { reply, .. }) => {
                let result = if matches!(current, CaptureState::Sending { .. }) {
                    Err("範囲選択を送信中です".to_owned())
                } else {
                    Ok(())
                };
                self.set_state(current);
                ignored = true;
                let _ = reply.send(result);
            }
            (current, CaptureEvent::Shutdown) => {
                self.closing = true;
                match current {
                    current @ CaptureState::Selecting { .. } => {
                        self.set_state(current);
                        self.popup_restart = None;
                        self.stop_selection();
                    }
                    CaptureState::Popup {
                        kind,
                        generation,
                        content,
                    } => {
                        self.hide(kind, generation, content, false);
                    }
                    CaptureState::PopupCloseFailed {
                        kind,
                        generation,
                        content,
                        sent,
                    } => {
                        self.hide(kind, generation, content, sent);
                    }
                    current
                    @ (CaptureState::Sending { .. } | CaptureState::PopupClosing { .. }) => {
                        self.set_state(current);
                    }
                    CaptureState::Idle => {}
                    current @ CaptureState::Voice { .. } => {
                        self.set_state(current);
                        if let Some(control) = &self.voice_control {
                            if !matches!(control.action, crate::ui_events::VoiceAction::Cancel) {
                                control.cancellation.cancel();
                            }
                        } else {
                            self.start_voice_control(
                                crate::ui_events::VoiceAction::Cancel,
                                VoiceControlReply::Internal,
                            );
                        }
                    }
                }
                self.view
                    .as_ref()
                    .expect("active capture projection")
                    .send_replace(self.state.view());
            }
            (current, event) => {
                if let CaptureEvent::PopupSend { reply, .. } = event {
                    let _ = reply.send(Err("送信する範囲選択が一致しません".to_owned()));
                }
                self.set_state(current);
                ignored = true;
            }
        }
        self.log(&format!(
            "capture: state={from:?} event={label} -> {:?}{}",
            self.state.view().phase,
            if ignored { " ignored=true" } else { "" },
        ));
        let current = self.state.view();
        if let Some(kind) = previous.kind {
            if matches!(self.state, CaptureState::Idle) || current.generation != previous.generation
            {
                self.bubble(crate::ui_events::UiEvent::OperationEnded {
                    kind,
                    generation: previous.generation,
                });
            }
        }
        if let Some(kind) = current.kind {
            if previous.kind.is_none() || current.generation != previous.generation {
                self.bubble(crate::ui_events::UiEvent::OperationStarted {
                    kind,
                    generation: current.generation,
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "capture_manager_tests.rs"]
pub(crate) mod tests;

impl Default for CapturePresenter {
    fn default() -> Self {
        let (view, _) = watch::channel(CaptureState::Idle.view());
        Self::new(view)
    }
}
