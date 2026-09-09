use crate::avatar_presenter::AvatarState;
use crate::motion_settings_presenter::{MotionEvent, PreviewRequest};
use crate::status_presenter::UiText;
use crate::ui_events::{PresenterId, UiEffect, UiEvent};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum AvatarSceneInput {
    Mounted {
        hidden: bool,
    },
    Unmounted,
    Visibility {
        hidden: bool,
    },
    Retry,
    Hide,
    Drag {
        button: i16,
    },
    OperationFailed {
        error: String,
    },
    Loaded {
        generation: u64,
        outcome: SceneOutcome,
    },
    Failed {
        generation: u64,
        error: String,
    },
    PreviewApplied {
        generation: u64,
        request_id: u64,
    },
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SceneOutcome {
    Ready,
    Unset,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Dormant,
    Loading,
    Ready,
    Unset,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvatarSceneView {
    pub snapshot: AvatarState,
    pub generation: u64,
    pub load: bool,
    pub status: Option<UiText>,
    pub retryable: bool,
    pub reply_directive: u64,
    pub reply_action: &'static str,
    pub preview_directive: u64,
    pub preview: Option<PreviewRequest>,
    pub drag_request: u64,
    pub emotions: coosenpai_core::emotion::EmotionState,
}
pub(crate) struct AvatarScenePresenter {
    view: AvatarSceneView,
    mounted: bool,
    hidden: bool,
    phase: Phase,
    baseline: bool,
    last_reply: Option<String>,
    pending_reply: bool,
    pending_preview: Option<PreviewRequest>,
    accepted: VecDeque<u64>,
}
impl Default for AvatarScenePresenter {
    fn default() -> Self {
        Self {
            view: AvatarSceneView {
                snapshot: Default::default(),
                generation: 0,
                load: false,
                status: None,
                retryable: false,
                reply_directive: 0,
                reply_action: "none",
                preview_directive: 0,
                preview: None,
                drag_request: 0,
                emotions: Default::default(),
            },
            mounted: false,
            hidden: false,
            phase: Phase::Dormant,
            baseline: false,
            last_reply: None,
            pending_reply: false,
            pending_preview: None,
            accepted: VecDeque::new(),
        }
    }
}
impl AvatarScenePresenter {
    pub(crate) fn observe(&mut self, state: &AvatarState) -> Vec<UiEffect> {
        let model_changed = state.model_revision != self.view.snapshot.model_revision;
        let visible_changed = state.visible != self.view.snapshot.visible;
        self.view.snapshot = state.clone();
        self.view.emotions = if state.emotions_enabled {
            state.emotions
        } else {
            Default::default()
        };
        if !self.baseline {
            self.last_reply = state.reply_id.clone();
            self.baseline = true;
        }
        if self.mounted && !self.hidden {
            if !state.visible && (visible_changed || self.view.load) {
                self.stop();
            } else if state.visible && (visible_changed || model_changed) {
                self.begin();
            }
        }
        if self.last_reply != state.reply_id {
            self.last_reply = state.reply_id.clone();
            if self.hidden {
                self.pending_reply = false;
            } else if state.reply_id.is_none() {
                self.pending_reply = false;
                self.reply("cancel");
            } else if self.mounted && state.visible {
                if self.phase == Phase::Ready {
                    self.reply("reply");
                } else if self.phase == Phase::Loading {
                    self.pending_reply = true;
                }
            }
        }
        if self.mounted {
            vec![self.render()]
        } else {
            vec![]
        }
    }
    pub(crate) fn input(&mut self, input: AvatarSceneInput) -> Vec<UiEffect> {
        let mut effects = vec![];
        match input {
            AvatarSceneInput::Mounted { hidden } => {
                self.mounted = true;
                self.hidden = hidden;
                self.baseline = true;
                self.last_reply = self.view.snapshot.reply_id.clone();
                if self.view.snapshot.visible && !hidden {
                    self.begin();
                } else {
                    self.stop();
                }
            }
            AvatarSceneInput::Unmounted => {
                self.mounted = false;
                self.stop();
                return vec![];
            }
            AvatarSceneInput::Visibility { hidden } => {
                if self.hidden == hidden {
                    return vec![];
                }
                self.hidden = hidden;
                if hidden {
                    self.stop();
                } else if self.mounted && self.view.snapshot.visible {
                    self.begin();
                }
            }
            AvatarSceneInput::Retry if self.mounted && !self.hidden && self.view.retryable => {
                if self.view.snapshot.visible && matches!(self.phase, Phase::Failed | Phase::Unset)
                {
                    self.begin();
                } else {
                    self.view.status = None;
                    self.view.retryable = false;
                }
            }
            AvatarSceneInput::Hide => effects.push(UiEffect::Deliver {
                child: PresenterId::Avatar,
                event: UiEvent::AvatarVisibility(Some(false)),
            }),
            AvatarSceneInput::Drag { button: 0 } => self.view.drag_request += 1,
            AvatarSceneInput::OperationFailed { error } => {
                self.view.status = Some(UiText::literal(error));
                self.view.retryable = true;
            }
            AvatarSceneInput::Loaded {
                generation,
                outcome,
            } if self.current(generation) && self.phase == Phase::Loading => match outcome {
                SceneOutcome::Ready => {
                    self.phase = Phase::Ready;
                    self.view.status = None;
                    if self.pending_reply {
                        self.pending_reply = false;
                        self.reply("reply");
                    }
                    if let Some(request) = self.pending_preview.clone() {
                        self.issue_preview(request);
                    }
                }
                SceneOutcome::Unset => {
                    self.phase = Phase::Unset;
                    self.view.status = Some(UiText::message("avatar.unset"));
                    self.pending_reply = false;
                    self.pending_preview = None;
                }
            },
            AvatarSceneInput::Failed { generation, error } if self.current(generation) => {
                self.phase = Phase::Failed;
                self.view.status = Some(
                    UiText::message("avatar.displayFailed").arg("cause", UiText::literal(error)),
                );
                self.view.retryable = true;
                self.view.load = false;
                self.pending_reply = false;
                self.pending_preview = None;
                self.view.preview = None;
            }
            AvatarSceneInput::PreviewApplied {
                generation,
                request_id,
            } if self.current(generation)
                && !self.hidden
                && self.phase == Phase::Ready
                && self
                    .pending_preview
                    .as_ref()
                    .is_some_and(|p| p.id == request_id) =>
            {
                self.pending_preview = None;
                self.accepted.push_back(request_id);
                if self.accepted.len() > 64 {
                    self.accepted.pop_front();
                }
                effects.push(accepted(request_id));
            }
            _ => return vec![],
        }
        if self.mounted {
            effects.insert(0, self.render());
        }
        effects
    }
    pub(crate) fn motions_changed(&mut self) -> Vec<UiEffect> {
        if self.mounted && self.view.snapshot.visible && !self.hidden {
            self.begin();
            vec![self.render()]
        } else {
            vec![]
        }
    }
    pub(crate) fn preview(&mut self, request: PreviewRequest) -> Vec<UiEffect> {
        if !self.mounted || !self.view.snapshot.visible || self.hidden {
            return vec![];
        }
        if self.accepted.contains(&request.id) {
            return vec![accepted(request.id)];
        }
        if self
            .pending_preview
            .as_ref()
            .is_some_and(|pending| pending.id == request.id)
        {
            return vec![];
        }
        match self.phase {
            Phase::Ready => self.issue_preview(request),
            Phase::Loading => self.pending_preview = Some(request),
            _ => return vec![],
        }
        vec![self.render()]
    }
    fn current(&self, generation: u64) -> bool {
        self.mounted
            && self.view.snapshot.visible
            && !self.hidden
            && generation == self.view.generation
    }
    fn begin(&mut self) {
        self.stop();
        self.phase = Phase::Loading;
        self.view.load = true;
        self.view.status = Some(UiText::message("avatar.loading"));
        self.view.retryable = false;
    }
    fn stop(&mut self) {
        self.view.generation += 1;
        self.phase = Phase::Dormant;
        self.view.load = false;
        self.view.status = None;
        self.view.retryable = false;
        self.pending_reply = false;
        self.pending_preview = None;
        self.view.preview = None;
        self.view.reply_directive += 1;
        self.view.preview_directive += 1;
        self.view.reply_action = "none";
    }
    fn reply(&mut self, action: &'static str) {
        self.view.reply_directive += 1;
        self.view.reply_action = action;
    }
    fn issue_preview(&mut self, request: PreviewRequest) {
        self.pending_preview = Some(request.clone());
        self.view.preview = Some(request);
        self.view.preview_directive += 1;
    }
    fn render(&self) -> UiEffect {
        UiEffect::AvatarSceneRender(Box::new(self.view.clone()))
    }
}
fn accepted(id: u64) -> UiEffect {
    UiEffect::Deliver {
        child: PresenterId::Root,
        event: UiEvent::MotionSettings(MotionEvent::PreviewAccepted(id)),
    }
}

