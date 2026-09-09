use crate::tutorial_events::{CardEvent, CardSignal, CardTask, TutorialEvent, TutorialTask};
use crate::ui_events::{UiEffect, UiEvent, UiTask};
use std::collections::HashMap;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

struct CardWait {
    main_delay: std::time::Duration,
    cancellation: CancellationToken,
    reply: oneshot::Sender<bool>,
}

#[derive(Default)]
pub(crate) struct TutorialCardPresenter {
    next_id: u64,
    waits: HashMap<u64, CardWait>,
}

impl TutorialCardPresenter {
    pub(crate) fn handle(&mut self, event: CardEvent, main_focused: bool) -> Vec<UiEffect> {
        match event {
            CardEvent::WaitCard {
                notice_id,
                milestone,
                main_delay,
                cancellation,
                reply,
            } => {
                if cancellation.is_cancelled() {
                    let _ = reply.send(false);
                    return Vec::new();
                }
                self.next_id = self.next_id.saturating_add(1);
                let id = self.next_id;
                self.waits.insert(
                    id,
                    CardWait {
                        main_delay,
                        cancellation: cancellation.child_token(),
                        reply,
                    },
                );
                vec![task(CardTask::ReadCard {
                    id,
                    notice_id,
                    milestone,
                })]
            }
            CardEvent::CardLoaded { id, signals } => {
                let Some(wait) = self.waits.get(&id) else {
                    return Vec::new();
                };
                if wait.cancellation.is_cancelled() {
                    self.finish(id, false);
                    return Vec::new();
                }
                if let Some((reached, dismissed)) = signals {
                    vec![task(CardTask::ObserveCard {
                        id,
                        reached,
                        dismissed,
                        cancellation: wait.cancellation.clone(),
                    })]
                } else {
                    self.fallback(id, main_focused)
                }
            }
            CardEvent::CardObserved {
                id,
                signal: CardSignal::Dismissed,
            } => self.fallback(id, main_focused),
            CardEvent::CardObserved { id, signal } => {
                let reached = matches!(signal, CardSignal::Reached);
                self.finish(id, reached);
                Vec::new()
            }
            CardEvent::CardDelayExpired(id) => {
                let active = self
                    .waits
                    .get(&id)
                    .is_some_and(|wait| !wait.cancellation.is_cancelled());
                self.finish(id, active);
                Vec::new()
            }
        }
    }

    fn fallback(&mut self, id: u64, main_focused: bool) -> Vec<UiEffect> {
        let Some(wait) = self.waits.get(&id) else {
            return Vec::new();
        };
        if !main_focused || wait.cancellation.is_cancelled() {
            self.finish(id, false);
            return Vec::new();
        }
        vec![
            UiEffect::Spawn(UiTask::Delay {
                duration: wait.main_delay,
                event: UiEvent::Tutorial(Box::new(TutorialEvent::Card(
                    CardEvent::CardDelayExpired(id),
                ))),
            }),
            task(CardTask::ObserveCancellation {
                id,
                cancellation: wait.cancellation.clone(),
            }),
        ]
    }

    fn finish(&mut self, id: u64, reached: bool) {
        if let Some(wait) = self.waits.remove(&id) {
            wait.cancellation.cancel();
            let _ = wait.reply.send(reached);
        }
    }
}

fn task(task: CardTask) -> UiEffect {
    UiEffect::Spawn(UiTask::Tutorial(TutorialTask::Card(task)))
}
