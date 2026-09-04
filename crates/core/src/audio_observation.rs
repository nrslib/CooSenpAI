use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::ObservationError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum AudioObservationSource {
    Microphone,
    Speaker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptRecord {
    #[serde(default)]
    pub observation_id: String,
    pub time: String,
    pub source: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_tag: Option<String>,
}

impl TranscriptRecord {
    pub fn from_observation(observation: &AudioObservation) -> Self {
        Self {
            observation_id: observation.id.clone(),
            time: observation.created_at.clone(),
            source: match observation.source {
                AudioObservationSource::Microphone => "mic",
                AudioObservationSource::Speaker => "speaker",
            }
            .to_owned(),
            text: observation.text.clone(),
            speaker_tag: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioObservation {
    pub kind: String,
    pub schema_version: u8,
    pub id: String,
    pub created_at: String,
    pub window_start: String,
    pub window_end: String,
    pub source: AudioObservationSource,
    pub text: String,
}

impl AudioObservation {
    pub fn from_confirmed_text(
        source: AudioObservationSource,
        text: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, ObservationError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ObservationError::Missing("text"));
        }
        if text.chars().count() > AUDIO_TEXT_MAX_CHARS {
            return Err(ObservationError::Invalid);
        }
        let timestamp = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        Ok(Self {
            kind: "audio".to_owned(),
            schema_version: 1,
            id: Uuid::new_v4().to_string(),
            created_at: timestamp.clone(),
            window_start: timestamp.clone(),
            window_end: timestamp,
            source,
            text: text.to_owned(),
        })
    }
}

pub const AUDIO_TEXT_MAX_CHARS: usize = 2_000;

pub(super) fn parse(value: Value) -> Result<AudioObservation, ObservationError> {
    let object = value.as_object().ok_or(ObservationError::Invalid)?;
    validate_keys(
        object,
        &[
            "kind",
            "schemaVersion",
            "id",
            "createdAt",
            "windowStart",
            "windowEnd",
            "source",
            "text",
        ],
    )?;
    let record: AudioObservation =
        serde_json::from_value(value).map_err(|_| ObservationError::Invalid)?;
    if record.kind != "audio"
        || record.schema_version != 1
        || record.id.is_empty()
        || record.created_at.is_empty()
        || record.window_start.is_empty()
        || record.window_end.is_empty()
        || record.text.trim().is_empty()
        || record.text.chars().count() > AUDIO_TEXT_MAX_CHARS
    {
        return Err(ObservationError::Invalid);
    }
    Ok(record)
}

fn validate_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), ObservationError> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        Err(ObservationError::Invalid)
    } else {
        Ok(())
    }
}

