use crate::bubbles::{BubblePresentationOutcome, BubbleRecord};
use crate::state::tutorial_state::TutorialPresentation;
use crate::tutorial_events::TutorialTask;
use crate::ui_events::{UiEffect, UiTask};
use coosenpai_core::config::Config;
use coosenpai_core::state::ConversationEntry;
use tokio::sync::oneshot;

type Reply = oneshot::Sender<()>;
#[derive(Debug)]
pub(crate) enum ResponseEvent {
    Finished,
    PendingLoaded {
        entry: Option<ConversationEntry>,
        config: Box<Config>,
        conversation_generation: u64,
        reply: Reply,
    },
    Prepared {
        presentation: Option<TutorialPresentation>,
        reply: Reply,
    },
    Presented {
        completion: ResponseCompletion,
        outcome: Result<BubblePresentationOutcome, String>,
        reply: Reply,
    },
    Done(Reply),
    NotificationCompleted {
        id: String,
        message: String,
        chat: bool,
        accepted: bool,
        reply: oneshot::Sender<bool>,
    },
    RuntimeConversationLoaded,
    NotificationRefreshed {
        id: String,
        message: String,
        chat: bool,
        reply: oneshot::Sender<bool>,
    },
    NotificationRecorded(oneshot::Sender<bool>),
}
#[derive(Debug)]
pub(crate) struct ResponseCompletion {
    pub entry_id: String,
    pub message: String,
    pub conversation: crate::command_guard::GenerationStamp,
    pub bubble: crate::command_guard::GenerationStamp,
}
#[derive(Debug)]
pub(crate) enum ResponseTask {
    Main {
        entry: Box<ConversationEntry>,
        reply: Reply,
    },
    Prepare {
        record: Box<BubbleRecord>,
        duration_ms: u64,
        reply: Reply,
    },
    Complete {
        presentation: TutorialPresentation,
        reply: Reply,
    },
    Accept {
        completion: ResponseCompletion,
        reply: Reply,
    },
    Reject {
        completion: ResponseCompletion,
        reply: Reply,
    },
    NotificationAccepted {
        id: String,
        message: String,
        reply: oneshot::Sender<bool>,
    },
    RefreshNotification {
        id: String,
        message: String,
        chat: bool,
        reply: oneshot::Sender<bool>,
    },
    Poll,
}
pub(crate) fn handle(event: ResponseEvent, main_focused: bool) -> Vec<UiEffect> {
    match event {
        ResponseEvent::Finished => Vec::new(),
        ResponseEvent::PendingLoaded {
            entry,
            config,
            conversation_generation,
            reply,
        } => {
            let Some(entry) = entry else {
                let _ = reply.send(());
                return Vec::new();
            };
            if main_focused {
                return vec![task(ResponseTask::Main {
                    entry: Box::new(entry),
                    reply,
                })];
            }
            let record = Box::new(BubbleRecord {
                id: entry.id,
                created_at: entry.created_at,
                message: entry.message,
                message_kind: "tutorial".to_owned(),
                notification_priority: "none".to_owned(),
                caused_by: entry.caused_by_ids.last().cloned(),
                display_name: config.companion.display_name,
                persona: config.companion.persona,
                avatar_color: config.ui.avatar_color,
                conversation_generation,
                persistent: true,
                interaction: None,
            });
            vec![task(ResponseTask::Prepare {
                record,
                duration_ms: config.notification.bubble_duration_ms,
                reply,
            })]
        }
        ResponseEvent::Prepared {
            presentation,
            reply,
        } => match presentation {
            Some(presentation) => vec![task(ResponseTask::Complete {
                presentation,
                reply,
            })],
            None => {
                let _ = reply.send(());
                Vec::new()
            }
        },
        ResponseEvent::Presented {
            completion,
            outcome,
            reply,
        } => {
            vec![task(
                if matches!(outcome, Ok(BubblePresentationOutcome::Acknowledged)) {
                    ResponseTask::Accept { completion, reply }
                } else {
                    ResponseTask::Reject { completion, reply }
                },
            )]
        }
        ResponseEvent::Done(reply) => {
            let _ = reply.send(());
            Vec::new()
        }
        ResponseEvent::NotificationCompleted {
            id,
            message,
            chat,
            accepted,
            reply,
        } => {
            if accepted {
                vec![task(ResponseTask::RefreshNotification {
                    id,
                    message,
                    chat,
                    reply,
                })]
            } else {
                let _ = reply.send(false);
                Vec::new()
            }
        }
        ResponseEvent::NotificationRefreshed {
            id,
            message,
            chat,
            reply,
        } => {
            if chat {
                vec![task(ResponseTask::NotificationAccepted {
                    id,
                    message,
                    reply,
                })]
            } else {
                let _ = reply.send(true);
                Vec::new()
            }
        }
        ResponseEvent::NotificationRecorded(reply) => {
            let _ = reply.send(true);
            Vec::new()
        }
        ResponseEvent::RuntimeConversationLoaded => vec![task(ResponseTask::Poll)],
    }
}
fn task(value: ResponseTask) -> UiEffect {
    UiEffect::Spawn(UiTask::Tutorial(TutorialTask::Response(value)))
}
