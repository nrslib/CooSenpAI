use crate::snapshot::AppSnapshot;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask, UiView, VoiceAction};
use coosenpai_core::state::ConversationRole;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[path = "composer_draft.rs"]
mod draft;
pub(crate) use draft::SelectionState;
use draft::{DraftState, Observation, Outcome, Transition};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ComposerInput {
    Edit {
        revision: u64,
        directive: u64,
        selection_state: SelectionState,
        text: String,
        selection: Selection,
    },
    Selection {
        edit_revision: u64,
        directive: u64,
        selection_state: SelectionState,
        selection: Selection,
    },
    Composition {
        session: u64,
        active: bool,
        revision: u64,
        directive: u64,
        selection_state: SelectionState,
        text: String,
        selection: Selection,
    },
    Key {
        key: String,
        meta_key: bool,
        shift_key: bool,
        ctrl_key: bool,
        alt_key: bool,
        composing: bool,
        key_code: u32,
        revision: u64,
        directive: u64,
        selection_state: SelectionState,
        text: String,
        selection: Selection,
    },
    Submit {
        revision: u64,
        directive: u64,
        selection_state: SelectionState,
        text: String,
        selection: Selection,
    },
    Microphone {
        gesture: MicrophoneGesture,
    },
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MicrophoneGesture {
    Press,
    Release,
    Cancel,
    Click,
}

#[derive(Debug)]
pub(crate) enum ComposerEvent {
    Input(ComposerInput),
    Sent {
        token: u64,
        result: Result<Option<String>, String>,
    },
    VoiceCompleted(Result<Option<String>, String>),
}
#[derive(Debug)]
pub(crate) enum ComposerTask {
    Send { token: u64, message: String },
    Voice(VoiceAction),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyBinding {
    pub key: String,
    pub meta_key: bool,
    pub shift_key: bool,
    pub ctrl_key: bool,
    pub alt_key: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComposerView {
    #[serde(flatten)]
    pub draft: DraftState,
    pub ready: bool,
    pub can_send: bool,
    pub pending_sends: usize,
    pub recording: bool,
    pub bindings: Vec<KeyBinding>,
    pub error: Option<String>,
}
struct HistorySelection {
    id: String,
    draft: String,
    selection: Selection,
}
struct PendingSend {
    text: String,
    selection: Selection,
    cleared_version: u64,
}

#[derive(Default)]
pub(crate) struct ComposerPresenter {
    view: ComposerView,
    next_send: u64,
    pending: BTreeMap<u64, PendingSend>,
    history: Vec<(String, String)>,
    history_selection: Option<HistorySelection>,
    active_response: bool,
    send_key: String,
    voice_mode: String,
    held: bool,
    voice_running: bool,
    voice_queue: VecDeque<VoiceAction>,
    operation_busy: bool,
}

impl ComposerPresenter {
    pub(crate) fn observe(
        &mut self,
        snapshot: &AppSnapshot,
        today: chrono::NaiveDate,
    ) -> Vec<UiEffect> {
        self.view.ready = !snapshot.onboarding.setup_required
            && (!snapshot.onboarding.tutorial_active || snapshot.onboarding.chat_input_enabled);
        self.active_response = snapshot.active_user_message_id.is_some();
        self.send_key = snapshot.config.keymap.send_key.clone();
        self.voice_mode = snapshot.config.speech.mode.clone();
        if !self.voice_running && self.voice_queue.is_empty() {
            self.view.recording = snapshot.speech.source.as_deref() == Some("composer")
                && ["starting", "recording", "finalizing"]
                    .contains(&snapshot.speech.phase.as_str());
        }
        let mut bytes = 0;
        self.history = snapshot
            .conversation
            .iter()
            .rev()
            .filter(|entry| {
                entry.role == ConversationRole::User
                    && !entry.message.is_empty()
                    && chrono::DateTime::parse_from_rfc3339(&entry.created_at)
                        .is_ok_and(|date| date.with_timezone(&chrono::Local).date_naive() == today)
            })
            .filter(|entry| {
                if bytes + entry.message.len() > 32_768 {
                    false
                } else {
                    bytes += entry.message.len();
                    true
                }
            })
            .take(50)
            .map(|entry| (entry.id.clone(), entry.message.clone()))
            .collect();
        if self
            .history_selection
            .as_ref()
            .is_some_and(|selected| !self.history.iter().any(|(id, _)| *id == selected.id))
        {
            let selected = self.history_selection.take().unwrap();
            self.replace(selected.draft, selected.selection, false);
        }
        vec![self.render()]
    }

    pub(crate) fn pending_sends(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn set_operation_busy(&mut self, busy: bool) {
        self.operation_busy = busy;
    }
    pub(crate) fn transcript(&mut self, generation: u64, text: String) -> Vec<UiEffect> {
        let outcome = self.transition(Transition::Transcript { generation, text });
        if !outcome.accepted {
            return vec![];
        }
        vec![self.render()]
    }

    pub(crate) fn handle(&mut self, event: ComposerEvent) -> Vec<UiEffect> {
        let mut effects = match event {
            ComposerEvent::Input(input) => self.input(input),
            ComposerEvent::Sent { token, result } => {
                let Some(pending) = self.pending.remove(&token) else {
                    return vec![];
                };
                match result {
                    Ok(Some(_)) => self.view.error = None,
                    result => {
                        self.view.error = Some(match result {
                            Err(error) => error,
                            _ => "チャットの受付結果がありません".into(),
                        });
                        self.transition(Transition::Restore {
                            version: pending.cleared_version,
                            text: pending.text,
                            selection: pending.selection,
                        });
                    }
                }
                vec![]
            }
            ComposerEvent::VoiceCompleted(result) => {
                self.voice_running = false;
                if let Err(error) = result {
                    self.view.error = Some(error);
                    self.view.recording = false;
                }
                self.next_voice().into_iter().collect()
            }
        };
        effects.insert(0, self.render());
        effects
    }

    fn input(&mut self, input: ComposerInput) -> Vec<UiEffect> {
        match input {
            ComposerInput::Edit {
                revision,
                directive,
                selection_state,
                text,
                selection,
            } => {
                let outcome = self.transition(Transition::Edit(Observation {
                    revision,
                    directive,
                    selection_state,
                    text,
                    selection,
                }));
                Self::observation_log("Edit", outcome.accepted)
            }
            ComposerInput::Selection {
                edit_revision,
                directive,
                selection_state,
                selection,
            } => {
                let outcome = self.transition(Transition::Select {
                    revision: edit_revision,
                    directive,
                    selection_state,
                    selection,
                });
                Self::observation_log("Selection", outcome.accepted)
            }
            ComposerInput::Composition {
                session,
                active,
                revision,
                directive,
                selection_state,
                text,
                selection,
            } => {
                let outcome = self.transition(Transition::Composition {
                    session,
                    active,
                    observation: Observation {
                        revision,
                        directive,
                        selection_state,
                        text,
                        selection,
                    },
                });
                Self::observation_log("Composition", outcome.accepted)
            }
            ComposerInput::Submit {
                revision,
                directive,
                selection_state,
                text,
                selection,
            } => {
                let outcome = self.transition(Transition::Observe(Observation {
                    revision,
                    directive,
                    selection_state,
                    text,
                    selection,
                }));
                if !outcome.accepted {
                    return Self::observation_log("DraftInput", false);
                }
                self.submit()
            }
            ComposerInput::Key {
                key,
                meta_key,
                shift_key,
                ctrl_key,
                alt_key,
                composing,
                key_code,
                revision,
                directive,
                selection_state,
                text,
                selection,
            } => {
                let outcome = self.transition(Transition::Observe(Observation {
                    revision,
                    directive,
                    selection_state,
                    text,
                    selection,
                }));
                if !outcome.accepted {
                    return Self::observation_log("DraftInput", false);
                }
                if self.view.draft.ime.active || composing || key_code == 229 {
                    return vec![];
                }
                if key == "Escape" {
                    if self.history_selection.take().is_some() {
                        return vec![];
                    }
                    if self.active_response {
                        return vec![UiEffect::Deliver {
                            child: PresenterId::Chat,
                            event: UiEvent::Conversation(
                                crate::conversation_presenter::ConversationEvent::CancelCurrent,
                            ),
                        }];
                    }
                    return vec![];
                }
                if !meta_key && !shift_key && !ctrl_key && !alt_key && self.history_key(&key) {
                    self.navigate(&key);
                    return vec![];
                }
                if key == "Enter"
                    && !shift_key
                    && !ctrl_key
                    && !alt_key
                    && (self.send_key == "enter" || meta_key)
                {
                    return self.submit();
                }
                vec![]
            }
            ComposerInput::Microphone { gesture } => self.microphone(gesture),
        }
    }

    fn transition(&mut self, event: Transition) -> Outcome {
        let outcome = self.view.draft.transition(event);
        if outcome.reset_history {
            self.history_selection = None;
        }
        outcome
    }

    fn observation_log(event: &str, accepted: bool) -> Vec<UiEffect> {
        if accepted {
            vec![]
        } else {
            vec![UiEffect::Log(format!(
                "ui: presenter=Composer event={event} ignored=true reason=stale-generation"
            ))]
        }
    }

    fn replace(&mut self, text: String, selection: Selection, focus: bool) {
        self.transition(Transition::Replace {
            text,
            selection,
            focus,
        });
    }

    fn submit(&mut self) -> Vec<UiEffect> {
        if !self.view.ready
            || self.view.draft.text.is_empty()
            || self.operation_busy
            || self.view.draft.ime.active
        {
            return vec![];
        }
        let text = self.view.draft.text.clone();
        let selection = self.view.draft.selection;
        self.history_selection = None;
        self.replace(String::new(), Selection::default(), false);
        self.next_send += 1;
        self.pending.insert(
            self.next_send,
            PendingSend {
                text: text.clone(),
                selection,
                cleared_version: self.view.draft.version,
            },
        );
        vec![
            UiEffect::Spawn(UiTask::Composer(ComposerTask::Send {
                token: self.next_send,
                message: text,
            })),
            UiEffect::Deliver {
                child: PresenterId::Chat,
                event: UiEvent::Conversation(
                    crate::conversation_presenter::ConversationEvent::Sent,
                ),
            },
        ]
    }

    fn history_key(&self, key: &str) -> bool {
        let Selection { start, end } = self.view.draft.selection;
        if start != end || self.history.is_empty() {
            return false;
        }
        match key {
            "ArrowUp" => !self
                .view
                .draft
                .text
                .encode_utf16()
                .take(start)
                .any(|c| c == 10),
            "ArrowDown" => {
                self.history_selection.is_some()
                    && !self
                        .view
                        .draft
                        .text
                        .encode_utf16()
                        .skip(end)
                        .any(|c| c == 10)
            }
            _ => false,
        }
    }

    fn navigate(&mut self, key: &str) {
        let position = self
            .history_selection
            .as_ref()
            .and_then(|selected| self.history.iter().position(|(id, _)| *id == selected.id));
        if key == "ArrowDown" && position == Some(0) {
            let selected = self.history_selection.take().unwrap();
            self.replace(selected.draft, selected.selection, false);
            return;
        }
        let index = if key == "ArrowUp" {
            position.map_or(0, |i| (i + 1).min(self.history.len() - 1))
        } else {
            position.unwrap() - 1
        };
        let (id, text) = self.history[index].clone();
        if let Some(selected) = &mut self.history_selection {
            selected.id = id;
        } else {
            self.history_selection = Some(HistorySelection {
                id,
                draft: self.view.draft.text.clone(),
                selection: self.view.draft.selection,
            });
        }
        let end = text.encode_utf16().count();
        self.replace(text, Selection { start: end, end }, false);
    }

    fn microphone(&mut self, gesture: MicrophoneGesture) -> Vec<UiEffect> {
        if !self.view.ready {
            return vec![];
        }
        let action = match (self.voice_mode.as_str(), gesture) {
            ("toggle", MicrophoneGesture::Click) => {
                if self.view.recording {
                    VoiceAction::Finish
                } else {
                    VoiceAction::Start(crate::speech::SpeechSource::Composer)
                }
            }
            ("pushToTalk", MicrophoneGesture::Press) if !self.held => {
                self.held = true;
                VoiceAction::Start(crate::speech::SpeechSource::Composer)
            }
            ("pushToTalk", MicrophoneGesture::Release) if self.held => {
                self.held = false;
                VoiceAction::Finish
            }
            ("pushToTalk", MicrophoneGesture::Cancel) if self.held => {
                self.held = false;
                VoiceAction::Cancel
            }
            _ => return vec![],
        };
        self.view.recording = matches!(action, VoiceAction::Start(_));
        self.voice_queue.push_back(action);
        self.next_voice().into_iter().collect()
    }
    fn next_voice(&mut self) -> Option<UiEffect> {
        if self.voice_running {
            return None;
        }
        let action = self.voice_queue.pop_front()?;
        self.voice_running = true;
        Some(UiEffect::Spawn(UiTask::Composer(ComposerTask::Voice(
            action,
        ))))
    }

    pub(crate) fn render(&mut self) -> UiEffect {
        self.view.pending_sends = self.pending.len();
        self.view.can_send = self.view.ready
            && !self.view.draft.text.is_empty()
            && !self.operation_busy
            && !self.view.draft.ime.active;
        self.view.bindings.clear();
        if !self.view.draft.ime.active {
            for key in ["Escape", "ArrowUp", "ArrowDown", "Enter"] {
                for meta_key in [false, true] {
                    let prevent = match key {
                        "Escape" => self.history_selection.is_some() || self.active_response,
                        "Enter" => self.send_key == "enter" || meta_key,
                        _ => !meta_key && self.history_key(key),
                    };
                    if prevent {
                        self.view.bindings.push(KeyBinding {
                            key: key.into(),
                            meta_key,
                            shift_key: false,
                            ctrl_key: false,
                            alt_key: false,
                        });
                    }
                }
            }
        }
        UiEffect::ComposerRender(Box::new(self.view.clone()))
    }
}

pub(crate) async fn run(
    state: std::sync::Arc<crate::state::DesktopState>,
    task: ComposerTask,
) -> UiEvent {
    let event = match task {
        ComposerTask::Send { token, message } => ComposerEvent::Sent {
            token,
            result: state
                .ui
                .request(UiView::Chat, UiEvent::SubmitChat(message))
                .await,
        },
        ComposerTask::Voice(action) => ComposerEvent::VoiceCompleted(
            state.ui.request(UiView::Chat, UiEvent::Voice(action)).await,
        ),
    };
    UiEvent::Composer(event)
}

