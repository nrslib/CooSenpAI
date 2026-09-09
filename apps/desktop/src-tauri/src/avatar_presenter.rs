use crate::presentation::{Presentation, PresentationAction, PresentationEvent, PresentationState};
use crate::snapshot::AppSnapshot;
use crate::ui_events::{Handling, PresenterId, UiEffect, UiEvent, ViewCommand};
use coosenpai_core::emotion::EmotionState;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvatarState {
    pub revision: u64,
    pub model_revision: u64,
    pub language: String,
    pub visible: bool,
    pub emotions: EmotionState,
    pub emotions_enabled: bool,
    pub thinking: bool,
    pub reply_id: Option<String>,
    #[serde(skip)]
    pub(crate) conversation_generation: Option<u64>,
    #[serde(skip)]
    pub(crate) observed_reply_id: Option<String>,
}

impl Default for AvatarState {
    fn default() -> Self {
        Self {
            revision: 0,
            model_revision: 0,
            language: "ja".to_owned(),
            visible: false,
            emotions: EmotionState::default(),
            emotions_enabled: true,
            thinking: false,
            reply_id: None,
            conversation_generation: None,
            observed_reply_id: None,
        }
    }
}

impl AvatarState {
    pub(crate) fn project(&mut self, snapshot: &AppSnapshot) -> bool {
        let thinking = snapshot.companion.phase == crate::snapshot::CompanionViewPhase::Thinking;
        let enabled = snapshot.config.companion.emotions_enabled;
        let latest_reply_id = snapshot
            .conversation
            .iter()
            .rev()
            .find(|entry| entry.role == coosenpai_core::state::ConversationRole::Companion)
            .map(|entry| entry.id.clone());
        let generation = Some(snapshot.selected_conversation_generation);
        let reply_id = if self.conversation_generation != generation || latest_reply_id.is_none() {
            // Loading history establishes a baseline, not a newly delivered reply.
            None
        } else if self.observed_reply_id != latest_reply_id {
            latest_reply_id.clone()
        } else {
            self.reply_id.clone()
        };
        self.conversation_generation = generation;
        self.observed_reply_id = latest_reply_id;
        if self.language == snapshot.config.ui.language
            && self.emotions == snapshot.companion_emotions
            && self.emotions_enabled == enabled
            && self.thinking == thinking
            && self.reply_id == reply_id
        {
            return false;
        }
        self.language = snapshot.config.ui.language.clone();
        self.emotions = snapshot.companion_emotions;
        self.emotions_enabled = enabled;
        self.thinking = thinking;
        self.reply_id = reply_id;
        self.revision = self.revision.saturating_add(1);
        true
    }
}

#[derive(Default)]
pub(crate) struct AvatarPresenter {
    state: AvatarState,
    scene: crate::avatar_scene_presenter::AvatarScenePresenter,
    snapshot_revision: Option<u64>,
    presentation: Presentation,
    positioned: bool,
    projection_dirty: bool,
}

impl AvatarPresenter {
    pub(crate) fn new(state: AvatarState) -> Self {
        Self {
            state,
            ..Self::default()
        }
    }

