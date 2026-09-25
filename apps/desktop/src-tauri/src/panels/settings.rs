use super::*;
use crate::commands_speaker::SpeakerDirectory;
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
    #[serde(default)]
    speaker_directory_revision: u64,
    config: Value,
    fields: Fields,
    config_revision: u64,
    avatar_image_load_failed: bool,
    issues: Vec<Value>,
    onboarding: Value,
    focus_section: Option<String>,
    focus_generation: u64,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetConfirmation {
    scope: String,
    category: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetAcceptance {
    fields: Fields,
}

#[derive(Clone, Default)]
struct ResetUndo {
    fields: Fields,
    scope: String,
    category: Option<String>,
}

const SETTINGS_CATEGORIES: &[&str] = &[
    "work",
    "general",
    "vision",
    "hearing",
    "speech",
    "notifications",
    "providers",
    "shortcuts",
    "setup",
    "developer",
];

// 話者管理の動作状態。統合・取消・再登録・削除・表示名の可否判断と
// busy・成功失敗・確認の遷移はここに置き、View は候補の描画と操作イベントだけを持つ。
#[derive(Default)]
struct SpeakerManagement {
    revision: u64,
    visible: bool,
    directory: Option<SpeakerDirectory>,
    directory_loading: Option<u64>,
    directory_error: Option<String>,
    busy: Option<u64>,
    succeeded: bool,
    error: Option<String>,
    delete_target: Option<String>,
    draft: BTreeMap<String, String>,
}

impl SpeakerManagement {
    fn field(&self, key: &str) -> &str {
        self.draft.get(key).map(String::as_str).unwrap_or("")
    }
    fn selected(&self, key: &str) -> bool {
        self.directory
            .as_ref()
            .is_some_and(|d| d.has_speaker(self.field(key)))
    }
    fn controls(&self) -> Value {
        let idle = self.busy.is_none();
        let rename = self.field("renameID");
        let existing = self
            .directory
            .as_ref()
            .and_then(|d| d.speakers.iter().find(|s| s.id == rename))
            .and_then(|s| s.name.as_deref());
        let name = self.field("nameDraft").trim();
        json!({
            "draft": self.draft,
            "mergeAvailable": self.directory.as_ref().is_some_and(|d| d.speakers.len() >= 2),
            "undoAvailable": self.directory.as_ref().is_some_and(|d| !d.merged.is_empty()),
            "canMerge": idle && self.selected("mergeFrom") && self.selected("mergeTo") && self.field("mergeFrom") != self.field("mergeTo"),
            "canUndo": idle && self.directory.as_ref().is_some_and(|d| d.has_merged_source(self.field("undoSource"))),
            "canRename": idle && self.selected("renameID") && !name.is_empty() && Some(name) != existing && coosenpai_core::speaker_names::validate_speaker_name(name).is_ok(),
            "canClear": idle && self.selected("renameID") && existing.is_some(),
            "canEditName": idle && self.selected("renameID"),
            "canReregister": idle && self.selected("reregisterID"),
            "canDelete": idle && self.selected("deleteID"),
        })
    }
    fn directory_of(&self) -> Result<&SpeakerDirectory, String> {
        self.directory
            .as_ref()
            .ok_or("話者一覧をまだ読み込めていません".into())
    }
    fn run(&mut self, io: &mut PanelIo, kind: &str, payload: Value) -> Result<(), String> {
        if self.busy.is_some() {
            return Err("話者の変更処理が進行中です".into());
        }
        {
            self.error = None;
            self.succeeded = false;
            self.busy = Some(io.command(kind, payload));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DebugWakeSelection {
    name: String,
    image: Vec<u8>,
}

#[derive(Default)]
struct DebugWake {
    selection: Option<DebugWakeSelection>,
    context: String,
    loading: bool,
    command: Option<u64>,
    result: Option<Value>,
    error: Option<String>,
}

impl DebugWake {
    fn phase(&self) -> &'static str {
        if self.loading {
            return "loading";
        }
        if self.command.is_some() {
            return "processing";
        }
        if self.error.is_some() {
            return "error";
        }
        if let Some(status) = self
            .result
            .as_ref()
            .and_then(|result| result["status"].as_str())
        {
            if status == "emitted" {
                return "emitted";
            }
            if status == "silent" {
                return "silent";
            }
            if status == "deferred" {
                return "deferred";
            }
        }
        if self.selection.is_some() {
            "ready"
        } else {
            "idle"
        }
    }

    fn view(&self) -> Value {
        json!({
            "phase": self.phase(),
            "selectedName": self.selection.as_ref().map(|selection| &selection.name),
            "context": self.context,
            "result": self.result,
            "error": self.error,
        })
    }
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
    focus_generation: Option<u64>,
    tutorial_persona: bool,
    picker_allowed: bool,
    picker_open: bool,
    discard: bool,
    confirmation: Option<String>,
    reset_scope: Option<String>,
    reset_category: Option<String>,
    reset_undo: Option<ResetUndo>,
    recording: Option<String>,
    persona: Option<Value>,
    persona_load: Option<u64>,
    vrm_open: bool,
    closing: Option<u64>,
    closed: bool,
    activity: ChildActivity,
    speaker: SpeakerManagement,
    feedback_export: Option<u64>,
    feedback_export_path: Option<String>,
    feedback_export_error: Option<String>,
    debug_wake: DebugWake,
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
        self.saving.is_some()
            || self.closing.is_some()
            || self.speaker.busy.is_some()
            || self.debug_wake.command.is_some()
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
        "resetScope":self.reset_scope,"resetCategory":self.reset_category,"canUndoReset":self.reset_undo.is_some(),
        "defaultConfig":default_config_value(),
        "recordingShortcut":self.recording,"personaDocument":self.persona,"personaPickerOpen":self.picker_open && self.picker_allowed,
        "personaPickerAllowed":self.picker_allowed,"showTutorialPersonaSettings":self.tutorial_persona,
        "vrmControlsOpen":self.vrm_open,"closing":self.closing.is_some(),"escapeEnabled":self.escape_enabled(),
        "feedbackExport":{"busy":self.feedback_export.is_some(),"path":self.feedback_export_path,"error":self.feedback_export_error},
        "debugWake":self.debug_wake.view(),
        "speakerManagement":{
            "loading":self.speaker.directory_loading.is_some(),
            "directoryError":self.speaker.directory_error,
            "speakers":self.speaker.directory.as_ref().map(|directory| &directory.speakers),
            "merged":self.speaker.directory.as_ref().map(|directory| &directory.merged),
            "busy":self.speaker.busy.is_some(),
            "succeeded":self.speaker.succeeded,
            "error":self.speaker.error,
            "deleteTarget":self.speaker.delete_target,
            "controls":self.speaker.controls(),
        }})
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
        if observation.speaker_directory_revision > self.speaker.revision {
            self.speaker.revision = observation.speaker_directory_revision;
            self.speaker.directory = None;
            self.speaker.directory_loading = None;
            if self.speaker.visible || self.category.as_deref() == Some("hearing") {
                self.ensure_speaker_directory(io);
            }
        }
        let focus_changed = self.focus_generation != Some(observation.focus_generation);
        self.focus_generation = Some(observation.focus_generation);
        let highlight = observation.focus_section.or_else(|| {
            observation.onboarding["settingsHighlight"]
                .as_str()
                .map(String::from)
        });
        if initial || focus_changed || highlight != self.highlight {
            match highlight.as_deref() {
                Some("watch") => self.category = Some("vision".into()),
                Some("persona") => self.category = Some("general".into()),
                Some("providers") => self.category = Some("providers".into()),
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
        if self.speaker.busy.is_some()
            && (matches!(
                name,
                "speakerMerge"
                    | "speakerUndoMerge"
                    | "speakerReregister"
                    | "speakerDelete"
                    | "speakerDeleteAll"
                    | "speakerRename"
            ) || name == "acceptConfirmation"
                && matches!(
                    self.confirmation.as_deref(),
                    Some("speaker-delete" | "speaker-delete-all")
                ))
        {
            return Err("話者の変更処理が進行中です".into());
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
                        self.clear_confirmation();
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
                if !SETTINGS_CATEGORIES.contains(&category.as_str()) {
                    return Err(action_error(&category));
                }
                self.category = Some(category.clone());
                if category == "hearing" {
                    self.speaker.directory = None;
                    self.ensure_speaker_directory(io);
                }
            }
            "feedbackExport" => {
                if self.feedback_export.is_none() {
                    self.feedback_export_path = None;
                    self.feedback_export_error = None;
                    self.feedback_export = Some(io.command("feedbackExport", ()));
                }
            }
            "speakerViewMounted" => {
                self.speaker.visible = true;
                self.ensure_speaker_directory(io);
            }
            "speakerViewUnmounted" => self.speaker.visible = false,
            "speakerDirectory" => self.ensure_speaker_directory(io),
            "speakerField" => {
                let field = value["field"]
                    .as_str()
                    .ok_or("話者編集フィールドがありません")?;
                let input = value["value"].as_str().ok_or("話者編集値がありません")?;
                if ![
                    "mergeFrom",
                    "mergeTo",
                    "undoSource",
                    "renameID",
                    "nameDraft",
                    "reregisterID",
                    "deleteID",
                ]
                .contains(&field)
                {
                    return Err(action_error(field));
                }
                if self.speaker.busy.is_none() {
                    self.speaker
                        .draft
                        .insert(field.to_owned(), input.to_owned());
                    if field == "renameID" {
                        let name = self
                            .speaker
                            .directory
                            .as_ref()
                            .and_then(|d| d.speakers.iter().find(|s| s.id == input))
                            .and_then(|s| s.name.clone())
                            .unwrap_or_default();
                        self.speaker.draft.insert("nameDraft".into(), name);
                    }
                }
            }
            "speakerMerge" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Merge {
                    source_id: String,
                    target_id: String,
                }
                let request: Merge = decode(value)?;
                let valid = crate::commands_speaker::valid_speaker_id;
                if !valid(&request.source_id)
                    || !valid(&request.target_id)
                    || request.source_id == request.target_id
                {
                    return Err("統合する話者 ID が不正です".into());
                }
                let directory = self.speaker.directory_of()?;
                if !directory.has_speaker(&request.source_id)
                    || !directory.has_speaker(&request.target_id)
                {
                    return Err("統合する話者 ID が候補にありません".into());
                }
                self.speaker.run(
                    io,
                    "speakerManagement",
                    json!({"operation":"merge","sourceId":request.source_id,"targetId":request.target_id}),
                )?;
            }
            "speakerUndoMerge" => {
                let source_id: String = decode(value)?;
                if !crate::commands_speaker::valid_speaker_id(&source_id) {
                    return Err("取り消す話者 ID が不正です".into());
                }
                if !self.speaker.directory_of()?.has_merged_source(&source_id) {
                    return Err("取り消す統合が候補にありません".into());
                }
                self.speaker.run(
                    io,
                    "speakerManagement",
                    json!({"operation":"undoMerge","sourceId":source_id}),
                )?;
            }
            "speakerReregister" => {
                let speaker_id: String = decode(value)?;
                if !crate::commands_speaker::valid_speaker_id(&speaker_id) {
                    return Err("再登録する話者 ID が不正です".into());
                }
                if !self.speaker.directory_of()?.has_speaker(&speaker_id) {
                    return Err("再登録する話者 ID が候補にありません".into());
                }
                self.speaker.run(
                    io,
                    "speakerManagement",
                    json!({"operation":"reregister","speakerId":speaker_id}),
                )?;
            }
            "speakerDelete" => {
                let speaker_id: String = decode(value)?;
                if !crate::commands_speaker::valid_speaker_id(&speaker_id) {
                    return Err("削除する話者 ID が不正です".into());
                }
                if !self.speaker.directory_of()?.has_speaker(&speaker_id) {
                    return Err("削除する話者 ID が候補にありません".into());
                }
                self.clear_confirmation();
                self.confirmation = Some("speaker-delete".into());
                self.speaker.delete_target = Some(speaker_id);
            }
            "speakerDeleteAll" => {
                self.clear_confirmation();
                self.confirmation = Some("speaker-delete-all".into());
            }
            "speakerRename" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Rename {
                    speaker_id: String,
                    name: Option<String>,
                }
                let request: Rename = decode(value)?;
                if !crate::commands_speaker::valid_speaker_id(&request.speaker_id) {
                    return Err("名前を付ける話者 ID が不正です".into());
                }
                if request
                    .name
                    .as_deref()
                    .is_some_and(|name| name.trim().is_empty())
                {
                    return Err("話者の表示名が空です".into());
                }
                if !self
                    .speaker
                    .directory_of()?
                    .has_speaker(&request.speaker_id)
                {
                    return Err("名前を付ける話者 ID が候補にありません".into());
                }
                self.speaker.run(
                    io,
                    "speakerRename",
                    json!({"speakerId":request.speaker_id,"name":request.name}),
                )?;
            }
            "debugWakeLoading" if self.debug_wake.command.is_none() => {
                self.debug_wake.loading = true;
                self.debug_wake.selection = None;
                self.debug_wake.result = None;
                self.debug_wake.error = None;
            }
            "debugWakeLoading" => {}
            "debugWakeSelect" if self.debug_wake.command.is_none() => {
                self.debug_wake.loading = false;
                let selection: DebugWakeSelection = decode(value)?;
                if selection.name.trim().is_empty() {
                    return Err("観測 Wake の画像名がありません".into());
                }
                if selection.image.is_empty() {
                    return Err("観測 Wake の画像が空です".into());
                }
                if selection.image.len() > coosenpai_core::attachments::MAX_ATTACHMENT_BYTES {
                    return Err("観測 Wake の画像が大きすぎます".into());
                }
                self.debug_wake.selection = Some(selection);
                self.debug_wake.result = None;
                self.debug_wake.error = None;
            }
            "debugWakeSelect" => {}
            "debugWakeSelectionError" if self.debug_wake.command.is_none() => {
                let error: String = decode(value)?;
                self.debug_wake.loading = false;
                self.debug_wake.selection = None;
                self.debug_wake.result = None;
                self.debug_wake.error = Some(error);
            }
            "debugWakeContext" if self.debug_wake.command.is_none() => {
                self.debug_wake.context = decode(value)?;
                self.debug_wake.result = None;
                self.debug_wake.error = None;
            }
            "debugWakeContext" => {}
            "debugWakeSend" if self.debug_wake.command.is_none() => {
                let Some(selection) = self.debug_wake.selection.as_ref() else {
                    self.debug_wake.error = Some("観測 Wake の画像を先に選択してください".into());
                    return Ok(());
                };
                let image = selection.image.clone();
                let context = self.debug_wake.context.clone();
                self.debug_wake.error = None;
                self.debug_wake.result = None;
                self.debug_wake.command =
                    Some(io.command("debugWake", json!({"image":image,"context":context})));
            }
            "debugWakeSend" => {}
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
                if let Some(confirmation) = value.as_str() {
                    if confirmation != "conversation-reset" {
                        return Err(action_error(confirmation));
                    }
                    self.clear_confirmation();
                    self.confirmation = Some(confirmation.into());
                } else {
                    let request: ResetConfirmation = decode(value)?;
                    match request.scope.as_str() {
                        "page" => {
                            let category =
                                request.category.ok_or("設定画面の各ページがありません")?;
                            if !SETTINGS_CATEGORIES.contains(&category.as_str()) {
                                return Err(action_error(&category));
                            }
                            self.confirmation = Some("settings-reset".into());
                            self.reset_scope = Some("page".into());
                            self.reset_category = Some(category);
                        }
                        "all" if request.category.is_none() => {
                            self.confirmation = Some("settings-reset".into());
                            self.reset_scope = Some("all".into());
                            self.reset_category = None;
                        }
                        _ => return Err(action_error(&request.scope)),
                    }
                }
            }
            "cancelConfirmation" => self.clear_confirmation(),
            "undoReset" if self.saving.is_none() => {
                if let Some(previous) = self.reset_undo.take() {
                    io.command(
                        "restoreReset",
                        json!({
                            "fields": previous.fields,
                            "scope": previous.scope,
                            "category": previous.category,
                            "defaults": default_config_value(),
                        }),
                    );
                }
            }
            "undoReset" => {}
            "acceptConfirmation" if self.saving.is_none() => {
                let confirmation = self.confirmation.take();
                match confirmation.as_deref() {
                    Some("conversation-reset") => {
                        io.command("resetConversation", ());
                    }
                    Some("speaker-delete") => {
                        let target = self
                            .speaker
                            .delete_target
                            .take()
                            .ok_or("削除する話者 ID がありません")?;
                        self.speaker.run(
                            io,
                            "speakerManagement",
                            json!({"operation":"delete","speakerId":target}),
                        )?;
                    }
                    Some("speaker-delete-all") => {
                        self.speaker.run(
                            io,
                            "speakerManagement",
                            json!({"operation":"deleteAll"}),
                        )?;
                    }
                    Some("settings-reset") => {
                        let scope = self
                            .reset_scope
                            .take()
                            .ok_or("設定の復元範囲がありません")?;
                        let category = self.reset_category.take();
                        let acceptance: ResetAcceptance = decode(value)?;
                        let fields = reset_undo_fields(acceptance.fields)?;
                        self.reset_undo = Some(ResetUndo {
                            fields: fields.clone(),
                            scope: scope.clone(),
                            category: category.clone(),
                        });
                        self.issues.clear();
                        io.command("resetDraft", json!({"fields":fields,"scope":scope,"category":category,"defaults":default_config_value()}));
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
        self.clear_confirmation();
        self.closing = Some(io.command("clearPreview", ()));
    }
    fn clear_confirmation(&mut self) {
        self.confirmation = None;
        self.reset_scope = None;
        self.reset_category = None;
        self.speaker.delete_target = None;
    }
    fn ensure_speaker_directory(&mut self, io: &mut PanelIo) {
        if self.speaker.directory.is_none() && self.speaker.directory_loading.is_none() {
            self.speaker.directory_loading = Some(io.command("speakerDirectory", ()));
        }
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
            "feedbackExport" if self.feedback_export == Some(command.id) => {
                self.feedback_export = None;
                if result.ok {
                    self.feedback_export_path = Some(decode(result.value)?);
                } else {
                    self.feedback_export_error = Some(result.message().into());
                }
            }
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
                    self.reset_undo = None;
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
            "speakerDirectory" if self.speaker.directory_loading == Some(command.id) => {
                self.speaker.directory_loading = None;
                if result.ok {
                    let directory: SpeakerDirectory = decode(result.value)?;
                    directory.validate()?;
                    self.speaker.directory = Some(directory);
                    self.speaker.directory_error = None;
                } else {
                    self.speaker.directory_error = Some(result.message().into());
                }
            }
            "speakerManagement" | "speakerRename" if self.speaker.busy == Some(command.id) => {
                self.speaker.busy = None;
                if result.ok {
                    self.speaker.error = None;
                    self.speaker.succeeded = true;
                    if command.kind == "speakerRename" {
                        let directory: SpeakerDirectory = decode(result.value)?;
                        directory.validate()?;
                        let name = directory
                            .speakers
                            .iter()
                            .find(|speaker| speaker.id == self.speaker.field("renameID"))
                            .and_then(|speaker| speaker.name.clone())
                            .unwrap_or_default();
                        self.speaker.draft.insert("nameDraft".into(), name);
                        self.speaker.directory = Some(directory);
                    } else {
                        // 台帳が変わる操作のあとは候補を読み直す
                        self.speaker.directory = None;
                        self.speaker.directory_loading = None;
                        self.ensure_speaker_directory(io);
                    }
                } else {
                    self.speaker.succeeded = false;
                    self.speaker.error = Some(result.message().into());
                }
            }
            "debugWake" if self.debug_wake.command == Some(command.id) => {
                self.debug_wake.command = None;
                self.debug_wake.loading = false;
                if result.ok {
                    self.debug_wake.error = None;
                    self.debug_wake.result = Some(result.value);
                } else {
                    self.debug_wake.result = None;
                    self.debug_wake.error = Some(result.message().into());
                }
            }
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

fn default_config_value() -> Value {
    serde_json::to_value(coosenpai_core::config::default_config())
        .expect("既定設定の JSON 化に失敗しました")
}

fn reset_undo_fields(fields: Fields) -> Result<Fields, String> {
    if fields
        .keys()
        .any(|key| key == "persona" || key == "avatarPath")
    {
        return Err("設定の復元対象に対象外の項目があります".into());
    }
    Ok(fields)
}
