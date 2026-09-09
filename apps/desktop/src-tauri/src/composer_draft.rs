use super::Selection;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SelectionState {
    Pending,
    Applied,
    Cancelled,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct ImeSession {
    pub session: u64,
    pub active: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftState {
    pub edit_revision: u64,
    pub text: String,
    pub directive: u64,
    pub selection: Selection,
    pub focus: bool,
    pub ime: ImeSession,
    #[serde(skip)]
    pub version: u64,
    #[serde(skip)]
    last_transcript: u64,
    #[serde(skip)]
    pending_transcript: Option<String>,
}

pub(crate) struct Observation {
    pub revision: u64,
    pub directive: u64,
    pub selection_state: SelectionState,
    pub text: String,
    pub selection: Selection,
}

pub(crate) enum Transition {
    Edit(Observation),
    Observe(Observation),
    Select {
        revision: u64,
        directive: u64,
        selection_state: SelectionState,
        selection: Selection,
    },
    Composition {
        session: u64,
        active: bool,
        observation: Observation,
    },
    Transcript {
        generation: u64,
        text: String,
    },
    Replace {
        text: String,
        selection: Selection,
        focus: bool,
    },
    Restore {
        version: u64,
        text: String,
        selection: Selection,
    },
}

#[derive(Default)]
pub(crate) struct Outcome {
    pub accepted: bool,
    pub reset_history: bool,
}

impl DraftState {
    pub(crate) fn transition(&mut self, event: Transition) -> Outcome {
        match event {
            Transition::Edit(observation) => self.observe(observation, true),
            Transition::Observe(observation) => self.observe(observation, false),
            Transition::Select {
                revision,
                directive,
                selection_state,
                selection,
            } => {
                let accepted = revision == self.edit_revision
                    && self.selection_is_current(directive, selection_state);
                if accepted {
                    self.selection = clamp_selection(&self.text, selection);
                }
                Outcome {
                    accepted,
                    reset_history: false,
                }
            }
            Transition::Composition {
                session,
                active,
                observation,
            } => {
                let accepted = if active {
                    session > self.ime.session
                } else {
                    session > self.ime.session || (session == self.ime.session && self.ime.active)
                };
                if !accepted {
                    return Outcome::default();
                }
                let mut outcome = self.observe(observation, false);
                self.ime = ImeSession { session, active };
                if !active {
                    if let Some(text) = self.pending_transcript.take() {
                        self.insert(text);
                        outcome.reset_history = true;
                    }
                }
                outcome.accepted = true;
                outcome
            }
            Transition::Transcript { generation, text } => {
                if generation <= self.last_transcript {
                    return Outcome::default();
                }
                self.last_transcript = generation;
                if self.ime.active {
                    self.pending_transcript = Some(text);
                } else {
                    self.insert(text);
                }
                Outcome {
                    accepted: true,
                    reset_history: !self.ime.active,
                }
            }
            Transition::Restore {
                version,
                text,
                selection,
            } => {
                let accepted = version == self.version && self.text.is_empty();
                if accepted {
                    self.replace(text, selection, true);
                }
                Outcome {
                    accepted,
                    reset_history: false,
                }
            }
            Transition::Replace {
                text,
                selection,
                focus,
            } => {
                self.replace(text, selection, focus);
                Outcome {
                    accepted: true,
                    reset_history: false,
                }
            }
        }
    }

    fn selection_is_current(&self, directive: u64, state: SelectionState) -> bool {
        directive == self.directive && state != SelectionState::Pending
    }

    fn observe(&mut self, observation: Observation, edit: bool) -> Outcome {
        let selection_current =
            self.selection_is_current(observation.directive, observation.selection_state);
        if observation.revision < self.edit_revision || (!edit && !selection_current) {
            return Outcome::default();
        }
        let changed = observation.revision > self.edit_revision;
        if changed {
            self.edit_revision = observation.revision;
            self.text = observation.text;
            self.version += 1;
        }
        self.selection = clamp_selection(
            &self.text,
            if selection_current {
                observation.selection
            } else {
                self.selection
            },
        );
        Outcome {
            accepted: true,
            reset_history: changed,
        }
    }

    fn replace(&mut self, text: String, selection: Selection, focus: bool) {
        self.selection = clamp_selection(&text, selection);
        self.text = text;
        self.focus = focus;
        self.directive += 1;
        self.version += 1;
    }

    fn insert(&mut self, text: String) {
        let current: Vec<u16> = self.text.encode_utf16().collect();
        let selected = self.selection;
        let value: Vec<u16> = current[..selected.start]
            .iter()
            .copied()
            .chain(text.encode_utf16())
            .chain(current[selected.end..].iter().copied())
            .collect();
        let end = selected.start + text.encode_utf16().count();
        self.replace(
            String::from_utf16_lossy(&value),
            Selection { start: end, end },
            true,
        );
    }
}

fn clamp_selection(text: &str, selection: Selection) -> Selection {
    let length = text.encode_utf16().count();
    let start = selection.start.min(length);
    Selection {
        start,
        end: selection.end.max(start).min(length),
    }
}

