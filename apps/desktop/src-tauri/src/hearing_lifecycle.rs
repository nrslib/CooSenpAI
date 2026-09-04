use coosenpai_core::ports::HearingSessionControl;
use coosenpai_core::state::AudioObservationSource;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HearingSessionSettings {
    pub(crate) locale: String,
    pub(crate) input_device: String,
    pub(crate) sources: Vec<AudioObservationSource>,
    pub(crate) debug_dump_dir: Option<String>,
}

impl HearingSessionSettings {
    pub(crate) fn new(
        locale: impl Into<String>,
        input_device: impl Into<String>,
        sources: Vec<AudioObservationSource>,
    ) -> Self {
        Self {
            locale: locale.into(),
            input_device: input_device.into(),
            sources,
            debug_dump_dir: None,
        }
    }

    pub(crate) fn with_debug_dump_dir(mut self, debug_dump_dir: Option<String>) -> Self {
        self.debug_dump_dir = debug_dump_dir;
        self
    }
}

pub(crate) enum Lifecycle {
    Idle,
    Starting {
        generation: u64,
        cancellation: CancellationToken,
        settings: HearingSessionSettings,
        initialization_completed: oneshot::Receiver<()>,
    },
    Listening {
        generation: u64,
        cancellation: CancellationToken,
        control: HearingSessionControl,
        settings: HearingSessionSettings,
    },
    Stopping {
        generation: u64,
        cancellation: CancellationToken,
        control: Option<HearingSessionControl>,
        settings: HearingSessionSettings,
        initialization_completed: Option<oneshot::Receiver<()>>,
    },
}

pub(crate) struct HearingLifecycle {
    state: Lifecycle,
    next_generation: u64,
}

pub(crate) struct StopOutcome {
    pub(crate) generation: u64,
    pub(crate) cancellation: CancellationToken,
    pub(crate) control: Option<HearingSessionControl>,
    pub(crate) changed: bool,
    pub(crate) initialization_completed: Option<oneshot::Receiver<()>>,
}

pub(crate) enum AttachOutcome {
    Listening,
    Cancel(HearingSessionControl),
}

impl Default for HearingLifecycle {
    fn default() -> Self {
        Self {
            state: Lifecycle::Idle,
            next_generation: 0,
        }
    }
}

impl HearingLifecycle {
    pub(crate) fn start(
        &mut self,
        cancellation: CancellationToken,
        settings: HearingSessionSettings,
        initialization_completed: oneshot::Receiver<()>,
    ) -> Option<u64> {
        if !matches!(self.state, Lifecycle::Idle) || settings.sources.is_empty() {
            return None;
        }
        let generation = self.next_generation.saturating_add(1);
        self.next_generation = generation;
        self.state = Lifecycle::Starting {
            generation,
            cancellation,
            settings,
            initialization_completed,
        };
        Some(generation)
    }

    pub(crate) fn accepts_events(&self, generation: u64) -> bool {
        matches!(
            self.state,
            Lifecycle::Listening { generation: current, .. } if current == generation
        )
    }

    pub(crate) fn same_settings(&self, settings: &HearingSessionSettings) -> bool {
        match &self.state {
            Lifecycle::Starting {
                settings: current, ..
            }
            | Lifecycle::Listening {
                settings: current, ..
            } => current == settings,
            Lifecycle::Idle | Lifecycle::Stopping { .. } => false,
        }
    }

    pub(crate) fn attach_session(
        &mut self,
        generation: u64,
        cancellation: CancellationToken,
        control: HearingSessionControl,
    ) -> AttachOutcome {
        match &self.state {
            Lifecycle::Starting {
                generation: current,
                settings,
                ..
            } if *current == generation => {
                self.state = Lifecycle::Listening {
                    generation,
                    cancellation,
                    control,
                    settings: settings.clone(),
                };
                AttachOutcome::Listening
            }
            _ => AttachOutcome::Cancel(control),
        }
    }

    pub(crate) fn stop(&mut self) -> Option<StopOutcome> {
        let previous = std::mem::replace(&mut self.state, Lifecycle::Idle);
        let (generation, cancellation, control, settings, initialization_completed) = match previous
        {
            Lifecycle::Starting {
                generation,
                cancellation,
                settings,
                initialization_completed,
            } => (
                generation,
                cancellation,
                None,
                settings,
                Some(initialization_completed),
            ),
            Lifecycle::Listening {
                generation,
                cancellation,
                control,
                settings,
            } => (generation, cancellation, Some(control), settings, None),
            Lifecycle::Idle => return None,
            Lifecycle::Stopping {
                generation,
                cancellation,
                control,
                settings,
                initialization_completed,
            } => {
                let outcome = StopOutcome {
                    generation,
                    cancellation: cancellation.clone(),
                    control: control.clone(),
                    changed: false,
                    initialization_completed: None,
                };
                self.state = Lifecycle::Stopping {
                    generation,
                    cancellation,
                    control,
                    settings,
                    initialization_completed,
                };
                return Some(outcome);
            }
        };
        self.state = Lifecycle::Stopping {
            generation,
            cancellation: cancellation.clone(),
            control: control.clone(),
            settings,
            initialization_completed: None,
        };
        Some(StopOutcome {
            generation,
            cancellation,
            control,
            changed: true,
            initialization_completed,
        })
    }

    pub(crate) fn complete_stop(&mut self, generation: u64) -> bool {
        if matches!(self.state, Lifecycle::Stopping { generation: current, .. } if current == generation)
        {
            self.state = Lifecycle::Idle;
            return true;
        }
        false
    }

    pub(crate) fn fail(&mut self, generation: u64) -> Option<StopOutcome> {
        let (current, cancellation, control) = match &self.state {
            Lifecycle::Starting {
                generation: current,
                cancellation,
                ..
            } => (*current, cancellation.clone(), None),
            Lifecycle::Listening {
                generation: current,
                cancellation,
                control,
                ..
            } => (*current, cancellation.clone(), Some(control.clone())),
            Lifecycle::Stopping {
                generation: current,
                cancellation,
                control,
                ..
            } => (*current, cancellation.clone(), control.clone()),
            Lifecycle::Idle => return None,
        };
        if current != generation {
            return None;
        }
        self.state = Lifecycle::Idle;
        Some(StopOutcome {
            generation,
            cancellation,
            control,
            changed: true,
            initialization_completed: None,
        })
    }
}

