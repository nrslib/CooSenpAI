use crate::ui_events::{UiEffect, UiEvent, UiTask};
use coosenpai_core::runtime::RuntimeSnapshot;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct ThoughtPresenter {
    scheduler: ThoughtBubbleScheduler,
    previous: Option<String>,
    revision: Option<u64>,
    timer_epoch: u64,
}

impl ThoughtPresenter {
    pub(crate) fn observed(
        &mut self,
        runtime: RuntimeSnapshot,
        initial: bool,
        conversation_generation: u64,
    ) -> Vec<UiEffect> {
        if self
            .revision
            .is_some_and(|revision| runtime.revision <= revision)
        {
            return Vec::new();
        }
        self.revision = Some(runtime.revision);
        let thought = if initial {
            None
        } else {
            presentable_thought(&runtime, self.previous.as_deref(), conversation_generation)
                .map(str::to_owned)
        };
        self.previous = runtime.latest_companion_thought;
        let Some(thought) = thought else {
            return Vec::new();
        };
        let action = self
            .scheduler
            .queue(Instant::now(), conversation_generation, thought);
        self.effects(action)
    }

    pub(crate) fn expired(&mut self, epoch: u64) -> Vec<UiEffect> {
        if epoch != self.timer_epoch {
            return Vec::new();
        }
        let action = self.scheduler.flush(Instant::now());
        self.effects(action)
    }

    pub(crate) fn clear(&mut self, conversation_switch: bool) {
        if conversation_switch {
            self.timer_epoch = self.timer_epoch.saturating_add(1);
            self.scheduler.clear_for_conversation_switch();
        } else {
            self.scheduler.clear_pending();
        }
    }

    fn effects(&self, action: ThoughtBubbleAction) -> Vec<UiEffect> {
        match action {
            ThoughtBubbleAction::Show(thought) => {
                vec![UiEffect::Spawn(UiTask::ReadThoughtPresentation {
                    generation: thought.conversation_generation,
                    message: thought.message,
                })]
            }
            ThoughtBubbleAction::Wait(duration) => vec![UiEffect::Spawn(UiTask::Delay {
                duration,
                event: UiEvent::ThoughtFlushExpired(self.timer_epoch),
            })],
            ThoughtBubbleAction::None => Vec::new(),
        }
    }
}

const THOUGHT_BUBBLE_COOLDOWN: Duration = Duration::from_millis(1_500);

#[derive(Debug, Default)]
pub(crate) struct ThoughtBubbleScheduler {
    last_presented_at: Option<Instant>,
    pending: Option<PendingThought>,
    flush_scheduled: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PendingThought {
    pub(crate) conversation_generation: u64,
    pub(crate) message: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ThoughtBubbleAction {
    Show(PendingThought),
    Wait(Duration),
    None,
}

impl ThoughtBubbleScheduler {
    pub(crate) fn queue(
        &mut self,
        now: Instant,
        conversation_generation: u64,
        thought: String,
    ) -> ThoughtBubbleAction {
        self.pending = Some(PendingThought {
            conversation_generation,
            message: thought,
        });
        let Some(last_presented_at) = self.last_presented_at else {
            self.last_presented_at = Some(now);
            return ThoughtBubbleAction::Show(self.pending.take().expect("thought is queued"));
        };
        let elapsed = now.saturating_duration_since(last_presented_at);
        if elapsed >= THOUGHT_BUBBLE_COOLDOWN && !self.flush_scheduled {
            self.last_presented_at = Some(now);
            return ThoughtBubbleAction::Show(self.pending.take().expect("thought is queued"));
        }
        if self.flush_scheduled {
            ThoughtBubbleAction::None
        } else {
            self.flush_scheduled = true;
            ThoughtBubbleAction::Wait(THOUGHT_BUBBLE_COOLDOWN.saturating_sub(elapsed))
        }
    }

    pub(crate) fn flush(&mut self, now: Instant) -> ThoughtBubbleAction {
        let Some(last_presented_at) = self.last_presented_at else {
            self.flush_scheduled = false;
            return ThoughtBubbleAction::None;
        };
        let elapsed = now.saturating_duration_since(last_presented_at);
        if elapsed < THOUGHT_BUBBLE_COOLDOWN {
            return ThoughtBubbleAction::Wait(THOUGHT_BUBBLE_COOLDOWN - elapsed);
        }
        self.flush_scheduled = false;
        self.last_presented_at = Some(now);
        self.pending
            .take()
            .map_or(ThoughtBubbleAction::None, ThoughtBubbleAction::Show)
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending = None;
    }

    pub(crate) fn clear_for_conversation_switch(&mut self) {
        self.last_presented_at = None;
        self.pending = None;
        self.flush_scheduled = false;
    }
}

pub(crate) fn presentable_thought<'a>(
    runtime: &'a RuntimeSnapshot,
    previous: Option<&str>,
    current_generation: u64,
) -> Option<&'a str> {
    let thought = runtime
        .latest_companion_thought
        .as_deref()
        .filter(|value| !value.trim().is_empty())?;
    if previous == Some(thought) {
        return None;
    }
    if runtime.latest_companion_thought_generation != Some(current_generation) {
        return None;
    }
    Some(thought)
}
