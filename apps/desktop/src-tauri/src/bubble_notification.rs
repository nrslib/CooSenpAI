use super::{register_replacing_for_surface, BubblePresenter};
use crate::bubbles::{BubblePresentation, BubbleRecord};
use crate::ui_events::UiEffect;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug)]
pub(super) enum NotificationPlan {
    Complete(bool),
    Unread,
    Bubble(BubblePresentation),
    Os(coosenpai_core::notification::NotificationRecord),
}

#[derive(Debug)]
pub(crate) struct NotificationContext {
    pub config: coosenpai_core::config::Config,
    pub display_name: String,
    pub input_active: bool,
    pub tutorial_active: bool,
    pub latest_thought: Option<String>,
    pub latest_thought_generation: Option<u64>,
}

impl BubblePresenter {
    pub(super) async fn notification(
        &mut self,
        record: coosenpai_core::notification::NotificationRecord,
        target: crate::state::NotificationTarget,
        context: NotificationContext,
        main_focused: bool,
        main_visible: bool,
    ) -> (NotificationPlan, Vec<UiEffect>) {
        use crate::state::{signed_build, NotificationTarget};
        if record.conversation_generation != self.model.lock().await.conversation_generation() {
            return (NotificationPlan::Complete(false), Vec::new());
        }
        let config = context.config;
        let mut effects = Vec::new();
        let plan = match target {
            NotificationTarget::Bubble
                if record.message_kind == "chat"
                    || matches!(config.notification.mode.as_str(), "bubble" | "both")
                    || !signed_build() =>
            {
                let input_active = context.input_active;
                let decision =
                    bubble_delivery_decision(&record.message_kind, input_active, main_focused);
                if self.delivery_log.should_log(
                    &record.id,
                    BubbleDeliveryLogKey {
                        decision,
                        main_focused,
                        input_active,
                    },
                ) {
                    effects.push(UiEffect::Log(bubble_delivery_log(
                        &record.message_kind,
                        decision,
                        main_focused,
                        input_active,
                    )));
                }
                match decision {
                    BubbleDeliveryDecision::SuppressUnread => NotificationPlan::Unread,
                    BubbleDeliveryDecision::SuppressRead => NotificationPlan::Complete(true),
                    BubbleDeliveryDecision::Show => {
                        let style = bubble_presentation_style(
                            &record.message_kind,
                            context.tutorial_active,
                            config.bubble.keep_latest,
                        );
                        let bubble = BubbleRecord {
                            id: record.id,
                            created_at: record.created_at,
                            message: record.message,
                            message_kind: if style.tutorial {
                                "tutorial".into()
                            } else {
                                record.message_kind
                            },
                            notification_priority: record.priority,
                            caused_by: record.caused_by,
                            display_name: context.display_name,
                            persona: config.companion.persona.clone(),
                            avatar_color: config.ui.avatar_color.clone(),
                            conversation_generation: record.conversation_generation,
                            persistent: style.persistent,
                            interaction: None,
                        };
                        match register_replacing_for_surface(
                            &self.model,
                            main_focused,
                            bubble,
                            Duration::from_millis(config.notification.bubble_duration_ms),
                            config.bubble.max_stack,
                            &[],
                        )
                        .await
                        {
                            Ok(presentation) => NotificationPlan::Bubble(presentation),
                            Err(_) => NotificationPlan::Complete(false),
                        }
                    }
                }
            }
            NotificationTarget::Os if main_visible || record.message_kind == "chat" => {
                NotificationPlan::Complete(true)
            }
            NotificationTarget::Os
                if signed_build() && matches!(config.notification.mode.as_str(), "os" | "both") =>
            {
                NotificationPlan::Os(record)
            }
            _ => NotificationPlan::Complete(true),
        };
        (plan, effects)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BubbleDeliveryDecision {
    Show,
    SuppressRead,
    SuppressUnread,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BubbleDeliveryLogKey {
    pub(super) decision: BubbleDeliveryDecision,
    pub(super) main_focused: bool,
    pub(super) input_active: bool,
}

#[derive(Default)]
pub(super) struct BubbleDeliveryLogState {
    last_by_notification: HashMap<String, BubbleDeliveryLogKey>,
}

impl BubbleDeliveryLogState {
    pub(super) fn should_log(&mut self, notification_id: &str, key: BubbleDeliveryLogKey) -> bool {
        if self.last_by_notification.get(notification_id).copied() == Some(key) {
            return false;
        }
        self.last_by_notification
            .insert(notification_id.to_owned(), key);
        true
    }

    pub(super) fn clear(&mut self, notification_id: &str) {
        self.last_by_notification.remove(notification_id);
    }
}

impl BubbleDeliveryDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Show => "show",
            Self::SuppressRead => "suppress-read",
            Self::SuppressUnread => "suppress-unread",
        }
    }
}

pub(super) fn bubble_delivery_log(
    message_kind: &str,
    decision: BubbleDeliveryDecision,
    main_focused: bool,
    input_active: bool,
) -> String {
    format!(
        "吹き出し配達判定: kind={message_kind} decision={} main-focused={main_focused} input-active={input_active}",
        decision.as_str()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BubblePresentationStyle {
    pub(super) tutorial: bool,
    pub(super) persistent: bool,
}

pub(super) fn bubble_presentation_style(
    message_kind: &str,
    tutorial_active: bool,
    keep_latest: bool,
) -> BubblePresentationStyle {
    if tutorial_active && message_kind == "chat" {
        BubblePresentationStyle {
            tutorial: true,
            persistent: true,
        }
    } else {
        BubblePresentationStyle {
            tutorial: false,
            persistent: keep_latest,
        }
    }
}

pub(super) fn bubble_delivery_decision(
    message_kind: &str,
    input_active: bool,
    main_focused: bool,
) -> BubbleDeliveryDecision {
    if message_kind != "chat" && input_active {
        BubbleDeliveryDecision::SuppressUnread
    } else if main_focused {
        BubbleDeliveryDecision::SuppressRead
    } else {
        BubbleDeliveryDecision::Show
    }
}

pub(super) fn should_show_thought_bubble(
    enabled: bool,
    input_active: bool,
    main_focused: bool,
) -> bool {
    enabled && !input_active && !main_focused
}

