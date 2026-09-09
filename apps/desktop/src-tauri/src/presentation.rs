#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PresentationState {
    #[default]
    Hidden,
    Loading {
        generation: u64,
    },
    Shown,
    CloseFailed,
}

#[derive(Debug)]
pub(crate) enum PresentationEvent<Request, Content> {
    Open(Request),
    Loaded {
        generation: u64,
        result: Result<Option<Content>, String>,
    },
    Hide,
    CancelLoad {
        generation: u64,
    },
}

pub(crate) enum PresentationAction<Request, Content> {
    Load {
        generation: u64,
        request: Request,
    },
    Show(Content),
    Hide,
    Unavailable,
    Failed(String),
    Ignored {
        received: u64,
        expected: Option<u64>,
    },
}

#[derive(Default)]
pub(crate) struct Presentation {
    state: PresentationState,
    generation: u64,
}

impl Presentation {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn close_failed(&mut self) {
        self.state = PresentationState::CloseFailed;
    }

    pub(crate) fn state(&self) -> PresentationState {
        self.state
    }

    pub(crate) fn transition<Request, Content>(
        &mut self,
        event: PresentationEvent<Request, Content>,
    ) -> PresentationAction<Request, Content> {
        use PresentationAction as Action;
        use PresentationEvent as Event;
        use PresentationState as State;
        match (self.state, event) {
            (_, Event::Open(request)) => {
                self.generation += 1;
                self.state = State::Loading {
                    generation: self.generation,
                };
                Action::Load {
                    generation: self.generation,
                    request,
                }
            }
            (_, Event::Hide) => {
                self.state = State::Hidden;
                Action::Hide
            }
            (
                State::Loading {
                    generation: expected,
                },
                Event::Loaded { generation, result },
            ) if generation == expected => match result {
                Ok(Some(content)) => {
                    self.state = State::Shown;
                    Action::Show(content)
                }
                Ok(None) => {
                    self.state = State::Hidden;
                    Action::Unavailable
                }
                Err(error) => {
                    self.state = State::Hidden;
                    Action::Failed(error)
                }
            },
            (
                State::Loading {
                    generation: expected,
                },
                Event::CancelLoad { generation },
            ) if generation == expected => {
                self.state = State::Hidden;
                Action::Hide
            }
            (state, Event::Loaded { generation, .. } | Event::CancelLoad { generation }) => {
                Action::Ignored {
                    received: generation,
                    expected: match state {
                        State::Loading { generation } => Some(generation),
                        State::Hidden | State::Shown | State::CloseFailed => None,
                    },
                }
            }
        }
    }
}

