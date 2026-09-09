use crate::commands::IpcResult;
use crate::model_catalog::ModelCatalogView;
use crate::snapshot::AppSnapshot;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask};
use coosenpai_core::config::Config;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum ModelPickerInput {
    Provider { value: String },
    Model { value: String },
    Effort { value: String },
    CommitModel,
    CommitEffort,
    Reload,
}

#[derive(Debug)]
pub(crate) enum ModelPickerEvent {
    Input(ModelPickerInput),
    Loaded {
        generation: u64,
        catalog: ModelCatalogView,
        snapshot: Arc<AppSnapshot>,
        merge: bool,
    },
    Saved {
        generation: u64,
        result: Box<IpcResult<Config>>,
    },
}

#[derive(Debug)]
pub(crate) enum ModelPickerTask {
    Load {
        generation: u64,
        merge: bool,
    },
    Save {
        generation: u64,
        patch: serde_json::Value,
    },
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelPickerView {
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    pub disabled: bool,
    pub reloading: bool,
    pub show_reload: bool,
    pub show_claude_help: bool,
    pub opencode_failed: bool,
    pub notice: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub(crate) struct ModelPickerPresenter {
    generation: u64,
    active: bool,
    snapshot: Option<Arc<AppSnapshot>>,
    catalog: Option<ModelCatalogView>,
    confirmed: (String, String, String),
    draft: (String, String, String),
    saving: bool,
    reloading: bool,
    notice: bool,
    error: Option<String>,
}

impl ModelPickerPresenter {
    pub(crate) fn invalidate(&mut self) {
        self.generation += 1;
        self.active = false;
        self.saving = false;
        self.reloading = false;
    }

    pub(crate) fn mount(&mut self) -> Vec<UiEffect> {
        self.active = true;
        vec![self.load(false)]
    }

    pub(crate) fn loaded(
        &mut self,
        snapshot: Arc<AppSnapshot>,
        catalog: ModelCatalogView,
    ) -> Vec<UiEffect> {
        self.invalidate();
        self.active = true;
        self.catalog = Some(catalog);
        self.observe(snapshot);
        self.draft = self.confirmed.clone();
        vec![self.render()]
    }

    pub(crate) fn observe(&mut self, snapshot: Arc<AppSnapshot>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.revision >= snapshot.revision)
        {
            return;
        }
        let config = &snapshot.config.companion;
        let next = (
            config.provider.clone(),
            config.model.clone(),
            config.effort.clone(),
        );
        if next != self.confirmed && !self.saving {
            self.draft = next.clone();
        }
        self.confirmed = next;
        self.snapshot = Some(snapshot);
    }

    pub(crate) fn handle(&mut self, event: ModelPickerEvent) -> Vec<UiEffect> {
        let mut effects = Vec::new();
        match event {
            ModelPickerEvent::Input(input) => {
                if !self.active
                    || self.saving
                    || self
                        .snapshot
                        .as_ref()
                        .is_none_or(|s| s.onboarding.tutorial_active)
                {
                    return effects;
                }
                match input {
                    ModelPickerInput::Provider { value } => {
                        if !["codex", "claude", "opencode"].contains(&value.as_str()) {
                            return effects;
                        }
                        let selected = self
                            .catalog
                            .as_ref()
                            .and_then(|c| c.providers.iter().find(|p| p.provider == value));
                        let model = selected
                            .map(|p| p.default_model.trim().to_owned())
                            .unwrap_or_default();
                        self.draft.0 = value;
                        self.draft.1 = model;
                        self.notice = self.draft.1.is_empty();
                        if !self.notice {
                            effects.extend(self.save_model());
                        }
                    }
                    ModelPickerInput::Model { value } => self.draft.1 = value,
                    ModelPickerInput::Effort { value } => self.draft.2 = value,
                    ModelPickerInput::CommitModel => {
                        if self.draft.1.is_empty() {
                            self.draft.1 = self.confirmed.1.clone();
                        } else {
                            effects.extend(self.save_model());
                        }
                    }
                    ModelPickerInput::CommitEffort => {
                        if self.draft.2 != self.confirmed.2 {
                            effects.push(
                                self.save(serde_json::json!({"companion":{"effort":self.draft.2}})),
                            );
                        }
                    }
                    ModelPickerInput::Reload => {
                        if !self.reloading {
                            effects.push(self.load(true));
                        }
                    }
                }
            }
            ModelPickerEvent::Loaded {
                generation,
                catalog,
                snapshot,
                merge,
            } => {
                if !self.active || generation != self.generation {
                    return effects;
                }
                self.reloading = false;
                if merge {
                    if let Some(current) = &mut self.catalog {
                        if let Some(next) =
                            catalog.providers.iter().find(|p| p.provider == "opencode")
                        {
                            if let Some(provider) = current
                                .providers
                                .iter_mut()
                                .find(|p| p.provider == "opencode")
                            {
                                provider.candidates = next.candidates.clone();
                            }
                            current.opencode_error = catalog.opencode_error;
                        }
                    } else {
                        self.catalog = Some(catalog);
                    }
                } else {
                    self.catalog = Some(catalog);
                }
                self.observe(snapshot.clone());
                effects.push(UiEffect::RenderSnapshot {
                    view: PresenterId::ModelPicker,
                    snapshot,
                });
            }
            ModelPickerEvent::Saved { generation, result } => {
                if !self.active || generation != self.generation {
                    return effects;
                }
                self.saving = false;
                match *result {
                    IpcResult::Success { value, .. } => {
                        self.confirmed = (
                            value.companion.provider,
                            value.companion.model,
                            value.companion.effort,
                        );
                        self.draft = self.confirmed.clone();
                        self.error = None;
                        effects.push(self.load(false));
                    }
                    IpcResult::Failure { error, .. } => {
                        self.draft = self.confirmed.clone();
                        self.error = Some(error.message);
                    }
                }
            }
        }
        effects.insert(0, self.render());
        effects
    }

    fn save_model(&mut self) -> Vec<UiEffect> {
        if (self.draft.0.as_str(), self.draft.1.as_str())
            == (self.confirmed.0.as_str(), self.confirmed.1.as_str())
        {
            return vec![];
        }
        vec![self
            .save(serde_json::json!({"companion":{"provider":self.draft.0,"model":self.draft.1}}))]
    }

    fn save(&mut self, patch: serde_json::Value) -> UiEffect {
        self.generation += 1;
        self.saving = true;
        self.reloading = false;
        self.notice = false;
        self.error = None;
        UiEffect::Spawn(UiTask::ModelPicker(ModelPickerTask::Save {
            generation: self.generation,
            patch,
        }))
    }

    fn load(&mut self, merge: bool) -> UiEffect {
        self.generation += 1;
        self.reloading = true;
        UiEffect::Spawn(UiTask::ModelPicker(ModelPickerTask::Load {
            generation: self.generation,
            merge,
        }))
    }

    pub(crate) fn render(&self) -> UiEffect {
        let provider = self
            .catalog
            .as_ref()
            .and_then(|c| c.providers.iter().find(|p| p.provider == self.draft.0));
        let mut models = Vec::new();
        if let Some(provider) = provider {
            for model in provider
                .candidates
                .iter()
                .chain(&provider.history)
                .chain(std::iter::once(&self.draft.1))
            {
                let model = model.trim().to_owned();
                if !model.is_empty() && !models.contains(&model) {
                    models.push(model);
                }
            }
        }
        let efforts = provider
            .map(|p| {
                p.model_efforts
                    .get(&self.draft.1)
                    .filter(|v| !v.is_empty())
                    .unwrap_or(&p.efforts)
                    .clone()
            })
            .unwrap_or_default();
        UiEffect::ModelPickerRender(Box::new(ModelPickerView {
            provider: self.draft.0.clone(),
            model: self.draft.1.clone(),
            effort: self.draft.2.clone(),
            providers: ["codex", "claude", "opencode"].map(str::to_owned).to_vec(),
            models,
            efforts,
            disabled: self.saving
                || self
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.onboarding.tutorial_active),
            reloading: self.reloading,
            show_reload: self.draft.0 == "opencode",
            show_claude_help: self.draft.0 == "claude",
            opencode_failed: self
                .catalog
                .as_ref()
                .is_some_and(|c| c.opencode_error.is_some()),
            notice: self.notice,
            error: self.error.clone(),
        }))
    }
}

pub(crate) async fn run(state: Arc<crate::state::DesktopState>, task: ModelPickerTask) -> UiEvent {
    let event = match task {
        ModelPickerTask::Load { generation, merge } => {
            let catalog = if merge {
                crate::model_catalog::reload_opencode_models(&state).await
            } else {
                crate::model_catalog::catalog_for_state(&state).await
            };
            ModelPickerEvent::Loaded {
                generation,
                catalog,
                snapshot: Arc::new(state.snapshot().await),
                merge,
            }
        }
        ModelPickerTask::Save { generation, patch } => ModelPickerEvent::Saved {
            generation,
            result: Box::new(
                crate::commands::update_config_for_source(
                    state,
                    patch,
                    None,
                    None,
                    crate::command_guard::CommandSource::IpcModelPopup,
                )
                .await,
            ),
        },
    };
    UiEvent::ModelPicker(event)
}

