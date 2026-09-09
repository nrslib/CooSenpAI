use crate::status_presenter::UiText;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask, UiView};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MotionSlot {
    Idle,
    Reply,
    Thinking,
    Joy,
    Embarrassment,
    Concern,
    Surprise,
    Curiosity,
    Frustration,
}
impl MotionSlot {
    fn text(self) -> UiText {
        UiText::message(match self {
            Self::Idle => "motions.slots.idle",
            Self::Reply => "motions.slots.reply",
            Self::Thinking => "motions.slots.thinking",
            Self::Joy => "motions.slots.joy",
            Self::Embarrassment => "motions.slots.embarrassment",
            Self::Concern => "motions.slots.concern",
            Self::Surprise => "motions.slots.surprise",
            Self::Curiosity => "motions.slots.curiosity",
            Self::Frustration => "motions.slots.frustration",
        })
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum MotionSelection {
    Builtin { id: BuiltinMotion },
    File { name: String },
    None,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum BuiltinMotion {
    Idle,
    Reply,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PreviewRequest {
    pub id: u64,
    pub slot: MotionSlot,
}
#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MotionInput {
    Mounted,
    Unmounted,
    Select {
        slot: MotionSlot,
        value: String,
    },
    Reset {
        slot: MotionSlot,
    },
    Preview {
        slot: MotionSlot,
    },
    Import {
        slot: MotionSlot,
        file_id: String,
        name: String,
        size: usize,
        consent: bool,
    },
    StorageLoaded {
        generation: u64,
        settings: Option<BTreeMap<MotionSlot, MotionSelection>>,
        error: Option<String>,
    },
    StorageSaved {
        generation: u64,
        error: Option<String>,
    },
}
#[derive(Debug)]
pub(crate) enum MotionEvent {
    Input(MotionInput),
    Shown {
        id: u64,
        result: Result<Option<String>, String>,
    },
    PreviewTick(u64),
    PreviewAccepted(u64),
}
#[derive(Debug)]
pub(crate) enum MotionTask {
    Show(u64),
    PreviewDelay { id: u64, millis: u64 },
}
#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum MotionStorage {
    Load {
        generation: u64,
    },
    Save {
        generation: u64,
        slot: MotionSlot,
        selection: MotionSelection,
    },
    Import {
        generation: u64,
        slot: MotionSlot,
        file_id: String,
    },
    Release {
        file_id: String,
    },
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MotionView {
    pub previewable: Vec<MotionSlot>,
    pub settings: Option<BTreeMap<MotionSlot, MotionSelection>>,
    pub busy: bool,
    pub error: Option<UiText>,
    pub status: Option<UiText>,
}
struct Preview {
    request: PreviewRequest,
    deadline: Option<Instant>,
}
#[derive(Default)]
pub(crate) struct MotionSettingsPresenter {
    view: MotionView,
    mounted: bool,
    generation: u64,
    next_preview: u64,
    preview: Option<Preview>,
    saved_slot: Option<MotionSlot>,
    pending_saves: BTreeSet<u64>,
}
impl MotionSettingsPresenter {
    pub(crate) fn handle(&mut self, event: MotionEvent, now: Instant) -> Vec<UiEffect> {
        let mut effects = match event {
            MotionEvent::Input(input) => self.input(input),
            MotionEvent::Shown { id, result } => {
                let Some(preview) = self.preview.as_mut().filter(|p| p.request.id == id) else {
                    return vec![];
                };
                match result {
                    Ok(_) => {
                        preview.deadline = Some(now + Duration::from_secs(5));
                        vec![send_preview(preview.request.clone()), delay(id, 500)]
                    }
                    Err(error) => {
                        self.preview = None;
                        self.view.busy = false;
                        self.view.error = Some(UiText::literal(error));
                        vec![]
                    }
                }
            }
            MotionEvent::PreviewTick(id) => {
                let Some(preview) = self.preview.as_ref().filter(|p| p.request.id == id) else {
                    return vec![];
                };
                let Some(deadline) = preview.deadline else {
                    return vec![];
                };
                if now >= deadline {
                    self.preview = None;
                    self.view.busy = false;
                    self.view.error = Some(UiText::message("vrm.errors.motion.avatarNotReady"));
                    vec![]
                } else {
                    vec![
                        send_preview(preview.request.clone()),
                        delay(id, (deadline - now).as_millis().min(500) as u64),
                    ]
                }
            }
            MotionEvent::PreviewAccepted(id) => {
                let Some(preview) = self.preview.as_ref().filter(|p| p.request.id == id) else {
                    return vec![];
                };
                self.view.status = Some(
                    UiText::message("motions.previewing").arg("slot", preview.request.slot.text()),
                );
                self.preview = None;
                self.view.busy = false;
                vec![]
            }
        };
        if self.mounted {
            effects.insert(0, self.render());
        }
        effects
    }
    fn input(&mut self, input: MotionInput) -> Vec<UiEffect> {
        match input {
            MotionInput::Mounted => {
                self.mounted = true;
                self.generation += 1;
                self.view.busy = true;
                self.view.error = None;
                self.preview = None;
                self.saved_slot = None;
                vec![storage(MotionStorage::Load {
                    generation: self.generation,
                })]
            }
            MotionInput::Unmounted => {
                self.mounted = false;
                self.generation += 1;
                self.preview = None;
                self.saved_slot = None;
                vec![]
            }
            MotionInput::StorageLoaded {
                generation,
                settings,
                error,
            } => {
                if !self.mounted || generation != self.generation {
                    return vec![];
                }
                self.view.busy = false;
                if let Some(error) = error {
                    self.view.error = Some(
                        UiText::message("motions.readFailed").arg("error", UiText::literal(error)),
                    );
                } else if let Some(settings) = settings {
                    self.view.previewable = settings
                        .iter()
                        .filter_map(|(slot, selection)| {
                            (!matches!(selection, MotionSelection::None)).then_some(*slot)
                        })
                        .collect();
                    self.view.settings = Some(settings);
                    self.view.error = None;
                    if let Some(slot) = self.saved_slot.take() {
                        self.view.status =
                            Some(UiText::message("motions.saved").arg("slot", slot.text()));
                    }
                }
                vec![]
            }
            MotionInput::StorageSaved { generation, error } => {
                if !self.pending_saves.remove(&generation) {
                    return vec![];
                }
                let mut effects = if error.is_none() {
                    vec![root(UiEvent::AvatarMotionsChanged)]
                } else {
                    vec![]
                };
                if self.mounted && generation == self.generation {
                    if let Some(error) = error {
                        self.view.busy = false;
                        self.saved_slot = None;
                        self.view.error = Some(
                            UiText::message("motions.changeFailed")
                                .arg("error", UiText::literal(error)),
                        );
                    } else {
                        effects.push(storage(MotionStorage::Load { generation }));
                    }
                }
                effects
            }
            MotionInput::Select { slot, value } if self.available() => {
                let selection = match value.as_str() {
                    "idle" => MotionSelection::Builtin {
                        id: BuiltinMotion::Idle,
                    },
                    "reply" => MotionSelection::Builtin {
                        id: BuiltinMotion::Reply,
                    },
                    "none" if slot != MotionSlot::Idle => MotionSelection::None,
                    _ => return vec![],
                };
                self.save(slot, selection)
            }
            MotionInput::Reset { slot } if self.available() => self.save(
                slot,
                match slot {
                    MotionSlot::Idle => MotionSelection::Builtin {
                        id: BuiltinMotion::Idle,
                    },
                    MotionSlot::Reply => MotionSelection::Builtin {
                        id: BuiltinMotion::Reply,
                    },
                    _ => MotionSelection::None,
                },
            ),
            MotionInput::Import {
                slot,
                file_id,
                name,
                size,
                consent,
            } => {
                if !self.available() {
                    return vec![storage(MotionStorage::Release { file_id })];
                }
                let error = if !consent {
                    Some("motions.consent")
                } else if !name.to_lowercase().ends_with(".vrma") {
                    Some("vrm.errors.motion.fileExtension")
                } else if size > 8 * 1024 * 1024 {
                    Some("vrm.errors.motion.tooLarge")
                } else if size == 0 || name.len() > 255 {
                    Some("vrm.errors.motion.invalidSettings")
                } else {
                    None
                };
                if let Some(key) = error {
                    self.view.error = Some(
                        UiText::message("motions.importFailed").arg("error", UiText::message(key)),
                    );
                    return vec![storage(MotionStorage::Release { file_id })];
                }
                self.begin_save(slot);
                vec![storage(MotionStorage::Import {
                    generation: self.generation,
                    slot,
                    file_id,
                })]
            }
            MotionInput::Preview { slot }
                if self.available()
                    && self
                        .view
                        .settings
                        .as_ref()
                        .and_then(|settings| settings.get(&slot))
                        .is_some_and(|selection| !matches!(selection, MotionSelection::None)) =>
            {
                self.next_preview += 1;
                self.view.busy = true;
                self.view.error = None;
                self.view.status = None;
                self.preview = Some(Preview {
                    request: PreviewRequest {
                        id: self.next_preview,
                        slot,
                    },
                    deadline: None,
                });
                vec![UiEffect::Spawn(UiTask::MotionSettings(MotionTask::Show(
                    self.next_preview,
                )))]
            }
            _ => vec![],
        }
    }
    fn available(&self) -> bool {
        self.mounted && !self.view.busy && self.view.settings.is_some()
    }
    fn begin_save(&mut self, slot: MotionSlot) {
        self.generation += 1;
        self.pending_saves.insert(self.generation);
        self.view.busy = true;
        self.view.error = None;
        self.view.status = None;
        self.saved_slot = Some(slot);
    }
    fn save(&mut self, slot: MotionSlot, selection: MotionSelection) -> Vec<UiEffect> {
        self.begin_save(slot);
        vec![storage(MotionStorage::Save {
            generation: self.generation,
            slot,
            selection,
        })]
    }
    fn render(&self) -> UiEffect {
        UiEffect::MotionSettingsRender(Box::new(self.view.clone()))
    }
}
fn storage(command: MotionStorage) -> UiEffect {
    UiEffect::MotionStorage(command)
}
fn root(event: UiEvent) -> UiEffect {
    UiEffect::Deliver {
        child: PresenterId::Root,
        event,
    }
}
fn send_preview(request: PreviewRequest) -> UiEffect {
    root(UiEvent::AvatarPreview(request))
}
fn delay(id: u64, millis: u64) -> UiEffect {
    UiEffect::Spawn(UiTask::MotionSettings(MotionTask::PreviewDelay {
        id,
        millis,
    }))
}
pub(crate) async fn run(
    state: std::sync::Arc<crate::state::DesktopState>,
    task: MotionTask,
) -> UiEvent {
    let event = match task {
        MotionTask::Show(id) => MotionEvent::Shown {
            id,
            result: state
                .ui
                .request(UiView::Settings, UiEvent::AvatarVisibility(Some(true)))
                .await,
        },
        MotionTask::PreviewDelay { id, millis } => {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            MotionEvent::PreviewTick(id)
        }
    };
    UiEvent::MotionSettings(event)
}

