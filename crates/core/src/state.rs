use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

#[path = "audio_observation.rs"]
mod audio_observation;
pub use audio_observation::{
    AudioObservation, AudioObservationSource, TranscriptRecord, AUDIO_TEXT_MAX_CHARS,
};
#[path = "observation_context.rs"]
mod observation_context;
pub use observation_context::PendingFrameContext;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationEventType {
    Error,
    #[serde(rename = "test-failed")]
    TestFailed,
    #[serde(rename = "test-passed")]
    TestPassed,
    #[serde(rename = "build-failed")]
    BuildFailed,
    #[serde(rename = "build-passed")]
    BuildPassed,
    Commit,
    Milestone,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ObservationConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservationEvent {
    #[serde(rename = "type")]
    pub event_type: ObservationEventType,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationFrame {
    pub trigger: ActivityTriggerKind,
    pub front_app: Option<String>,
    pub app: Option<String>,
    pub target: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityTriggerKind {
    #[serde(rename = "typing-paused")]
    TypingPaused,
    #[serde(rename = "app-switched")]
    AppSwitched,
    Timer,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VisualObservationData {
    pub activity: String,
    #[serde(default)]
    pub outline: String,
    pub changes: Vec<String>,
    pub events: Vec<ObservationEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guess: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ObservationConfidence>,
    pub wake_companion: bool,
}

impl<'de> Deserialize<'de> for VisualObservationData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = VisualObservationDataWire::deserialize(deserializer)?;
        let legacy = wire
            .text_excerpts
            .into_iter()
            .map(|excerpt| excerpt.text)
            .collect::<Vec<_>>()
            .join("\n");
        let outline = match (wire.outline, legacy) {
            (outline, legacy) if outline.is_empty() => legacy,
            (outline, legacy) if legacy.is_empty() => outline,
            (outline, legacy) => format!("{outline}\n{legacy}"),
        };
        Ok(Self {
            activity: wire.activity,
            outline,
            changes: wire.changes,
            events: wire.events,
            guess: wire.guess,
            confidence: wire.confidence,
            wake_companion: wire.wake_companion,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisualObservationDataWire {
    activity: String,
    #[serde(default)]
    outline: String,
    #[serde(default)]
    text_excerpts: Vec<LegacyObservationTextExcerpt>,
    changes: Vec<String>,
    events: Vec<ObservationEvent>,
    #[serde(default)]
    guess: Option<String>,
    #[serde(default)]
    confidence: Option<ObservationConfidence>,
    wake_companion: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyObservationTextExcerpt {
    #[allow(dead_code)]
    region: LegacyObservationRegion,
    #[serde(default)]
    #[allow(dead_code)]
    app: Option<String>,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LegacyObservationRegion {
    Terminal,
    Editor,
    Browser,
    Chat,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VisualObservation {
    pub kind: String,
    pub schema_version: u8,
    pub id: String,
    pub created_at: String,
    pub window_start: String,
    pub window_end: String,
    pub frame_count: usize,
    pub frames: Vec<ObservationFrame>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_frame_ids: Vec<String>,
    #[serde(flatten)]
    pub data: VisualObservationData,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoChangeObservation {
    pub kind: String,
    pub schema_version: u8,
    pub id: String,
    pub created_at: String,
    pub window_start: String,
    pub window_end: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagnation: Option<StagnationObservation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagnationObservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_created_at: Option<String>,
    #[serde(default)]
    pub last_meaningful_change_at: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub activity_signals: bool,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ObservationRecord {
    Visual(VisualObservation),
    NoChange(NoChangeObservation),
    Audio(AudioObservation),
}

impl ObservationRecord {
    pub fn id(&self) -> &str {
        match self {
            Self::Visual(value) => &value.id,
            Self::NoChange(value) => &value.id,
            Self::Audio(value) => &value.id,
        }
    }

    pub fn created_at(&self) -> &str {
        match self {
            Self::Visual(value) => &value.created_at,
            Self::NoChange(value) => &value.created_at,
            Self::Audio(value) => &value.created_at,
        }
    }

    pub fn is_visual(&self) -> bool {
        matches!(self, Self::Visual(_))
    }

    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio(_))
    }

    pub fn is_companion_signal(&self) -> bool {
        self.is_visual()
            || self.is_audio()
            || matches!(self, Self::NoChange(value) if value.stagnation.is_some())
    }

    pub fn is_critical_signal(&self) -> bool {
        matches!(
            self,
            Self::Visual(value)
                if value.data.events.iter().any(|event| matches!(
                    event.event_type,
                    ObservationEventType::Error
                        | ObservationEventType::TestFailed
                        | ObservationEventType::BuildFailed
                ))
        )
    }

    pub fn source_frame_ids(&self) -> &[String] {
        match self {
            Self::Visual(value) => &value.source_frame_ids,
            Self::NoChange(_) | Self::Audio(_) => &[],
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserScreenContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observations: Vec<ObservationRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_frames: Vec<PendingFrameContext>,
}

impl UserScreenContext {
    pub fn is_empty(&self) -> bool {
        self.observations.is_empty() && self.pending_frames.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationEntry {
    pub schema_version: u8,
    pub id: String,
    pub created_at: String,
    pub role: ConversationRole,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tutorial_response_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_context: Option<UserScreenContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caused_by_ids: Vec<String>,
    pub notification_priority: String,
}

impl ConversationEntry {
    pub fn observation_ids(&self) -> impl Iterator<Item = &str> {
        self.caused_by_ids.iter().map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConversationRole {
    User,
    Companion,
}

#[derive(Debug, Error)]
pub enum ObservationError {
    #[error("観察の形式が不正です")]
    Invalid,
    #[error("観察の必須項目がありません: {0}")]
    Missing(&'static str),
}

pub const DEFAULT_OBSERVATION_LIMITS: ObservationLimits = ObservationLimits {
    text_excerpt_max_chars: 600,
    text_excerpt_max_count: 6,
    text_total_max_chars: 2_000,
    changes_max_count: 8,
};

#[derive(Debug, Clone, Copy)]
pub struct ObservationLimits {
    pub text_excerpt_max_chars: usize,
    pub text_excerpt_max_count: usize,
    pub text_total_max_chars: usize,
    pub changes_max_count: usize,
}

impl ObservationLimits {
    pub fn outline_max_bytes(self) -> usize {
        self.text_total_max_chars.min(
            self.text_excerpt_max_chars
                .saturating_mul(self.text_excerpt_max_count),
        )
    }
}

pub fn parse_visual_observation(
    value: Value,
    limits: ObservationLimits,
) -> Result<VisualObservationData, ObservationError> {
    let object = value.as_object().ok_or(ObservationError::Invalid)?;
    validate_visual_keys(object)?;
    let activity = object
        .get("activity")
        .and_then(Value::as_str)
        .ok_or(ObservationError::Missing("activity"))?;
    let wake = object
        .get("wakeCompanion")
        .and_then(Value::as_bool)
        .ok_or(ObservationError::Missing("wakeCompanion"))?;
    let outline = object
        .get("outline")
        .and_then(Value::as_str)
        .ok_or(ObservationError::Missing("outline"))?;
    let changes = object
        .get("changes")
        .and_then(Value::as_array)
        .ok_or(ObservationError::Missing("changes"))?;
    if changes.iter().any(|item| item.as_str().is_none()) {
        return Err(ObservationError::Invalid);
    }
    let changes = changes
        .iter()
        .take(limits.changes_max_count)
        .filter_map(Value::as_str)
        .map(|item| truncate(item, 200))
        .collect();
    let events = object
        .get("events")
        .and_then(Value::as_array)
        .ok_or(ObservationError::Missing("events"))?
        .iter()
        .take(10)
        .map(parse_event)
        .collect::<Result<Vec<_>, _>>()?;
    let guess = match object.get("guess") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(truncate(value, 300)),
        _ => return Err(ObservationError::Invalid),
    };
    let confidence = match object.get("confidence").and_then(Value::as_str) {
        None => None,
        Some("high") => Some(ObservationConfidence::High),
        Some("medium") => Some(ObservationConfidence::Medium),
        Some("low") => Some(ObservationConfidence::Low),
        _ => return Err(ObservationError::Invalid),
    };
    Ok(VisualObservationData {
        activity: truncate(activity, 300),
        outline: truncate_bytes(outline, limits.outline_max_bytes()),
        changes,
        events,
        guess,
        confidence,
        wake_companion: wake,
    })
}

pub fn parse_observation(
    value: Value,
    limits: ObservationLimits,
) -> Result<ObservationRecord, ObservationError> {
    let object = value.as_object().ok_or(ObservationError::Invalid)?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or(ObservationError::Missing("kind"))?;
    if kind == "audio" {
        return Ok(ObservationRecord::Audio(audio_observation::parse(value)?));
    }
    if kind == "no-change" {
        validate_keys(
            object,
            &[
                "kind",
                "schemaVersion",
                "id",
                "createdAt",
                "windowStart",
                "windowEnd",
                "stagnation",
            ],
        )?;
        let record: NoChangeObservation =
            serde_json::from_value(value).map_err(|_| ObservationError::Invalid)?;
        if record.kind != "no-change"
            || record.schema_version != 1
            || record.id.is_empty()
            || record.created_at.is_empty()
            || record.window_start.is_empty()
            || record.window_end.is_empty()
            || record.stagnation.as_ref().is_some_and(|stagnation| {
                stagnation.last_meaningful_change_at.is_empty()
                    || stagnation.elapsed_ms == 0
                    || !stagnation.activity_signals
                    || stagnation.detail.is_empty()
            })
        {
            return Err(ObservationError::Invalid);
        }
        return Ok(ObservationRecord::NoChange(record));
    }
    if kind != "visual" {
        return Err(ObservationError::Invalid);
    }
    let value = normalize_legacy_visual_observation(value)?;
    let object = value.as_object().ok_or(ObservationError::Invalid)?;
    validate_stored_visual_keys(object)?;
    let data = parse_visual_observation(visual_data_value(object), limits)?;
    let record: VisualObservation =
        serde_json::from_value(value).map_err(|_| ObservationError::Invalid)?;
    if record.kind != "visual"
        || record.schema_version != 1
        || record.id.is_empty()
        || record.created_at.is_empty()
        || record.window_start.is_empty()
        || record.window_end.is_empty()
        || record.frame_count == 0
        || record.frames.len() != record.frame_count
    {
        return Err(ObservationError::Invalid);
    }
    Ok(ObservationRecord::Visual(VisualObservation {
        data,
        ..record
    }))
}

fn parse_event(value: &Value) -> Result<ObservationEvent, ObservationError> {
    let object = value.as_object().ok_or(ObservationError::Invalid)?;
    let event_type = match object.get("type").and_then(Value::as_str) {
        Some("error") => ObservationEventType::Error,
        Some("test-failed") => ObservationEventType::TestFailed,
        Some("test-passed") => ObservationEventType::TestPassed,
        Some("build-failed") => ObservationEventType::BuildFailed,
        Some("build-passed") => ObservationEventType::BuildPassed,
        Some("commit") => ObservationEventType::Commit,
        Some("milestone") => ObservationEventType::Milestone,
        Some("other") => ObservationEventType::Other,
        _ => return Err(ObservationError::Invalid),
    };
    let detail = object
        .get("detail")
        .and_then(Value::as_str)
        .ok_or(ObservationError::Invalid)?;
    Ok(ObservationEvent {
        event_type,
        detail: truncate(detail, 200),
    })
}

fn validate_visual_keys(object: &Map<String, Value>) -> Result<(), ObservationError> {
    validate_visual_data_keys(object)
}

fn validate_visual_data_keys(object: &Map<String, Value>) -> Result<(), ObservationError> {
    validate_keys(
        object,
        &[
            "activity",
            "outline",
            "changes",
            "events",
            "guess",
            "confidence",
            "wakeCompanion",
        ],
    )?;
    if let Some(events) = object.get("events").and_then(Value::as_array) {
        for item in events.iter().take(10) {
            let item = item.as_object().ok_or(ObservationError::Invalid)?;
            validate_keys(item, &["type", "detail"])?;
        }
    }
    Ok(())
}

fn validate_stored_visual_keys(object: &Map<String, Value>) -> Result<(), ObservationError> {
    validate_keys(
        object,
        &[
            "kind",
            "schemaVersion",
            "id",
            "createdAt",
            "windowStart",
            "windowEnd",
            "frameCount",
            "frames",
            "sourceFrameIds",
            "activity",
            "outline",
            "textExcerpts",
            "changes",
            "events",
            "guess",
            "confidence",
            "wakeCompanion",
        ],
    )?;
    let frames = object
        .get("frames")
        .and_then(Value::as_array)
        .ok_or(ObservationError::Missing("frames"))?;
    for item in frames {
        let item = item.as_object().ok_or(ObservationError::Invalid)?;
        validate_keys(item, &["trigger", "frontApp", "app", "target"])?;
    }
    let data = visual_data_value(object);
    let data = data.as_object().ok_or(ObservationError::Invalid)?;
    validate_visual_data_keys(data)
}

fn validate_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), ObservationError> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        Err(ObservationError::Invalid)
    } else {
        Ok(())
    }
}

fn visual_data_value(object: &Map<String, Value>) -> Value {
    Value::Object(
        [
            "activity",
            "outline",
            "changes",
            "events",
            "guess",
            "confidence",
            "wakeCompanion",
        ]
        .into_iter()
        .filter_map(|key| object.get(key).map(|value| (key.to_owned(), value.clone())))
        .collect(),
    )
}

fn normalize_legacy_visual_observation(value: Value) -> Result<Value, ObservationError> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or(ObservationError::Invalid)?;
    let Some(legacy_excerpts) = object.remove("textExcerpts") else {
        return Ok(Value::Object(object));
    };
    let legacy_outline = legacy_outline(&legacy_excerpts)?;
    let current_outline = match object.remove("outline") {
        None => None,
        Some(Value::String(value)) => Some(value),
        Some(_) => return Err(ObservationError::Invalid),
    };
    let outline = match (current_outline, legacy_outline) {
        (Some(current), legacy) if current.is_empty() => legacy,
        (Some(current), legacy) if legacy.is_empty() => current,
        (Some(current), legacy) => format!("{current}\n{legacy}"),
        (None, legacy) => legacy,
    };
    object.insert("outline".to_owned(), Value::String(outline));
    Ok(Value::Object(object))
}

fn legacy_outline(value: &Value) -> Result<String, ObservationError> {
    let excerpts = value.as_array().ok_or(ObservationError::Invalid)?;
    let mut texts = Vec::new();
    for item in excerpts {
        let item = item.as_object().ok_or(ObservationError::Invalid)?;
        validate_keys(item, &["region", "app", "text"])?;
        match item.get("region").and_then(Value::as_str) {
            Some("terminal" | "editor" | "browser" | "chat" | "other") => {}
            _ => return Err(ObservationError::Invalid),
        }
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .ok_or(ObservationError::Invalid)?;
        texts.push(text);
    }
    Ok(texts.join("\n"))
}

pub fn truncate(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let collected: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        let prefix: String = value.chars().take(limit.saturating_sub(1)).collect();
        format!("{prefix}…")
    } else {
        collected
    }
}

pub fn truncate_bytes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    if limit == 0 {
        return String::new();
    }
    let ellipsis = "…";
    if limit < ellipsis.len() {
        let mut end = 0usize;
        for (index, character) in value.char_indices() {
            let next = index + character.len_utf8();
            if next > limit {
                break;
            }
            end = next;
        }
        return value[..end].to_owned();
    }
    let prefix_limit = limit.saturating_sub(ellipsis.len());
    let mut end = 0usize;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        if next > prefix_limit {
            break;
        }
        end = next;
    }
    format!("{}\u{2026}", &value[..end])
}

