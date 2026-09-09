use crate::bubbles::reading_delay;
use crate::tutorial_events::{TutorialEvent, TutorialTask};
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask};
use coosenpai_core::onboarding::TutorialStep;
use coosenpai_core::runtime::RuntimeError;
use std::collections::HashMap;

pub(crate) use crate::tutorial_progress_events::{
    ProgressAction, ProgressEvent, ProgressResult, ProgressTask, Reply,
};
enum Flow {
    Settings {
        reply: Reply,
        failure: Option<RuntimeError>,
    },
    Watch {
        reply: Reply,
        failure: Option<RuntimeError>,
    },
    Finish {
        step: TutorialStep,
        skipped: bool,
        next: Option<TutorialStep>,
        automatic: bool,
        reply: Reply,
    },
    Response {
        entry_id: String,
        message: String,
        step: Option<TutorialStep>,
        reply: Option<Reply>,
    },
    Guide {
        step: TutorialStep,
        notice_id: String,
    },
}
#[derive(Default)]
pub(crate) struct TutorialProgressPresenter {
    next_id: u64,
    flows: HashMap<u64, Flow>,
}
impl TutorialProgressPresenter {
    pub(crate) fn handle(&mut self, event: ProgressEvent) -> Vec<UiEffect> {
        match event {
            ProgressEvent::Settings(reply) => {
                let id = self.insert(Flow::Settings {
                    reply,
                    failure: None,
                });
                vec![task(id, ProgressAction::BeginSettings)]
            }
            ProgressEvent::Watch(reply) => {
                let id = self.insert(Flow::Watch {
                    reply,
                    failure: None,
                });
                vec![task(id, ProgressAction::BeginWatch)]
            }
            ProgressEvent::WatchDelayExpired(id) => {
                if matches!(self.flows.get(&id), Some(Flow::Watch { .. })) {
                    vec![task(id, ProgressAction::PresentAfterWatch)]
                } else {
                    Vec::new()
                }
            }

            ProgressEvent::Finish {
                step,
                skipped,
                automatic,
                reply,
            } => {
                let id = self.insert(Flow::Finish {
                    step,
                    skipped,
                    next: None,
                    automatic,
                    reply,
                });
                vec![task(id, ProgressAction::Finish { step, skipped })]
            }
            ProgressEvent::Response {
                entry_id,
                message,
                reply,
            } => {
                let action = ProgressAction::AcceptResponse {
                    entry_id: entry_id.clone(),
                    message: message.clone(),
                };
                let id = self.insert(Flow::Response {
                    entry_id,
                    message,
                    step: None,
                    reply: Some(reply),
                });
                vec![task(id, action)]
            }
            ProgressEvent::Guide(step) => {
                let Some(key) = tutorial_guide_intro_key(step) else {
                    return Vec::new();
                };
                let id = self.insert(Flow::Guide {
                    step,
                    notice_id: String::new(),
                });
                vec![task(id, ProgressAction::GuideDetails(key))]
            }
            ProgressEvent::AdvanceChecked {
                step,
                current,
                presented,
                guide,
                reply,
            } => {
                let ready = if guide {
                    tutorial_guide_intro_key(step).is_some() && current == Some(step) && presented
                } else {
                    tutorial_response_auto_advance_ready(step, current, presented)
                };
                let _ = reply.send(ready);
                Vec::new()
            }
            ProgressEvent::TransitionExpired { id, cancellation } => {
                if cancellation.is_cancelled() {
                    self.finish(id, Ok(()))
                } else {
                    self.present_next(id)
                }
            }
            ProgressEvent::Completed { id, result } => self.completed(id, result),
        }
    }
    fn insert(&mut self, flow: Flow) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.flows.insert(self.next_id, flow);
        self.next_id
    }
    fn completed(&mut self, id: u64, result: ProgressResult) -> Vec<UiEffect> {
        let Some(flow) = self.flows.get_mut(&id) else {
            return Vec::new();
        };
        match result {
            ProgressResult::Settings(highlight) => match highlight {
                None => self.finish(id, Ok(())),
                Some(crate::tutorial::TutorialSettingsHighlight::Watch) => {
                    vec![task(id, ProgressAction::CompleteSettings)]
                }
                Some(crate::tutorial::TutorialSettingsHighlight::Persona) => {
                    vec![task(id, ProgressAction::Clear)]
                }
            },
            ProgressResult::Watch(false) => self.finish(id, Ok(())),
            ProgressResult::Watch(true) => vec![UiEffect::Spawn(UiTask::Delay {
                duration: std::time::Duration::from_secs(2),
                event: event(ProgressEvent::WatchDelayExpired(id)),
            })],
            ProgressResult::Current(Some(TutorialStep::Watch)) => {
                vec![task(id, ProgressAction::FinishWatch)]
            }
            ProgressResult::Current(_) => self.finish(id, Ok(())),
            ProgressResult::Presented(outcome)
                if matches!(flow, Flow::Settings { .. } | Flow::Watch { .. }) =>
            {
                let acknowledged = matches!(
                    outcome,
                    Ok(crate::tutorial_notice::TutorialBubbleOutcome::Acknowledged)
                );
                let action = match flow {
                    Flow::Settings { failure, .. } => {
                        *failure = outcome.err();
                        if acknowledged {
                            ProgressAction::CompleteSettings
                        } else {
                            ProgressAction::FailSettings
                        }
                    }
                    Flow::Watch { failure, .. } => {
                        *failure = outcome.err();
                        if acknowledged {
                            ProgressAction::GuideDetails("after-watch")
                        } else {
                            ProgressAction::FailWatch
                        }
                    }
                    _ => unreachable!(),
                };
                vec![task(id, action)]
            }

            ProgressResult::Step(Ok(value)) => {
                let Flow::Finish { next, .. } = flow else {
                    return Vec::new();
                };
                *next = value;
                vec![task(id, ProgressAction::Clear)]
            }
            ProgressResult::Step(Err(error))
            | ProgressResult::Advanced(Err(error))
            | ProgressResult::Done(Err(error))
            | ProgressResult::Presented(Err(error)) => self.finish(id, Err(error)),
            ProgressResult::Response(accepted) => {
                let Flow::Response {
                    entry_id,
                    message,
                    step,
                    reply,
                } = flow
                else {
                    return Vec::new();
                };
                if let Some(reply) = reply.take() {
                    let _ = reply.send(Ok(()));
                }
                *step = accepted;
                if accepted.is_some_and(tutorial_response_auto_advance_step) {
                    let delay = reading_delay(message);
                    let mut effects = Vec::new();
                    if accepted == Some(TutorialStep::Chat) {
                        effects.push(UiEffect::Log(crate::e2e_logs::chat_response(
                            entry_id,
                            delay.as_millis(),
                        )));
                    }
                    effects.push(task(
                        id,
                        ProgressAction::Read {
                            notice_id: entry_id.clone(),
                            delay,
                        },
                    ));
                    effects
                } else {
                    self.finish(id, Ok(()))
                }
            }
            ProgressResult::Guide(details) => {
                if matches!(flow, Flow::Watch { .. }) {
                    return match details {
                        Ok((notice_id, message)) => vec![task(
                            id,
                            ProgressAction::Read {
                                notice_id,
                                delay: reading_delay(&message),
                            },
                        )],
                        Err(error) => self.finish(id, Err(RuntimeError::Factory(error))),
                    };
                }
                let Flow::Guide { notice_id, .. } = flow else {
                    return Vec::new();
                };
                match details {
                    Ok((id_value, message)) => {
                        *notice_id = id_value.clone();
                        vec![task(
                            id,
                            ProgressAction::Read {
                                notice_id: id_value,
                                delay: reading_delay(&message),
                            },
                        )]
                    }
                    Err(error) => self.finish(id, Err(RuntimeError::Factory(error))),
                }
            }
            ProgressResult::Read(true) => {
                let action = match flow {
                    Flow::Watch { .. } => ProgressAction::Current,
                    Flow::Response {
                        step: Some(step),
                        entry_id,
                        ..
                    } => ProgressAction::Advance {
                        step: *step,
                        notice_id: entry_id.clone(),
                        guide: false,
                    },
                    Flow::Guide { step, notice_id } => ProgressAction::Advance {
                        step: *step,
                        notice_id: notice_id.clone(),
                        guide: true,
                    },
                    _ => return Vec::new(),
                };
                vec![task(id, action)]
            }
            ProgressResult::Advanced(Ok(advanced)) => {
                let log = match flow {
                    Flow::Response {
                        step: Some(step), ..
                    } if advanced => Some(UiEffect::Log(crate::e2e_logs::response_read(*step))),
                    _ => None,
                };
                let mut effects = self.finish(id, Ok(()));
                effects.extend(log);
                effects
            }
            ProgressResult::Read(false) | ProgressResult::Done(Ok(())) => self.finish(id, Ok(())),
            ProgressResult::Cleared {
                cleared,
                cancellation,
            } => {
                let needs_intro = match flow {
                    Flow::Settings { .. } => true,
                    Flow::Finish { next, .. } => next.and_then(step_intro_key).is_some(),
                    _ => return Vec::new(),
                };
                if needs_intro && cleared {
                    return vec![UiEffect::Spawn(UiTask::Delay {
                        duration: crate::bubbles::TUTORIAL_TRANSITION_DELAY,
                        event: event(ProgressEvent::TransitionExpired { id, cancellation }),
                    })];
                }
                self.present_next(id)
            }
            ProgressResult::Permission => {
                let Flow::Finish { next, .. } = flow else {
                    return Vec::new();
                };
                match next.and_then(step_intro_key) {
                    Some(key) => vec![task(id, ProgressAction::Present(key))],
                    None => self.finished_step(id),
                }
            }
            ProgressResult::Presented(Ok(_)) => self.finished_step(id),
        }
    }
    fn present_next(&mut self, id: u64) -> Vec<UiEffect> {
        if matches!(self.flows.get(&id), Some(Flow::Settings { .. })) {
            return vec![task(id, ProgressAction::WatchSequence)];
        }
        let Some(Flow::Finish {
            step,
            skipped,
            next,
            ..
        }) = self.flows.get(&id)
        else {
            return Vec::new();
        };
        let presentation = tutorial_step_presentation(*step, *skipped, *next);
        let mut effects = Vec::new();
        if presentation.hide_main {
            effects.push(UiEffect::Deliver {
                child: PresenterId::Chat,
                event: UiEvent::Close,
            });
        }
        if let Some(key) = presentation.next_intro {
            effects.push(task(
                id,
                match next {
                    Some(TutorialStep::Image | TutorialStep::Voice) => {
                        ProgressAction::Permission(next.expect("next intro step"))
                    }
                    _ => ProgressAction::Present(key),
                },
            ));
        } else {
            effects.extend(self.finished_step(id));
        }
        effects
    }
    fn finished_step(&mut self, id: u64) -> Vec<UiEffect> {
        let Some(Flow::Finish {
            next, automatic, ..
        }) = self.flows.get(&id)
        else {
            return Vec::new();
        };
        let followup = if *automatic { Some(*next) } else { None };
        let mut effects = self.finish(id, Ok(()));
        match followup {
            Some(Some(step)) => effects.extend(self.handle(ProgressEvent::Guide(step))),
            Some(None) => effects.push(task(id, ProgressAction::FinishTutorial)),
            None => {}
        }
        effects
    }
    fn finish(&mut self, id: u64, result: Result<(), RuntimeError>) -> Vec<UiEffect> {
        match self.flows.remove(&id) {
            Some(Flow::Settings { reply, failure }) | Some(Flow::Watch { reply, failure }) => {
                let _ = reply.send(failure.map_or(result, Err));
                Vec::new()
            }
            Some(Flow::Finish { reply, .. })
            | Some(Flow::Response {
                reply: Some(reply), ..
            }) => {
                let _ = reply.send(result);
                Vec::new()
            }
            _ => result
                .err()
                .map(|error| UiEffect::Log(format!("チュートリアルの進行に失敗しました: {error}")))
                .into_iter()
                .collect(),
        }
    }
}
pub(crate) fn event(value: ProgressEvent) -> UiEvent {
    UiEvent::Tutorial(Box::new(TutorialEvent::Progress(value)))
}
fn task(id: u64, action: ProgressAction) -> UiEffect {
    UiEffect::Spawn(UiTask::Tutorial(TutorialTask::Progress(ProgressTask {
        id,
        action,
    })))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TutorialStepPresentation {
    hide_main: bool,
    next_intro: Option<&'static str>,
}

fn tutorial_step_presentation(
    completed: TutorialStep,
    skipped: bool,
    next: Option<TutorialStep>,
) -> TutorialStepPresentation {
    TutorialStepPresentation {
        hide_main: completed == TutorialStep::Chat && !skipped,
        next_intro: next.and_then(step_intro_key),
    }
}

fn tutorial_response_auto_advance_step(step: TutorialStep) -> bool {
    matches!(
        step,
        TutorialStep::Chat | TutorialStep::Text | TutorialStep::Image | TutorialStep::Voice
    )
}

fn tutorial_guide_intro_key(step: TutorialStep) -> Option<&'static str> {
    match step {
        TutorialStep::Persona => Some("persona-intro"),
        TutorialStep::Chat
        | TutorialStep::Text
        | TutorialStep::Image
        | TutorialStep::Voice
        | TutorialStep::Watch => None,
    }
}

fn tutorial_response_auto_advance_ready(
    step: TutorialStep,
    current: Option<TutorialStep>,
    response_presented: bool,
) -> bool {
    tutorial_response_auto_advance_step(step) && current == Some(step) && response_presented
}

pub(crate) fn step_intro_key(step: TutorialStep) -> Option<&'static str> {
    match step {
        TutorialStep::Text => Some("text-intro"),
        TutorialStep::Image => Some("image-intro"),
        TutorialStep::Voice => Some("voice-intro"),
        TutorialStep::Persona => Some("persona-intro"),
        TutorialStep::Watch => Some("watch-intro"),
        TutorialStep::Chat => None,
    }
}

