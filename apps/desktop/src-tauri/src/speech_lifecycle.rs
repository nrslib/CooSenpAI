use coosenpai_core::ports::SpeechSessionControl;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpeechSource {
    Shortcut,
    Composer,
}

impl SpeechSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Shortcut => "shortcut",
            Self::Composer => "composer",
        }
    }
}

pub(crate) enum Lifecycle {
    Idle,
    Starting {
        generation: u64,
        cancellation: CancellationToken,
        finish_requested: bool,
    },
    Active {
        generation: u64,
        cancellation: CancellationToken,
        control: SpeechSessionControl,
        finishing: bool,
    },
    Confirming {
        generation: u64,
    },
    Sending {
        generation: u64,
    },
    Cancelling {
        generation: u64,
    },
    Cleaning {
        generation: u64,
    },
}

pub(crate) struct SpeechLifecycle {
    state: Lifecycle,
    next_generation: u64,
    revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpeechShortcutState {
    pub(crate) revision: u64,
    pub(crate) cancel_generation: Option<u64>,
}

pub(crate) enum StartOutcome {
    Continue,
    FinishBeforeStart,
    Stale,
}

pub(crate) enum SessionOutcome {
    Active,
    Finish(SpeechSessionControl),
    Cancel(SpeechSessionControl),
}

pub(crate) struct FinishOutcome {
    pub(crate) generation: u64,
    pub(crate) control: Option<SpeechSessionControl>,
}

pub(crate) struct CancelOutcome {
    pub(crate) generation: Option<u64>,
    pub(crate) cancellation: Option<CancellationToken>,
    pub(crate) control: Option<SpeechSessionControl>,
    pub(crate) changed: bool,
    pub(crate) startup_owned: bool,
    pub(crate) message: Option<&'static str>,
}

pub(crate) enum FinalOutcome {
    Composer,
    Confirm,
    Send,
}

impl Default for SpeechLifecycle {
    fn default() -> Self {
        Self {
            state: Lifecycle::Idle,
            next_generation: 0,
            revision: 0,
        }
    }
}

impl SpeechLifecycle {

    pub(crate) fn start_with_generation(
        &mut self,
        cancellation: CancellationToken,
        generation: u64,
    ) -> Option<u64> {
        if !matches!(self.state, Lifecycle::Idle) {
            return None;
        }
        self.next_generation = self.next_generation.max(generation);
        self.state = Lifecycle::Starting {
            generation,
            cancellation,
            finish_requested: false,
        };
        self.bump_revision();
        Some(generation)
    }

    pub(crate) fn finish(&mut self) -> Option<FinishOutcome> {
        match &mut self.state {
            Lifecycle::Starting {
                generation,
                finish_requested,
                ..
            } => {
                let generation = *generation;
                *finish_requested = true;
                self.bump_revision();
                Some(FinishOutcome {
                    generation,
                    control: None,
                })
            }
            Lifecycle::Active {
                generation,
                control,
                finishing,
                ..
            } if !*finishing => {
                let generation = *generation;
                *finishing = true;
                let control = control.clone();
                self.bump_revision();
                Some(FinishOutcome {
                    generation,
                    control: Some(control),
                })
            }
            Lifecycle::Idle
            | Lifecycle::Active { .. }
            | Lifecycle::Confirming { .. }
            | Lifecycle::Sending { .. }
            | Lifecycle::Cancelling { .. }
            | Lifecycle::Cleaning { .. } => None,
        }
    }

    pub(crate) fn finish_generation(&mut self, generation: u64) -> Option<FinishOutcome> {
        if !matches!(
            self.state,
            Lifecycle::Starting { generation: current, finish_requested: false, .. }
                | Lifecycle::Active { generation: current, finishing: false, .. }
                if current == generation
        ) {
            return None;
        }
        self.finish()
    }

    pub(crate) fn continue_start(&mut self, generation: u64) -> StartOutcome {
        match &self.state {
            Lifecycle::Starting {
                generation: current,
                finish_requested: true,
                ..
            } if *current == generation => {
                self.state = Lifecycle::Cleaning { generation };
                self.bump_revision();
                StartOutcome::FinishBeforeStart
            }
            Lifecycle::Starting {
                generation: current,
                ..
            } if *current == generation => StartOutcome::Continue,
            _ => StartOutcome::Stale,
        }
    }