    pub(crate) fn handle(&mut self, event: UiEvent) -> Handling {
        let mut effects = match event {
            UiEvent::AvatarScene(input) => self.scene.input(input),
            UiEvent::AvatarMotionsChanged => self.scene.motions_changed(),
            UiEvent::AvatarPreview(request) => self.scene.preview(request),
            UiEvent::AvatarRead {
                model_changed,
                reply,
            } => {
                if model_changed {
                    self.state.model_revision = self.state.model_revision.saturating_add(1);
                    self.state.revision = self.state.revision.saturating_add(1);
                }
                let _ = reply.send(self.state.clone());
                if model_changed {
                    vec![self.render()]
                } else {
                    Vec::new()
                }
            }
            UiEvent::SnapshotUpdated(snapshot) => {
                if self.snapshot_revision.is_some_and(|revision| {
                    revision > snapshot.revision
                        || (revision == snapshot.revision && !self.projection_dirty)
                }) {
                    return Handling::Handled(vec![]);
                }
                self.snapshot_revision = Some(snapshot.revision);
                if self.state.project(&snapshot) || self.projection_dirty {
                    vec![self.render()]
                } else {
                    Vec::new()
                }
            }
            UiEvent::AvatarRenderFailed { revision, error } => {
                if revision == self.state.revision {
                    self.projection_dirty = true;
                }
                {
                    let mut effects = self.scene.input(
                        crate::avatar_scene_presenter::AvatarSceneInput::OperationFailed {
                            error: error.clone(),
                        },
                    );
                    effects.push(UiEffect::Fail(error));
                    effects
                }
            }
            UiEvent::Mounted(_) => vec![self.render()],
            UiEvent::AvatarWindow(event) => self.present(event),
            UiEvent::AvatarVisibility(requested) => self.visibility(requested),
            UiEvent::Present(ViewCommand::Hide) | UiEvent::Close => self.visibility(Some(false)),
            UiEvent::Present(ViewCommand::Show | ViewCommand::Front) => self.visibility(Some(true)),
            UiEvent::AvatarApplied {
                visible,
                generation,
                result,
            } => {
                if visible {
                    if generation != self.presentation.generation()
                        || self.presentation.state() != PresentationState::Shown
                    {
                        vec![UiEffect::Log(format!("ui: presenter=Avatar event=Applied({generation}) ignored=true reason=stale-generation"))]
                    } else {
                        match result {
                            Ok(()) => {
                                self.positioned = true;
                                self.commit(true)
                            }
                            Err(error) => {
                                self.presentation
                                    .transition::<(), ()>(PresentationEvent::Hide);
                                {
                                    let mut effects = self.scene.input(crate::avatar_scene_presenter::AvatarSceneInput::OperationFailed { error: error.clone() });
                                    effects.push(UiEffect::Fail(error));
                                    effects
                                }
                            }
                        }
                    }
                } else if generation != self.presentation.generation() {
                    vec![UiEffect::Log(format!("ui: presenter=Avatar event=Hidden({generation}) ignored=true reason=stale-generation"))]
                } else {
                    match result {
                        Ok(()) => {
                            self.presentation
                                .transition::<(), ()>(PresentationEvent::Hide);
                            self.commit(false)
                        }
                        Err(error) => {
                            let mut effects = self.scene.input(
                                crate::avatar_scene_presenter::AvatarSceneInput::OperationFailed {
                                    error: error.clone(),
                                },
                            );
                            effects.push(UiEffect::Fail(error));
                            effects
                        }
                    }
                }
            }
            event => return Handling::Bubble(event),
        };
        effects.extend(self.scene.observe(&self.state));
        Handling::Handled(effects)
    }

    fn visibility(&mut self, requested: Option<bool>) -> Vec<UiEffect> {
        let visible = requested.unwrap_or(self.presentation.state() == PresentationState::Hidden);
        let loading = matches!(self.presentation.state(), PresentationState::Loading { .. });
        if visible == self.state.visible && !loading {
            return Vec::new();
        }
        if visible {
            return self.present(PresentationEvent::Open(()));
        }
        if loading {
            self.presentation
                .transition::<(), ()>(PresentationEvent::Hide);
        }
        let generation = self.presentation.generation();
        vec![UiEffect::AvatarVisibility {
            visible: false,
            generation,
            position_initial: false,
            language: self.state.language.clone(),
        }]
    }

    fn present(&mut self, event: PresentationEvent<(), ()>) -> Vec<UiEffect> {
        match self.presentation.transition(event) {
            PresentationAction::Load { generation, .. } => vec![UiEffect::Deliver {
                child: PresenterId::Root,
                event: UiEvent::AvatarWindow(PresentationEvent::Loaded { generation, result: Ok(Some(())) }),
            }],
            PresentationAction::Show(()) => vec![UiEffect::AvatarVisibility {
                visible: true, generation: self.presentation.generation(),
                position_initial: !self.positioned, language: self.state.language.clone(),
            }],
            PresentationAction::Hide | PresentationAction::Unavailable => vec![UiEffect::AvatarVisibility {
                visible: false, generation: self.presentation.generation(),
                position_initial: false, language: self.state.language.clone(),
            }],
            PresentationAction::Failed(error) => { let mut effects = self.scene.input(crate::avatar_scene_presenter::AvatarSceneInput::OperationFailed { error: error.clone() }); effects.push(UiEffect::Fail(error)); effects },
            PresentationAction::Ignored { received, expected } => vec![UiEffect::Log(format!(
                "ui: presenter=Avatar event=Loaded({received}) ignored=true reason=stale-generation expected={expected:?}"))],
        }
    }

    fn commit(&mut self, visible: bool) -> Vec<UiEffect> {
        if self.state.visible == visible {
            return Vec::new();
        }
        self.state.visible = visible;
        self.state.revision = self.state.revision.saturating_add(1);
        vec![self.render()]
    }

    fn render(&mut self) -> UiEffect {
        self.projection_dirty = false;
        UiEffect::AvatarRender(self.state.clone())
    }
}

