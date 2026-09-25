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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerIdentificationStatus {
    Identified,
    Unknown,
    Mixed,
    Unavailable,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_registry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_status: Option<SpeakerIdentificationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

impl TranscriptRecord {
    pub fn from_observation(observation: &AudioObservation) -> Self {
        Self {
            observation_id: observation.id.clone(),
            time: observation.created_at.clone(),
            source: observation_source_name(observation.source).to_owned(),
            text: observation.text.clone(),
            speaker_tag: observation.speaker_id.clone(),
            speaker_registry_id: observation.speaker_registry_id.clone(),
            speaker_status: observation.speaker_status,
            audio_start_ms: None,
            audio_end_ms: None,
            transcript_path: None,
        }
    }

    /// 期間列を持つ観察は期間ごとに1件、持たない観察は従来どおり区間1件にする。
    pub fn periods_from_observation(observation: &AudioObservation) -> Vec<Self> {
        if observation.speaker_segments.is_empty() {
            return vec![Self::from_observation(observation)];
        }
        observation
            .speaker_segments
            .iter()
            .map(|segment| Self {
                observation_id: observation.id.clone(),
                time: observation.created_at.clone(),
                source: observation_source_name(observation.source).to_owned(),
                text: segment.text.clone().unwrap_or_default(),
                speaker_tag: segment.speaker_id.clone(),
                speaker_registry_id: segment.speaker_registry_id.clone(),
                speaker_status: Some(segment.status),
                audio_start_ms: Some(segment.start_ms),
                audio_end_ms: Some(segment.end_ms),
                transcript_path: None,
            })
            .collect()
    }
}

fn observation_source_name(source: AudioObservationSource) -> &'static str {
    match source {
        AudioObservationSource::Microphone => "mic",
        AudioObservationSource::Speaker => "speaker",
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_registry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_status: Option<SpeakerIdentificationStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_segments: Vec<crate::ports::HearingSpeakerSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_decision_details: Vec<crate::speaker_decision::SpeakerDecisionDetails>,
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
        let text = crate::state::truncate(text, AUDIO_TEXT_MAX_CHARS);
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
            segment_id: None,
            audio_start_ms: None,
            audio_end_ms: None,
            speaker_id: None,
            speaker_registry_id: None,
            speaker_status: None,
            speaker_segments: Vec::new(),
            speaker_decision_details: Vec::new(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_speaker_identification(
        &mut self,
        segment_id: &str,
        audio_start_ms: u64,
        audio_end_ms: u64,
        speaker_id: Option<&str>,
        speaker_registry_id: Option<&str>,
        speaker_status: SpeakerIdentificationStatus,
        speaker_segments: Vec<crate::ports::HearingSpeakerSegment>,
    ) -> Result<(), ObservationError> {
        if self.source != AudioObservationSource::Speaker
            || segment_id.is_empty()
            || Uuid::parse_str(segment_id).is_err()
            || audio_end_ms <= audio_start_ms
            || !speaker_segments
                .iter()
                .all(|segment| segment.is_valid_for_segment(segment_id))
        {
            return Err(ObservationError::Invalid);
        }
        match speaker_status {
            SpeakerIdentificationStatus::Identified => {
                let Some(speaker_id) = speaker_id else {
                    return Err(ObservationError::Invalid);
                };
                if !crate::speaker_id::is_valid_speaker_id(speaker_id)
                    || speaker_registry_id.is_none()
                {
                    return Err(ObservationError::Invalid);
                }
            }
            SpeakerIdentificationStatus::Unknown
            | SpeakerIdentificationStatus::Mixed
            | SpeakerIdentificationStatus::Unavailable => {
                if speaker_id.is_some() || speaker_registry_id.is_some() {
                    return Err(ObservationError::Invalid);
                }
            }
        }
        if let Some(registry_id) = speaker_registry_id {
            if Uuid::parse_str(registry_id).is_err() {
                return Err(ObservationError::Invalid);
            }
        }
        self.schema_version = 2;
        self.segment_id = Some(segment_id.to_owned());
        self.audio_start_ms = Some(audio_start_ms);
        self.audio_end_ms = Some(audio_end_ms);
        self.speaker_id = speaker_id.map(ToOwned::to_owned);
        self.speaker_registry_id = speaker_registry_id.map(ToOwned::to_owned);
        self.speaker_status = Some(speaker_status);
        self.speaker_segments = speaker_segments;
        Ok(())
    }

    pub fn apply_speaker_correction(
        &mut self,
        correction: &crate::ports::HearingSpeakerCorrection,
    ) -> Result<bool, ObservationError> {
        if self.source != AudioObservationSource::Speaker
            || !correction.is_valid()
            || self.segment_id.as_deref() != Some(correction.segment_id.as_str())
        {
            return Err(ObservationError::Invalid);
        }
        if self.speaker_segments.is_empty() {
            if self.audio_start_ms != Some(correction.audio_start_ms)
                || self.audio_end_ms != Some(correction.audio_end_ms)
                || self.speaker_status != Some(SpeakerIdentificationStatus::Unknown)
            {
                return Ok(false);
            }
            self.speaker_id = Some(correction.speaker_id.clone());
            self.speaker_registry_id = Some(correction.speaker_registry_id.clone());
            self.speaker_status = Some(SpeakerIdentificationStatus::Identified);
            if let Some(details) = &correction.decision_details {
                self.speaker_decision_details.push(details.clone());
            }
            return Ok(true);
        }
        let Some(segment) = self.speaker_segments.iter_mut().find(|segment| {
            segment.start_ms == correction.audio_start_ms
                && segment.end_ms == correction.audio_end_ms
        }) else {
            return Ok(false);
        };
        if segment.status != SpeakerIdentificationStatus::Unknown {
            return Ok(false);
        }
        if segment.decision_details.iter().any(|details| {
            details
                .registry_id
                .as_deref()
                .is_some_and(|id| id != correction.speaker_registry_id)
                || details.model_package_digest != correction.model_package_digest
        }) {
            return Err(ObservationError::Invalid);
        }
        segment.speaker_id = Some(correction.speaker_id.clone());
        segment.speaker_registry_id = Some(correction.speaker_registry_id.clone());
        segment.status = SpeakerIdentificationStatus::Identified;
        if let Some(details) = &correction.decision_details {
            segment.decision_details.push(details.clone());
        }
        Ok(true)
    }

    pub fn has_speaker_correction(
        &self,
        correction: &crate::ports::HearingSpeakerCorrection,
    ) -> Result<bool, ObservationError> {
        if self.source != AudioObservationSource::Speaker
            || !correction.is_valid()
            || self.segment_id.as_deref() != Some(correction.segment_id.as_str())
        {
            return Err(ObservationError::Invalid);
        }
        if self.speaker_segments.is_empty() {
            return Ok(self.audio_start_ms == Some(correction.audio_start_ms)
                && self.audio_end_ms == Some(correction.audio_end_ms)
                && self.speaker_status == Some(SpeakerIdentificationStatus::Identified)
                && self.speaker_id.as_deref() == Some(correction.speaker_id.as_str())
                && self.speaker_registry_id.as_deref()
                    == Some(correction.speaker_registry_id.as_str()));
        }
        Ok(self.speaker_segments.iter().any(|segment| {
            segment.start_ms == correction.audio_start_ms
                && segment.end_ms == correction.audio_end_ms
                && segment.status == SpeakerIdentificationStatus::Identified
                && segment.speaker_id.as_deref() == Some(correction.speaker_id.as_str())
                && segment.speaker_registry_id.as_deref()
                    == Some(correction.speaker_registry_id.as_str())
        }))
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
            "segmentId",
            "audioStartMs",
            "audioEndMs",
            "speakerId",
            "speakerRegistryId",
            "speakerStatus",
            "speakerSegments",
            "speakerDecisionDetails",
        ],
    )?;
    let record: AudioObservation =
        serde_json::from_value(value).map_err(|_| ObservationError::Invalid)?;
    if record.kind != "audio"
        || !matches!(record.schema_version, 1 | 2)
        || record.id.is_empty()
        || DateTime::parse_from_rfc3339(&record.created_at).is_err()
        || DateTime::parse_from_rfc3339(&record.window_start).is_err()
        || DateTime::parse_from_rfc3339(&record.window_end).is_err()
        || record.text.trim().is_empty()
        || record.text.chars().count() > AUDIO_TEXT_MAX_CHARS
        || !valid_speaker_metadata(&record)
    {
        return Err(ObservationError::Invalid);
    }
    Ok(record)
}

fn valid_speaker_metadata(record: &AudioObservation) -> bool {
    let has_metadata = record.segment_id.is_some()
        || record.audio_start_ms.is_some()
        || record.audio_end_ms.is_some()
        || record.speaker_id.is_some()
        || record.speaker_registry_id.is_some()
        || record.speaker_status.is_some()
        || !record.speaker_segments.is_empty()
        || !record.speaker_decision_details.is_empty();
    if record.schema_version == 1 {
        return !has_metadata;
    }
    if record.source != AudioObservationSource::Speaker {
        return false;
    }
    let (Some(segment_id), Some(start), Some(end), Some(status)) = (
        record.segment_id.as_deref(),
        record.audio_start_ms,
        record.audio_end_ms,
        record.speaker_status,
    ) else {
        return false;
    };
    if Uuid::parse_str(segment_id).is_err() || end <= start {
        return false;
    }
    if record.speaker_decision_details.len() > 1
        || !record.speaker_decision_details.iter().all(|details| {
            details.is_valid_for_segment(segment_id)
                && record.speaker_segments.is_empty()
                && Some(details.start_ms) == record.audio_start_ms
                && Some(details.end_ms) == record.audio_end_ms
        })
    {
        return false;
    }
    if !record
        .speaker_segments
        .iter()
        .all(|segment| segment.is_valid_for_segment(segment_id))
    {
        return false;
    }
    if record
        .speaker_registry_id
        .as_deref()
        .is_some_and(|value| Uuid::parse_str(value).is_err())
    {
        return false;
    }
    match status {
        SpeakerIdentificationStatus::Identified => {
            record
                .speaker_id
                .as_deref()
                .is_some_and(crate::speaker_id::is_valid_speaker_id)
                && record.speaker_registry_id.is_some()
        }
        SpeakerIdentificationStatus::Unknown
        | SpeakerIdentificationStatus::Mixed
        | SpeakerIdentificationStatus::Unavailable => {
            record.speaker_id.is_none() && record.speaker_registry_id.is_none()
        }
    }
}

fn validate_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), ObservationError> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        Err(ObservationError::Invalid)
    } else {
        Ok(())
    }
}