    pub(crate) fn attach_session(
        &mut self,
        generation: u64,
        cancellation: CancellationToken,
        control: SpeechSessionControl,
    ) -> SessionOutcome {
        match &self.state {
            Lifecycle::Starting {
                generation: current,
                finish_requested,
                ..
            } if *current == generation => {
                let finish = *finish_requested;
                self.state = Lifecycle::Active {
                    generation,
                    cancellation,
                    control: control.clone(),
                    finishing: finish,
                };
                self.bump_revision();
                if finish {
                    SessionOutcome::Finish(control)
                } else {
                    SessionOutcome::Active
                }
            }
            _ => SessionOutcome::Cancel(control),
        }
    }

    pub(crate) fn cancel(&mut self) -> CancelOutcome {
        if let Lifecycle::Sending { .. } = &self.state {
            return CancelOutcome {
                generation: Some(self.next_generation),
                cancellation: None,
                control: None,
                changed: false,
                startup_owned: false,
                message: Some("音声入力を送信中です"),
            };
        }
        if let Lifecycle::Cancelling { .. } | Lifecycle::Cleaning { .. } = &self.state {
            return CancelOutcome {
                generation: Some(self.next_generation),
                cancellation: None,
                control: None,
                changed: false,
                startup_owned: false,
                message: Some("音声入力を終了しています"),
            };
        }
        let previous = std::mem::replace(&mut self.state, Lifecycle::Idle);
        match previous {
            Lifecycle::Starting {
                generation,
                cancellation,
                ..
            } => {
                self.state = Lifecycle::Cancelling { generation };
                self.bump_revision();
                CancelOutcome {
                    generation: Some(generation),
                    cancellation: Some(cancellation),
                    control: None,
                    changed: true,
                    startup_owned: true,
                    message: None,
                }
            }
            Lifecycle::Active {
                generation,
                cancellation,
                control,
                ..
            } => {
                self.state = Lifecycle::Cancelling { generation };
                self.bump_revision();
                CancelOutcome {
                    generation: Some(generation),
                    cancellation: Some(cancellation),
                    control: Some(control),
                    changed: true,
                    startup_owned: false,
                    message: None,
                }
            }
            Lifecycle::Confirming { generation } => {
                self.state = Lifecycle::Cancelling { generation };
                self.bump_revision();
                CancelOutcome {
                    generation: Some(generation),
                    cancellation: None,
                    control: None,
                    changed: true,
                    startup_owned: false,
                    message: None,
                }
            }
            Lifecycle::Idle => CancelOutcome {
                generation: None,
                cancellation: None,
                control: None,
                changed: false,
                startup_owned: false,
                message: None,
            },
            Lifecycle::Sending { .. }
            | Lifecycle::Cancelling { .. }
            | Lifecycle::Cleaning { .. } => unreachable!(),
        }
    }

    pub(crate) fn complete_cancel(&mut self, generation: u64) -> bool {
        if matches!(self.state, Lifecycle::Cancelling { generation: current } if current == generation)
        {
            self.state = Lifecycle::Idle;
            self.bump_revision();
            true
        } else {
            false
        }
    }

    pub(crate) fn complete_cleanup(&mut self, generation: u64) -> bool {
        if matches!(self.state, Lifecycle::Cleaning { generation: current } if current == generation)
        {
            self.state = Lifecycle::Idle;
            self.bump_revision();
            true
        } else {
            false
        }
    }

    pub(crate) fn claim_final(
        &mut self,
        generation: u64,
        source: SpeechSource,
        confirm_before_send: bool,
    ) -> Option<FinalOutcome> {
        if !matches!(
            &self.state,
            Lifecycle::Active {
                generation: current,
                ..
            } if *current == generation
        ) {
            return None;
        }
        let outcome = match source {
            SpeechSource::Composer => FinalOutcome::Composer,
            SpeechSource::Shortcut if confirm_before_send => FinalOutcome::Confirm,
            SpeechSource::Shortcut => FinalOutcome::Send,
        };
        self.state = match outcome {
            FinalOutcome::Confirm => Lifecycle::Confirming { generation },
            FinalOutcome::Send => Lifecycle::Sending { generation },
            FinalOutcome::Composer => Lifecycle::Cleaning { generation },
        };
        self.bump_revision();
        Some(outcome)
    }

    pub(crate) fn claim_confirmation(&mut self) -> Option<u64> {
        let generation = match self.state {
            Lifecycle::Confirming { generation } => generation,
            _ => return None,
        };
        self.state = Lifecycle::Sending { generation };
        self.bump_revision();
        Some(generation)
    }

    pub(crate) fn restore_confirmation(&mut self, generation: u64) -> bool {
        if matches!(self.state, Lifecycle::Sending { generation: current } if current == generation)
        {
            self.state = Lifecycle::Confirming { generation };
            self.bump_revision();
            true
        } else {
            false
        }
    }

