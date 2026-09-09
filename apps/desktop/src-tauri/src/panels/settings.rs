use super::*;
use coosenpai_core::locale::{text, Locale, TextKey};
use serde_json::json;

#[path = "settings_fields.rs"]
mod fields;
#[path = "settings_patch.rs"]
mod patch;
use fields::Fields;

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Draft {
    basis: FormBasis,
    fields: Fields,
    revision: u64,
    avatar_image: Option<Vec<u8>>,
    avatar_file_name: Option<String>,
}
#[derive(Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct FormBasis {
    config_revision: u64,
    generation: u64,
    fields: Fields,
    avatar_image: Option<Vec<u8>>,
    avatar_file_name: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Observation {
    config: Value,
    fields: Fields,
    config_revision: u64,
    avatar_image_load_failed: bool,
    issues: Vec<Value>,
    onboarding: Value,
    focus_section: Option<String>,
}
#[derive(Deserialize)]
struct ConfigResult {
    config: Value,
    fields: Fields,
}

#[derive(Deserialize)]
struct Initial {
    observation: Observation,
    draft: Draft,
}

#[derive(Default)]
pub(super) struct SettingsPresenter {
    base: Value,
    base_fields: Fields,
    base_revision: u64,
    draft: Draft,
    last_input: Draft,
    reflection_generation: u64,
    avatar_failed: bool,
    saving: Option<u64>,
    submitted: Option<Draft>,
    issues: Vec<Value>,
    external: Option<Vec<String>>,
    saved: bool,
    saved_timer: Option<u64>,
    category: Option<String>,
    highlight: Option<String>,
    tutorial_persona: bool,
    picker_allowed: bool,
    picker_open: bool,
    discard: bool,
    confirmation: Option<String>,
    recording: Option<String>,
    persona: Option<Value>,
    persona_load: Option<u64>,
    vrm_open: bool,
    closing: Option<u64>,
    closed: bool,
    activity: ChildActivity,
}

