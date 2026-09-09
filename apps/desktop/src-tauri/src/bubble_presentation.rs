use super::{BubbleContent, BubblePresenter, BubbleWindowEvent};
use crate::presentation::{PresentationAction, PresentationEvent};
use crate::ui_events::{Handling, PresenterId, UiEffect, UiEvent, UiTask, ViewCommand};
use std::sync::Arc;
use std::time::Duration;

impl BubblePresenter {
    pub(crate) fn handle(&mut self, event: UiEvent) -> Handling {
        match event {
            UiEvent::BubbleWindow(event) => self.present(event),
            UiEvent::BubbleTypingTick(epoch) => {
                if epoch != self.typing_epoch {
                    return Handling::Handled(Vec::new());
                }
                let Some((id, revealed, count)) = self.typing.as_mut() else {
                    return Handling::Handled(Vec::new());
                };
                *revealed = (*revealed + count.div_ceil(72).max(1)).min(*count);
                let complete = *revealed == *count;
                let mut effects = vec![UiEffect::BubbleTyping {
                    id: id.clone(),
                    revealed: (!complete).then_some(*revealed),
                }];
                if !complete {
                    effects.push(UiEffect::Run(UiTask::Delay {
                        duration: std::time::Duration::from_millis(25),
                        event: UiEvent::BubbleTypingTick(epoch),
                    }));
                }
                Handling::Handled(effects)
            }
            UiEvent::BubbleHideExpired(generation) => {
                let current = self.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.generation == generation && snapshot.records.is_empty()
                });
                if current {
                    self.present(PresentationEvent::Hide)
                } else {
                    Handling::Handled(Vec::new())
                }
            }

            UiEvent::BubbleFocused(focused) => {
                Handling::Handled(vec![UiEffect::BubbleFocused(focused)])
            }
            UiEvent::RestorePointer => Handling::Handled(vec![UiEffect::Pointer(
                self.snapshot
                    .as_ref()
                    .is_some_and(|snapshot| !snapshot.records.is_empty()),
            )]),
            UiEvent::BubbleResize(height) => Handling::Handled(
                self.snapshot
                    .as_ref()
                    .map(|snapshot| UiEffect::BubbleResize {
                        height,
                        position: snapshot.position.clone(),
                        display: self.display.clone(),
                    })
                    .into_iter()
                    .collect(),
            ),
            UiEvent::Present(ViewCommand::Hide) => self.present(PresentationEvent::Hide),
            UiEvent::Present(command) => Handling::Handled(vec![UiEffect::View {
                view: PresenterId::Bubble,
                command,
            }]),
            event => Handling::Bubble(event),
        }
    }
}

impl BubblePresenter {
    pub(super) fn update(
        &mut self,
        snapshot: Arc<crate::bubbles::BubbleSnapshot>,
        display: String,
    ) -> Handling {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.generation > snapshot.generation)
        {
            return Handling::Handled(Vec::new());
        }
        if snapshot.records.is_empty() {
            if matches!(
                self.presentation.state(),
                crate::presentation::PresentationState::Loading { .. }
            ) {
                self.presentation
                    .transition::<BubbleContent, BubbleContent>(PresentationEvent::Hide);
            }
            self.render(snapshot, display)
        } else if self.presentation.state() != crate::presentation::PresentationState::Shown {
            self.present(PresentationEvent::Open(BubbleContent { snapshot, display }))
        } else {
            self.render(snapshot, display)
        }
    }

    pub(super) fn render(
        &mut self,
        snapshot: Arc<crate::bubbles::BubbleSnapshot>,
        display: String,
    ) -> Handling {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.generation > snapshot.generation)
        {
            return Handling::Handled(Vec::new());
        }
        self.controls.observe(&snapshot);
        let front = snapshot
            .records
            .iter()
            .find(|record| Some(&record.id) == snapshot.front_id.as_ref());
        let previous_front = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .records
                .iter()
                .find(|record| Some(&record.id) == snapshot.front_id.as_ref())
        });
        let restart_typing = front.map(|record| (&record.id, &record.message))
            != previous_front.map(|record| (&record.id, &record.message));
        let had_content = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.records.is_empty());
        let has_content = !snapshot.records.is_empty();
        let layout = self
            .snapshot
            .as_ref()
            .is_none_or(|previous| previous.position != snapshot.position)
            || self.display != display;
        self.snapshot = Some(snapshot.clone());
        self.display = display.clone();
        let mut effects = vec![
            UiEffect::BubbleControls(Box::new(self.controls.view.clone())),
            UiEffect::BubbleRender {
                snapshot: snapshot.clone(),
                display,
                layout,
            },
        ];
        if has_content && !had_content {
            effects.push(UiEffect::Pointer(true));
        }
        if !has_content && had_content {
            effects.push(UiEffect::Pointer(false));
            effects.push(UiEffect::Run(UiTask::Delay {
                duration: Duration::from_millis(180),
                event: UiEvent::BubbleHideExpired(snapshot.generation),
            }));
        }
        if let Some(record) = front {
            if !snapshot.reading {
                self.typing_epoch += 1;
                self.typing = None;
                effects.push(UiEffect::BubbleTyping {
                    id: record.id.clone(),
                    revealed: None,
                });
            } else if restart_typing {
                self.typing_epoch += 1;
                self.typing = Some((record.id.clone(), 0, record.message.chars().count()));
                effects.push(UiEffect::BubbleTyping {
                    id: record.id.clone(),
                    revealed: Some(0),
                });
                effects.push(UiEffect::Run(UiTask::Delay {
                    duration: std::time::Duration::from_millis(25),
                    event: UiEvent::BubbleTypingTick(self.typing_epoch),
                }));
            }
        } else {
            self.typing_epoch += 1;
            self.typing = None;
        }
        Handling::Handled(effects)
    }

    pub(super) fn present(&mut self, event: BubbleWindowEvent) -> Handling {
        let effects = match self.presentation.transition(event) {
            PresentationAction::Load { generation, request } => vec![UiEffect::Deliver {
                child: PresenterId::Root,
                event: UiEvent::BubbleWindow(PresentationEvent::Loaded { generation, result: Ok(Some(request)) }),
            }],
            PresentationAction::Show(content) => {
                let Handling::Handled(mut effects) = self.render(content.snapshot, content.display) else { unreachable!() };
                effects.push(UiEffect::Pointer(true));
                effects.push(UiEffect::View { view: PresenterId::Bubble, command: ViewCommand::Show });
                effects
            }
            PresentationAction::Hide | PresentationAction::Unavailable => self.hide(),
            PresentationAction::Failed(error) => {
                let mut effects = self.hide();
                effects.push(UiEffect::Fail(error));
                effects
            }
            PresentationAction::Ignored { received, expected } => vec![UiEffect::Log(format!(
                "ui: presenter=Bubble event=Loaded({received}) ignored=true reason=stale-generation expected={expected:?}"))],
        };
        Handling::Handled(effects)
    }

    fn hide(&mut self) -> Vec<UiEffect> {
        let had_controls = self.controls.view.record.is_some();
        self.controls.hide();
        self.typing_epoch += 1;
        self.typing = None;
        let mut effects = vec![
            UiEffect::Pointer(false),
            UiEffect::View {
                view: PresenterId::Bubble,
                command: ViewCommand::Hide,
            },
        ];
        if had_controls {
            effects.insert(
                0,
                UiEffect::BubbleControls(Box::new(self.controls.view.clone())),
            );
        }
        effects
    }
}