    pub(crate) fn complete(&mut self, generation: u64) -> bool {
        if self.is_current(generation) && !self.can_apply_cleanup(generation) {
            self.state = Lifecycle::Cleaning { generation };
            self.bump_revision();
            true
        } else {
            false
        }
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        match self.state {
            Lifecycle::Starting {
                generation: current,
                ..
            }
            | Lifecycle::Active {
                generation: current,
                ..
            }
            | Lifecycle::Confirming {
                generation: current,
            }
            | Lifecycle::Sending {
                generation: current,
            }
            | Lifecycle::Cancelling {
                generation: current,
            }
            | Lifecycle::Cleaning {
                generation: current,
            } => current == generation,
            Lifecycle::Idle => false,
        }
    }

    pub(crate) fn is_recording(&self) -> bool {
        matches!(
            self.state,
            Lifecycle::Starting { .. } | Lifecycle::Active { .. }
        )
    }

    pub(crate) fn accepts_session_events(&self, generation: u64) -> bool {
        matches!(
            self.state,
            Lifecycle::Active {
                generation: current,
                ..
            } if current == generation
        )
    }

    pub(crate) fn is_confirming(&self, generation: u64) -> bool {
        matches!(
            self.state,
            Lifecycle::Confirming {
                generation: current
            } if current == generation
        )
    }

    pub(crate) fn confirming_generation(&self) -> Option<u64> {
        match self.state {
            Lifecycle::Confirming { generation } => Some(generation),
            Lifecycle::Idle
            | Lifecycle::Starting { .. }
            | Lifecycle::Active { .. }
            | Lifecycle::Sending { .. }
            | Lifecycle::Cancelling { .. }
            | Lifecycle::Cleaning { .. } => None,
        }
    }

    pub(crate) fn is_sending(&self, generation: u64) -> bool {
        matches!(
            self.state,
            Lifecycle::Sending {
                generation: current
            } if current == generation
        )
    }

    pub(crate) fn is_finalizing(&self, generation: u64) -> bool {
        matches!(
            self.state,
            Lifecycle::Active {
                generation: current,
                finishing: true,
                ..
            } if current == generation
        )
    }

    pub(crate) fn phase(&self) -> &'static str {
        match self.state {
            Lifecycle::Idle => "idle",
            Lifecycle::Starting { .. } => "starting",
            Lifecycle::Active {
                finishing: true, ..
            } => "finalizing",
            Lifecycle::Active { .. } => "recording",
            Lifecycle::Confirming { .. } => "confirming",
            Lifecycle::Sending { .. } => "sending",
            Lifecycle::Cancelling { .. } => "cancelling",
            Lifecycle::Cleaning { .. } => "cleaning",
        }
    }

    pub(crate) fn can_apply_cleanup(&self, generation: u64) -> bool {
        self.next_generation == generation
            && (matches!(self.state, Lifecycle::Cleaning { generation: current } if current == generation)
                || matches!(self.state, Lifecycle::Cancelling { generation: current } if current == generation))
    }

    pub(crate) fn is_cancelling(&self, generation: u64) -> bool {
        matches!(
            self.state,
            Lifecycle::Cancelling {
                generation: current
            } if current == generation
        )
    }

    pub(crate) fn cancelling_generation(&self) -> Option<u64> {
        match self.state {
            Lifecycle::Cancelling { generation } => Some(generation),
            Lifecycle::Idle
            | Lifecycle::Starting { .. }
            | Lifecycle::Active { .. }
            | Lifecycle::Confirming { .. }
            | Lifecycle::Sending { .. }
            | Lifecycle::Cleaning { .. } => None,
        }
    }

    pub(crate) fn monitors_key_release(&self, generation: u64) -> bool {
        matches!(
            self.state,
            Lifecycle::Starting { generation: current, finish_requested: false, .. }
                | Lifecycle::Active { generation: current, finishing: false, .. }
                if current == generation
        )
    }

    pub(crate) fn shortcut_state(&self) -> SpeechShortcutState {
        let cancel_generation = match self.state {
            Lifecycle::Starting { generation, .. } | Lifecycle::Active { generation, .. } => {
                Some(generation)
            }
            Lifecycle::Idle
            | Lifecycle::Confirming { .. }
            | Lifecycle::Sending { .. }
            | Lifecycle::Cancelling { .. }
            | Lifecycle::Cleaning { .. } => None,
        };
        SpeechShortcutState {
            revision: self.revision,
            cancel_generation,
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

