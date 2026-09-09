use crate::bubbles::{BubbleAction, BubbleInteraction, BubbleRecord};
use crate::tutorial::{tutorial_step_for_guide_key, TUTORIAL_SKIP_ACTION};
use crate::tutorial_events::TutorialTask;
use crate::tutorial_notice::TutorialBubbleOutcome;
use crate::ui_events::{UiEffect, UiTask};
use coosenpai_core::config::Config;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::runtime::RuntimeError;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

type Reply = oneshot::Sender<Result<TutorialBubbleOutcome, RuntimeError>>;
#[derive(Debug)]
pub(crate) enum NoticeEvent {
    Requested {
        id: String,
        key: String,
        created_at: String,
        message: String,
        config: Box<Config>,
        conversation_generation: u64,
        require_ack: bool,
        replaced_bubble_ids: Vec<String>,
        cancellation: CancellationToken,
        reply: Reply,
    },
    Presented {
        result: Result<TutorialBubbleOutcome, RuntimeError>,
        reply: Reply,
    },
    ConversationSaved,
}
#[derive(Debug)]
pub(crate) enum NoticeTask {
    Acknowledge {
        record: Box<BubbleRecord>,
        duration_ms: u64,
        replaced_bubble_ids: Vec<String>,
        cancellation: CancellationToken,
        reply: Reply,
    },
    BestEffort {
        record: Box<BubbleRecord>,
        duration_ms: u64,
        reply: Reply,
    },
}
pub(crate) fn handle(event: NoticeEvent) -> Vec<UiEffect> {
    match event {
        NoticeEvent::Requested {
            id,
            key,
            created_at,
            message,
            config,
            conversation_generation,
            require_ack,
            replaced_bubble_ids,
            cancellation,
            reply,
        } => {
            let interaction = tutorial_skip_interaction_for_locale(
                &key,
                Locale::from_config(&config.ui.language),
            );
            let record = Box::new(BubbleRecord {
                id,
                created_at,
                message,
                message_kind: "tutorial".to_owned(),
                notification_priority: "none".to_owned(),
                caused_by: None,
                display_name: config.companion.display_name,
                persona: config.companion.persona,
                avatar_color: config.ui.avatar_color,
                conversation_generation,
                persistent: true,
                interaction,
            });
            let duration_ms = config.notification.bubble_duration_ms;
            let task = if require_ack {
                NoticeTask::Acknowledge {
                    record,
                    duration_ms,
                    replaced_bubble_ids,
                    cancellation,
                    reply,
                }
            } else {
                NoticeTask::BestEffort {
                    record,
                    duration_ms,
                    reply,
                }
            };
            vec![UiEffect::Spawn(UiTask::Tutorial(TutorialTask::Notice(
                task,
            )))]
        }
        NoticeEvent::Presented { result, reply } => {
            let _ = reply.send(result);
            Vec::new()
        }
        NoticeEvent::ConversationSaved => vec![UiEffect::Spawn(UiTask::RefreshConversation)],
    }
}

fn tutorial_skip_interaction_for_locale(key: &str, locale: Locale) -> Option<BubbleInteraction> {
    tutorial_step_for_guide_key(key).map(|_| BubbleInteraction {
        select: None,
        secret_input: None,
        actions: vec![BubbleAction {
            id: TUTORIAL_SKIP_ACTION.to_owned(),
            label: text(TextKey::TutorialSkip, locale).to_owned(),
        }],
        detail: None,
        technical_detail: None,
    })
}

