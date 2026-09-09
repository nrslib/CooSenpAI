use crate::command_guard::CommandSource;
use crate::commands::IpcResult;
use crate::snapshot::AppSnapshot;
use crate::ui_events::{Handling, PresenterId, UiEffect, UiEvent, UiTask};
use coosenpai_core::config::Config;
use tokio::sync::oneshot;

pub(crate) type Reply<T> = oneshot::Sender<IpcResult<T>>;

#[derive(Debug)]
pub(crate) enum UserCommand {
    CaptureSnapshot(Reply<crate::commands_capture::CapturePopupIpcSnapshot>),
    SpeechSnapshot(Reply<crate::speech::SpeechPopupSnapshot>),
    WatchStart {
        source: CommandSource,
        reply: Reply<AppSnapshot>,
    },
    WatchStop {
        source: CommandSource,
        reply: Reply<AppSnapshot>,
    },
    EmotionsReset {
        source: CommandSource,
        reply: Reply<AppSnapshot>,
    },
    ChatCancel(Reply<String>),
    ChatRetry(Reply<String>),
    TutorialNext(Reply<AppSnapshot>),
    TutorialSettingsPresented(Reply<AppSnapshot>),
    TutorialFinish(Reply<AppSnapshot>),
    TutorialRestart(Reply<AppSnapshot>),
    SetupRestart(Reply<AppSnapshot>),
    SetupPrompt(Reply<AppSnapshot>),
    ConversationReset(Reply<AppSnapshot>),
    BubbleInteract {
        id: String,
        action: String,
        value: Option<String>,
        reply: Reply<()>,
    },
    BubbleDismiss {
        id: String,
        reply: Reply<()>,
    },
    BubblePreview {
        preview: Option<crate::bubbles::BubbleAppearancePreview>,
        reply: Reply<()>,
    },
    BubbleFastForward {
        id: Option<String>,
        reply: Reply<bool>,
    },
    BubbleNavigate {
        direction: crate::bubbles::BubbleDeckDirection,
        reply: Reply<bool>,
    },
    ModelSave {
        patch: serde_json::Value,
        reply: Reply<Config>,
    },
    SystemSettings {
        pane: coosenpai_core::ports::SystemSettingsPane,
        failure: coosenpai_core::locale::TextKey,
        reply: Reply<()>,
    },
}

impl UserCommand {
    pub(crate) fn owner(&self) -> PresenterId {
        match self {
            Self::CaptureSnapshot(_) | Self::SpeechSnapshot(_) => PresenterId::Capture,
            Self::TutorialNext(_)
            | Self::TutorialSettingsPresented(_)
            | Self::TutorialFinish(_)
            | Self::TutorialRestart(_)
            | Self::SetupRestart(_)
            | Self::SetupPrompt(_)
            | Self::BubbleInteract { .. } => PresenterId::Tutorial,
            Self::BubbleDismiss { .. }
            | Self::BubblePreview { .. }
            | Self::BubbleFastForward { .. }
            | Self::BubbleNavigate { .. } => PresenterId::Bubble,
            Self::ModelSave { .. } => PresenterId::ModelPicker,
            Self::SystemSettings { .. } => PresenterId::Settings,
            _ => PresenterId::Chat,
        }
    }
}

#[derive(Debug)]
pub(crate) enum CommandCompletion {
    CaptureSnapshot {
        result: Box<IpcResult<crate::commands_capture::CapturePopupIpcSnapshot>>,
        reply: Reply<crate::commands_capture::CapturePopupIpcSnapshot>,
    },
    SpeechSnapshot {
        result: Box<IpcResult<crate::speech::SpeechPopupSnapshot>>,
        reply: Reply<crate::speech::SpeechPopupSnapshot>,
    },
    Snapshot {
        result: Box<IpcResult<AppSnapshot>>,
        reply: Reply<AppSnapshot>,
    },
    Text {
        result: IpcResult<String>,
        reply: Reply<String>,
    },
    Unit {
        result: IpcResult<()>,
        reply: Reply<()>,
    },
    Bool {
        result: IpcResult<bool>,
        reply: Reply<bool>,
    },
}
impl CommandCompletion {
    pub(crate) fn reply(self) {
        match self {
            Self::Snapshot { result, reply } => {
                let _ = reply.send(*result);
            }
            Self::CaptureSnapshot { result, reply } => {
                let _ = reply.send(*result);
            }
            Self::SpeechSnapshot { result, reply } => {
                let _ = reply.send(*result);
            }
            Self::Text { result, reply } => {
                let _ = reply.send(result);
            }
            Self::Unit { result, reply } => {
                let _ = reply.send(result);
            }
            Self::Bool { result, reply } => {
                let _ = reply.send(result);
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct TutorialPresenter {
    progress: crate::tutorial_progress_presenter::TutorialProgressPresenter,
    cards: crate::tutorial_card_presenter::TutorialCardPresenter,
    sequence: crate::tutorial_sequence_presenter::TutorialSequencePresenter,
}
impl TutorialPresenter {
    pub(crate) fn handle(&mut self, event: UiEvent, main_focused: bool) -> Handling {
        match event {
            UiEvent::Tutorial(event) => Handling::Handled(match *event {
                crate::tutorial_events::TutorialEvent::Lifecycle(event) => {
                    crate::tutorial_lifecycle_presenter::handle(event)
                }
                crate::tutorial_events::TutorialEvent::Response(event) => {
                    crate::tutorial_response_presenter::handle(event, main_focused)
                }
                crate::tutorial_events::TutorialEvent::Progress(event) => {
                    self.progress.handle(event)
                }
                crate::tutorial_events::TutorialEvent::Notice(event) => {
                    crate::tutorial_notice_presenter::handle(event)
                }
                crate::tutorial_events::TutorialEvent::Card(event) => {
                    self.cards.handle(event, main_focused)
                }
                crate::tutorial_events::TutorialEvent::Sequence(event) => {
                    self.sequence.handle(event)
                }
            }),
            UiEvent::UserCommand(command) if command.owner() == PresenterId::Tutorial => {
                Handling::Handled(vec![UiEffect::Spawn(UiTask::UserCommand(command))])
            }
            UiEvent::CommandFinished {
                owner: PresenterId::Tutorial,
                completion,
            } => {
                completion.reply();
                Handling::Handled(Vec::new())
            }
            event => Handling::Bubble(event),
        }
    }
}