impl SettingsPresenter {
    pub(super) fn closing(&self) -> bool {
        self.closed || self.closing.is_some()
    }
    pub(super) fn allows_child(&self, kind: PanelKind) -> bool {
        !self.closing()
            && match kind {
                PanelKind::Persona => self.persona.is_some(),
                PanelKind::Vrm => self.vrm_open,
                _ => true,
            }
    }
    pub(super) fn blocks_close(&self) -> bool {
        self.saving.is_some() || self.closing.is_some()
    }
    fn escape_enabled(&self) -> bool {
        if self.closed {
            false
        } else if self.recording.is_some() {
            true
        } else if self.persona.is_some() {
            !self.activity.persona
        } else if self.discard || self.confirmation.is_some() {
            true
        } else if self.vrm_open {
            !self.activity.vrm
        } else {
            !self.blocks_close() && !self.activity.busy()
        }
    }
    pub(super) fn handle(
        &mut self,
        event: PanelEvent,
        io: &mut PanelIo,
        activity: ChildActivity,
    ) -> Result<Value, String> {
        self.activity = activity;
        match event {
            PanelEvent::Mount { value } => {
                let initial: Initial = decode(value)?;
                if initial.draft.basis.generation != 0
                    || initial.draft.basis.config_revision != initial.observation.config_revision
                {
                    return Err("初期フォームの設定世代が一致しません".into());
                }
                self.last_input = initial.draft.clone();
                self.draft = initial.draft;
                self.observe(initial.observation, io, true)?;
                io.command("focusFirst", ());
            }
            PanelEvent::Change { value } if !self.closed && self.closing.is_none() => {
                self.accept_draft(decode(value)?, io)?;
            }
            PanelEvent::Action { name, value } if !self.closed => self.action(&name, value, io)?,
            PanelEvent::Completed { id, result } => {
                if let Some(command) = io.completed(id) {
                    self.complete(command, result, io)?;
                }
            }
            _ => {}
        }
        Ok(self.view())
    }
    pub(super) fn refresh_children(&mut self, activity: ChildActivity) -> Value {
        self.activity = activity;
        self.view()
    }
    fn view(&self) -> Value {
        json!({"dirty":self.dirty(),"saving":self.saving.is_some(),"saved":self.saved,
            "issues":self.issues,"externalChanges":self.external,"discardConfirmOpen":self.discard,
            "confirmation":self.confirmation,"activeCategory":self.category.as_deref().unwrap_or("general"),
            "recordingShortcut":self.recording,"personaDocument":self.persona,"personaPickerOpen":self.picker_open && self.picker_allowed,
            "personaPickerAllowed":self.picker_allowed,"showTutorialPersonaSettings":self.tutorial_persona,
            "vrmControlsOpen":self.vrm_open,"closing":self.closing.is_some(),"escapeEnabled":self.escape_enabled()})
    }
    fn accept_draft(&mut self, input: Draft, io: &mut PanelIo) -> Result<(), String> {
        if input.revision <= self.last_input.revision {
            return Ok(());
        }
        let basis = &input.basis;
        let previous_basis = &self.last_input.basis;
        if basis.generation < previous_basis.generation
            || basis.generation > self.reflection_generation
            || basis.config_revision < previous_basis.config_revision
            || basis.config_revision > self.base_revision
            || (basis.generation == previous_basis.generation && basis != previous_basis)
        {
            return Err("フォームの反映世代または基準値が不正です".into());
        }
        let (previous, image, name) = if basis.generation == previous_basis.generation {
            (
                &self.last_input.fields,
                &self.last_input.avatar_image,
                &self.last_input.avatar_file_name,
            )
        } else {
            (&basis.fields, &basis.avatar_image, &basis.avatar_file_name)
        };
        fields::check_shape(&input.fields, &self.base_fields)?;
        fields::check_shape(&basis.fields, &self.base_fields)?;
        self.draft
            .fields
            .extend(fields::changed(previous, &input.fields));
        if image != &input.avatar_image || name != &input.avatar_file_name {
            self.draft.avatar_image = input.avatar_image.clone();
            self.draft.avatar_file_name = input.avatar_file_name.clone();
        }
        self.draft.revision = input.revision;
        self.draft.basis = input.basis.clone();
        self.saved = false;
        self.last_input = input;
        if self.draft.fields != self.last_input.fields
            || self.draft.avatar_image != self.last_input.avatar_image
            || self.draft.avatar_file_name != self.last_input.avatar_file_name
        {
            self.reflect(io);
        }
        Ok(())
    }
    fn candidate(&self) -> Value {
        fields::candidate(&self.base, &self.base_fields, &self.draft.fields)
    }
    fn dirty(&self) -> bool {
        self.draft.avatar_image.is_some() || self.draft.fields != self.base_fields
    }
    fn observe(
        &mut self,
        observation: Observation,
        io: &mut PanelIo,
        initial: bool,
    ) -> Result<(), String> {
        fields::check_shape(&observation.fields, &self.draft.fields)?;
        let highlight = observation.focus_section.or_else(|| {
            observation.onboarding["settingsHighlight"]
                .as_str()
                .map(String::from)
        });
        if initial || highlight != self.highlight {
            match highlight.as_deref() {
                Some("watch") => self.category = Some("vision".into()),
                Some("persona") => self.category = Some("general".into()),
                _ => {}
            }
            self.highlight = highlight;
        }
        let tutorial = observation.onboarding["tutorialActive"] == true;
        let persona_step = observation.onboarding["currentStep"] == "persona";
        let tutorial_persona =
            tutorial && persona_step && observation.onboarding["settingsHighlight"] == "persona";
        self.picker_allowed = !tutorial || persona_step;
        if tutorial_persona && !self.tutorial_persona {
            self.picker_open = true;
        }
        if !self.picker_allowed {
            self.picker_open = false;
        }
        self.tutorial_persona = tutorial_persona;
        if (initial || !self.dirty() && self.saving.is_none())
            && observation.config_revision >= self.base_revision
        {
            self.base = observation.config;
            self.base_fields = observation.fields;
            self.base_revision = observation.config_revision;
            self.avatar_failed = observation.avatar_image_load_failed;
            self.issues = observation.issues;
            if !initial {
                self.draft.fields = self.base_fields.clone();
                self.reflect(io);
            }
        }
        Ok(())
    }
    fn action(&mut self, name: &str, value: Value, io: &mut PanelIo) -> Result<(), String> {
        if self.closing.is_some() {
            return Ok(());
        }
        if matches!(name, "save" | "close") && !value.is_null() {
            self.accept_draft(decode(value.clone())?, io)?;
        }
        match name {
            "snapshot" => self.observe(decode(value)?, io, false)?,
            "save" if self.saving.is_none() && self.closing.is_none() && self.dirty() => {
                let mut patch = patch::changed(&self.base, &self.candidate());
                if self.draft.avatar_image.is_some() {
                    patch = patch::merge(&patch, &json!({"ui":{"avatarPath":"state/avatar.png"}}));
                }
                self.submitted = Some(self.draft.clone());
                self.saving = Some(io.command("save", json!({"patch":patch,"avatarImage":self.draft.avatar_image,"baseConfigRevision":self.base_revision})));
            }
            "save" => {}
            "close" => self.request_close(io),
            "discard" if !self.blocks_close() && !self.activity.busy() => {
                self.discard = false;
                self.clear_and_close(io);
            }
            "discard" => {}
            "cancelDiscard" => self.discard = false,
            "escape" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Key {
                    key: String,
                    composing: bool,
                    key_code: u32,
                }
                if let Some(draft) = value.get("draft") {
                    self.accept_draft(decode(draft.clone())?, io)?;
                }
                let key: Key = decode(value)?;
                if key.key == "Escape"
                    && !key.composing
                    && key.key_code != 229
                    && self.escape_enabled()
                {
                    if self.recording.is_some() {
                        self.recording = None;
                    } else if self.persona.is_some() {
                        self.persona = None;
                        self.persona_load = None;
                    } else if self.discard {
                        self.discard = false;
                    } else if self.confirmation.is_some() {
                        self.confirmation = None;
                    } else if self.vrm_open {
                        self.vrm_open = false;
                    } else {
                        self.request_close(io);
                    }
                }
            }
            "recording" => self.recording = decode(value)?,
            "category" => {
                let category: String = decode(value)?;
                if ![
                    "work",
                    "general",
                    "vision",
                    "hearing",
                    "speech",
                    "notifications",
                    "providers",
                    "shortcuts",
                    "setup",
                    "beta",
                ]
                .contains(&category.as_str())
                {
                    return Err(action_error(&category));
                }
                self.category = Some(category);
            }
            "focusIssue" => {
                let path: String = decode(value)?;
                self.category = Some(patch::category_for_issue(&path).into());
                io.command("focusIssue", path);
            }
            "issues" => self.issues = decode(value)?,
            "openVrm" => self.vrm_open = true,
            "closeVrm" if !self.activity.vrm => self.vrm_open = false,
            "closeVrm" => {}
            "openPicker" if self.picker_allowed => self.picker_open = true,
            "openPicker" => {}
            "closePicker" => self.picker_open = false,
            "editPersona" => {
                self.persona_load = Some(io.command("loadPersona", value));
            }
            "closePersona" if !self.activity.persona => {
                self.persona = None;
                self.persona_load = None;
            }
            "closePersona" => {}
            "selectPersona" if self.saving.is_none() && self.closing.is_none() => {
                self.submitted = Some(self.draft.clone());
                self.saving = Some(io.command("selectPersona", value));
            }
            "selectPersona" => {}
            "confirm" => {
                let confirmation: String = decode(value)?;
                if !["tuning", "conversation-reset"].contains(&confirmation.as_str()) {
                    return Err(action_error(&confirmation));
                }
                self.confirmation = Some(confirmation);
            }
            "cancelConfirmation" => self.confirmation = None,
            "acceptConfirmation" if self.saving.is_none() => {
                match self.confirmation.take().as_deref() {
                    Some("tuning") => {
                        self.issues.clear();
                        io.command("resetTuning", ());
                    }
                    Some("conversation-reset") => {
                        io.command("resetConversation", ());
                    }
                    _ => {}
                }
            }
            "acceptConfirmation" => {}
            _ => return Err(action_error(name)),
        }
        Ok(())
    }
    fn request_close(&mut self, io: &mut PanelIo) {
        if self.blocks_close() || self.activity.busy() {
            return;
        }
        if self.dirty() {
            self.discard = true;
        } else {
            self.clear_and_close(io);
        }
    }
    fn clear_and_close(&mut self, io: &mut PanelIo) {
        self.closing = Some(io.command("clearPreview", ()));
    }
    fn reflect(&mut self, io: &mut PanelIo) {
        self.reflection_generation += 1;
        io.command("reflect", json!({"config":self.candidate(),"revision":self.draft.revision,
            "fields":self.draft.fields,
            "configRevision":self.base_revision,"generation":self.reflection_generation,
            "avatarImage":self.draft.avatar_image,"avatarFileName":self.draft.avatar_file_name,"avatarImageLoadFailed":self.avatar_failed}));
    }
    fn complete(
        &mut self,
        command: PanelCommand,
        result: IoResult,
        io: &mut PanelIo,
    ) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        match command.kind.as_str() {
            "save" | "selectPersona" if self.saving == Some(command.id) => {
                if result.ok {
                    let submitted = self.submitted.take().expect("saving has a draft");
                    let saved: ConfigResult = decode(result.value)?;
                    fields::check_shape(&saved.fields, &self.base_fields)?;
                    let mut remaining = if command.kind == "selectPersona" {
                        fields::changed(&self.base_fields, &self.draft.fields)
                    } else {
                        fields::changed(&submitted.fields, &self.draft.fields)
                    };
                    if command.kind == "selectPersona" {
                        remaining.remove("persona");
                    }
                    self.base_revision = saved.config["revision"]
                        .as_u64()
                        .ok_or("設定の revision がありません")?;
                    self.base = saved.config;
                    self.base_fields = saved.fields;
                    self.draft.fields = self.base_fields.clone();
                    self.draft.fields.extend(remaining);
                    if command.kind == "save" && self.draft.avatar_image == submitted.avatar_image {
                        if submitted.avatar_image.is_some() {
                            self.avatar_failed = false;
                        }
                        self.draft.avatar_image = None;
                        self.draft.avatar_file_name = None;
                    }
                    self.issues = decode(result.issues.unwrap_or_else(|| json!([])))?;
                    self.external = None;
                    self.saving = None;
                    self.saved = !self.dirty();
                    if self.saved {
                        self.saved_timer = Some(io.command("savedDelay", 1500));
                    }
                    self.reflect(io);
                } else {
                    let conflict = [Locale::Ja, Locale::En].iter().any(|locale| {
                        result.message() == text(TextKey::ConfigRevisionConflict, *locale)
                    });
                    self.set_failure(&result, "config");
                    if command.kind == "save" && conflict {
                        self.saving = Some(io.command("reloadConflict", ()));
                    } else {
                        self.saving = None;
                        self.submitted = None;
                    }
                }
            }
            "reloadConflict" if self.saving == Some(command.id) => {
                self.saving = None;
                self.submitted = None;
                if result.ok {
                    let latest: ConfigResult = decode(result.value)?;
                    fields::check_shape(&latest.fields, &self.base_fields)?;
                    let local = fields::changed(&self.base_fields, &self.draft.fields);
                    self.external = Some(patch::paths(&patch::changed(&self.base, &latest.config)));
                    self.base_revision = latest.config["revision"]
                        .as_u64()
                        .ok_or("設定の revision がありません")?;
                    self.base = latest.config;
                    self.base_fields = latest.fields;
                    self.draft.fields = self.base_fields.clone();
                    self.draft.fields.extend(local);
                    self.reflect(io);
                    self.issues = vec![json!({"path":"config","message":self.conflict_message()})];
                } else {
                    self.issues = vec![
                        json!({"path":"config","message":format!("{} {}",self.conflict_message(),result.message())}),
                    ];
                }
            }
            "clearPreview" if self.closing == Some(command.id) => {
                self.closing = None;
                if result.ok {
                    self.closed = true;
                    io.command("close", ());
                } else {
                    self.set_failure(&result, "ui");
                }
            }
            "savedDelay" if self.saved_timer == Some(command.id) => {
                self.saved = false;
                self.saved_timer = None;
            }
            "loadPersona" if self.persona_load == Some(command.id) => {
                self.persona_load = None;
                if result.ok {
                    self.persona = Some(result.value);
                } else {
                    self.set_failure(&result, "companion.persona");
                }
            }
            "resetConversation" if !result.ok => self.set_failure(&result, "config"),
            _ => {}
        }
        Ok(())
    }
    fn set_failure(&mut self, result: &IoResult, path: &str) {
        self.issues = result
            .error
            .as_ref()
            .and_then(|e| e.issues.as_ref())
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![json!({"path":path,"message":result.message()})]);
    }
    fn conflict_message(&self) -> &'static str {
        text(
            TextKey::ConfigRevisionConflict,
            Locale::from_config(self.base["ui"]["language"].as_str().unwrap_or("ja")),
        )
    }
}
