use crate::bubbles::{reading_delay, BubbleMilestone};
use crate::tutorial_events::TutorialTask;
use crate::tutorial_notice::TutorialBubbleOutcome;
use crate::ui_events::{UiEffect, UiTask};
use coosenpai_core::runtime::RuntimeError;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

type SequenceReply = oneshot::Sender<Result<TutorialBubbleOutcome, RuntimeError>>;

#[derive(Debug, Clone, Copy)]
pub(crate) enum SequenceKind {
    Intro { follows_setup_ok: bool },
    Watch,
}

#[derive(Debug)]
pub(crate) struct SequenceCard {
    pub outcome: Result<TutorialBubbleOutcome, RuntimeError>,
    pub details: Result<(String, String), RuntimeError>,
    pub already_accepted: bool,
    pub has_completion: bool,
}

#[derive(Debug)]
pub(crate) enum SequenceEvent {
    Start {
        kind: SequenceKind,
        reply: SequenceReply,
    },
    Cancel(oneshot::Sender<()>),
    Presented {
        generation: u64,
        card: SequenceCard,
    },
    Reached {
        generation: u64,
        reached: bool,
    },
}

#[derive(Debug)]
pub(crate) enum SequenceTask {
    Present {
        generation: u64,
        key: &'static str,
        milestone: BubbleMilestone,
        cancellation: CancellationToken,
    },
    Wait {
        generation: u64,
        notice_id: String,
        milestone: BubbleMilestone,
        delay: std::time::Duration,
        cancellation: CancellationToken,
    },
}

struct Sequence {
    keys: &'static [&'static str],
    index: usize,
    cancellation: CancellationToken,
    reply: SequenceReply,
}

#[derive(Default)]
pub(crate) struct TutorialSequencePresenter {
    generation: u64,
    sequence: Option<Sequence>,
}

impl TutorialSequencePresenter {
    pub(crate) fn handle(&mut self, event: SequenceEvent) -> Vec<UiEffect> {
        match event {
            SequenceEvent::Start { kind, reply } => {
                self.finish(Ok(TutorialBubbleOutcome::Dismissed));
                self.generation = self.generation.saturating_add(1);
                let keys: &'static [&'static str] = match kind {
                    SequenceKind::Intro {
                        follows_setup_ok: true,
                    } => &["setup-ok", "intro", "intro-click"],
                    SequenceKind::Intro {
                        follows_setup_ok: false,
                    } => &["intro", "intro-click"],
                    SequenceKind::Watch => &["after-persona", "watch-intro"],
                };
                self.sequence = Some(Sequence {
                    keys,
                    index: 0,
                    cancellation: CancellationToken::new(),
                    reply,
                });
                self.present()
            }
            SequenceEvent::Cancel(reply) => {
                self.finish(Ok(TutorialBubbleOutcome::Dismissed));
                let _ = reply.send(());
                Vec::new()
            }
            SequenceEvent::Presented { generation, card } => {
                let Some(sequence) = &self.sequence else {
                    return Vec::new();
                };
                if generation != self.generation {
                    return Vec::new();
                }
                match card.outcome {
                    Err(error) => {
                        self.finish(Err(error));
                        return Vec::new();
                    }
                    Ok(TutorialBubbleOutcome::Dismissed) => {
                        self.finish(Ok(TutorialBubbleOutcome::Dismissed));
                        return Vec::new();
                    }
                    Ok(TutorialBubbleOutcome::Acknowledged) => {}
                }
                let (notice_id, message) = match card.details {
                    Ok(details) => details,
                    Err(error) => {
                        self.finish(Err(error));
                        return Vec::new();
                    }
                };
                if card.already_accepted && !card.has_completion {
                    return self.advance();
                }
                let final_card = sequence.index + 1 == sequence.keys.len();
                vec![task(SequenceTask::Wait {
                    generation,
                    notice_id,
                    milestone: if final_card {
                        BubbleMilestone::Activated
                    } else {
                        BubbleMilestone::Read
                    },
                    delay: if final_card {
                        std::time::Duration::ZERO
                    } else {
                        reading_delay(&message)
                    },
                    cancellation: sequence.cancellation.clone(),
                })]
            }
            SequenceEvent::Reached {
                generation,
                reached,
            } => {
                if self.sequence.is_none() || generation != self.generation {
                    return Vec::new();
                }
                if reached {
                    self.advance()
                } else {
                    self.finish(Ok(TutorialBubbleOutcome::Dismissed));
                    Vec::new()
                }
            }
        }
    }

    fn present(&self) -> Vec<UiEffect> {
        let Some(sequence) = &self.sequence else {
            return Vec::new();
        };
        vec![task(SequenceTask::Present {
            generation: self.generation,
            key: sequence.keys[sequence.index],
            milestone: if sequence.index + 1 == sequence.keys.len() {
                BubbleMilestone::Activated
            } else {
                BubbleMilestone::Read
            },
            cancellation: sequence.cancellation.clone(),
        })]
    }

    fn advance(&mut self) -> Vec<UiEffect> {
        let Some(sequence) = &mut self.sequence else {
            return Vec::new();
        };
        sequence.index += 1;
        if sequence.index == sequence.keys.len() {
            self.finish(Ok(TutorialBubbleOutcome::Acknowledged));
            Vec::new()
        } else {
            self.present()
        }
    }

    fn finish(&mut self, result: Result<TutorialBubbleOutcome, RuntimeError>) {
        if let Some(sequence) = self.sequence.take() {
            sequence.cancellation.cancel();
            let _ = sequence.reply.send(result);
        }
    }
}

fn task(task: SequenceTask) -> UiEffect {
    UiEffect::Spawn(UiTask::Tutorial(TutorialTask::Sequence(task)))
}
