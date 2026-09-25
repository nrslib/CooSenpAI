use crate::state::{AudioObservationSource, SpeakerIdentificationStatus};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum PortError {
    #[error("platform adapter の I/O に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("platform adapter が利用できません: {0}")]
    Unavailable(String),
    #[error("platform adapter のプロトコルが不正です: {0}")]
    Protocol(String),
    #[error("platform adapter の話者プロトコルが不正です: {0}")]
    SpeakerProtocol(String),
    #[error("画面収録の権限がありません: {0}")]
    ScreenCapturePermission(String),
    #[error("platform adapter が timeout しました")]
    Timeout,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusElement {
    pub bundle_id: String,
    pub window_title: Option<String>,
    pub role: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FocusElementWire {
    bundle_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_title: Option<String>,
    role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

impl Serialize for FocusElement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bounded = self.clone().bounded();
        FocusElementWire {
            bundle_id: bounded.bundle_id,
            window_title: bounded.window_title,
            role: bounded.role,
            title: bounded.title,
            description: bounded.description,
            value: bounded.value,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FocusElement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = FocusElementWire::deserialize(deserializer)?;
        Ok(Self {
            bundle_id: value.bundle_id,
            window_title: value.window_title,
            role: value.role,
            title: value.title,
            description: value.description,
            value: value.value,
        }
        .bounded())
    }
}

pub const SECURE_TEXT_FIELD_ROLE: &str = "AXSecureTextField";

impl FocusElement {
    pub fn bounded(mut self) -> Self {
        self.bundle_id = crate::state::truncate(&self.bundle_id, 300);
        self.window_title = self
            .window_title
            .map(|value| crate::state::truncate(&value, 300));
        self.role = crate::state::truncate(&self.role, 120);
        self.title = self.title.map(|value| crate::state::truncate(&value, 300));
        self.description = self
            .description
            .map(|value| crate::state::truncate(&value, 300));
        self.value = if self.role == SECURE_TEXT_FIELD_ROLE {
            None
        } else {
            self.value.map(|value| crate::state::truncate(&value, 600))
        };
        self
    }

    pub fn prompt_summary(&self) -> String {
        let bounded = self.clone().bounded();
        let window_title = bounded
            .window_title
            .as_deref()
            .map_or_else(|| "（ウインドウ名なし）".to_owned(), single_line);
        let value = bounded
            .value
            .as_deref()
            .map_or_else(|| "（値なし）".to_owned(), single_line);
        format!(
            "フォーカス: {} / {} / {} / {}",
            bounded.bundle_id, window_title, bounded.role, value
        )
    }
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

#[async_trait]
pub trait FocusElementPort: Send + Sync {
    async fn read_focused_element(
        &self,
        target_bundle_id: Option<&str>,
    ) -> Result<Option<FocusElement>, PortError>;
}

pub const FOCUS_ELEMENT_TIMEOUT: Duration = Duration::from_millis(200);

pub const SELECTED_TEXT_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub const SELECTED_TEXT_POLL_TIMEOUT: Duration = Duration::from_millis(1000);

pub trait ClipboardReader: Send + Sync {
    fn read_text(&self) -> Result<Option<String>, PortError>;

    fn change_count(&self) -> Result<i64, PortError>;
}

pub trait ClipboardWriter: Send + Sync {
    fn write_text(&self, text: &str) -> Result<(), PortError>;

    fn clear(&self) -> Result<(), PortError>;
}

#[async_trait]
pub trait SelectedTextCopyPort: Send + Sync {
    async fn synthesize_copy(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<SelectedTextCopyOutcome, PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedTextCopyOutcome {
    Sent { change_count_before_post: i64 },
    PermissionDenied,
    Cancelled,
    ReleaseTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

pub trait BubbleDisplayPort: Send + Sync {
    fn cursor_point(&self) -> Result<ScreenPoint, PortError>;

    fn frontmost_window_point(&self) -> Result<Option<ScreenPoint>, PortError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenDisplay {
    pub id: u32,
    /// Global desktop coordinates in logical points (CoreGraphics top-left origin).
    pub bounds: WindowBounds,
}

#[derive(Debug, Clone)]
pub struct CapturedScreen {
    pub display: ScreenDisplay,
    pub path: PathBuf,
}

#[async_trait]
pub trait ScreenCapturePort: Send + Sync {
    async fn capture(
        &self,
        destination_directory: &Path,
        cancellation: CancellationToken,
    ) -> Result<Vec<CapturedScreen>, PortError>;
}

pub trait HelperResolverPort: Send + Sync {
    fn resolve_ocr_helper(
        &self,
        executable_dir: &Path,
        product_root: &Path,
        configured: Option<&str>,
    ) -> Option<PathBuf>;

    fn resolve_provider_bridge(
        &self,
        executable_dir: &Path,
        product_root: &Path,
        resource_root: Option<&Path>,
    ) -> Option<PathBuf>;

    fn resolve_speech_helper(&self, executable_dir: &Path, product_root: &Path) -> Option<PathBuf>;

    fn resolve_hearing_helper(&self, executable_dir: &Path, product_root: &Path)
        -> Option<PathBuf>;

    fn resolve_node(&self, path_value: &str) -> Option<PathBuf>;
}

pub trait ProviderApiKeyStore: Send + Sync {
    fn read(&self, provider: crate::provider::ProviderName) -> Result<Option<String>, PortError>;

    fn write(
        &self,
        provider: crate::provider::ProviderName,
        api_key: &str,
    ) -> Result<(), PortError>;

    fn delete(&self, provider: crate::provider::ProviderName) -> Result<(), PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemSettingsPane {
    ScreenCapture,
    SystemAudio,
    Accessibility,
    Microphone,
    SpeechRecognition,
}

#[async_trait]
pub trait SystemSettingsPort: Send + Sync {
    async fn open(
        &self,
        pane: SystemSettingsPane,
        cancellation: CancellationToken,
    ) -> Result<(), PortError>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechPermissionKind {
    NotDetermined,
    Granted,
    Denied,
    Restricted,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechPermissions {
    pub microphone: SpeechPermissionKind,
    pub recognition: SpeechPermissionKind,
}

impl Default for SpeechPermissions {
    fn default() -> Self {
        Self {
            microphone: SpeechPermissionKind::NotDetermined,
            recognition: SpeechPermissionKind::NotDetermined,
        }
    }
}

#[async_trait]
pub trait SpeechPermissionPort: Send + Sync {
    async fn request_recognition(
        &self,
        cancellation: CancellationToken,
    ) -> Result<SpeechPermissionKind, PortError>;

    fn current(&self) -> Result<SpeechPermissions, PortError>;

    async fn request(
        &self,
        cancellation: CancellationToken,
    ) -> Result<SpeechPermissions, PortError>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpeechRecognitionEngine {
    SpeechAnalyzer,
    SFSpeechRecognizer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SpeechEvent {
    Ready {
        locale: String,
        microphone: SpeechPermissionKind,
        recognition: SpeechPermissionKind,
        engine: SpeechRecognitionEngine,
    },
    Partial {
        text: String,
    },
    Final {
        text: String,
    },
    Warning {
        kind: String,
        message: String,
    },
    Error {
        kind: String,
        message: String,
    },
    Closed,
}

pub enum SpeechCommand {
    Finish,
    Cancel {
        completed: oneshot::Sender<Result<(), PortError>>,
    },
}

#[derive(Clone)]
pub struct SpeechSessionControl {
    commands: mpsc::Sender<SpeechCommand>,
}

pub struct SpeechSession {
    control: SpeechSessionControl,
    events: mpsc::Receiver<Result<SpeechEvent, PortError>>,
}

impl SpeechSession {
    pub fn from_channels(
        commands: mpsc::Sender<SpeechCommand>,
        events: mpsc::Receiver<Result<SpeechEvent, PortError>>,
    ) -> Self {
        Self {
            control: SpeechSessionControl { commands },
            events,
        }
    }

    pub fn control(&self) -> SpeechSessionControl {
        self.control.clone()
    }

    pub async fn next_event(&mut self) -> Option<Result<SpeechEvent, PortError>> {
        self.events.recv().await
    }
}

impl SpeechSessionControl {
    pub async fn finish(&self) -> Result<(), PortError> {
        self.commands
            .send(SpeechCommand::Finish)
            .await
            .map_err(|_| PortError::Unavailable("音声入力は停止しています".to_owned()))
    }

    pub async fn cancel(&self) -> Result<(), PortError> {
        let (completed, result) = oneshot::channel();
        self.commands
            .send(SpeechCommand::Cancel { completed })
            .await
            .map_err(|_| PortError::Unavailable("音声入力は停止しています".to_owned()))?;
        result.await.map_err(|_| {
            PortError::Unavailable("音声入力の終了を確認できませんでした".to_owned())
        })?
    }
}

#[async_trait]
pub trait SpeechPort: Send + Sync {
    async fn start(
        &self,
        locale: &str,
        input_device: &str,
        cancellation: CancellationToken,
    ) -> Result<SpeechSession, PortError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HearingEvent {
    Ready {
        locale: String,
        microphone: SpeechPermissionKind,
        recognition: SpeechPermissionKind,
        #[serde(
            rename = "protocolVersion",
            default = "default_hearing_protocol_version"
        )]
        protocol_version: u8,
        #[serde(rename = "speakerIdentification", default)]
        speaker_identification: bool,
    },
    SpeakerIdentification {
        status: SpeakerIdentificationPreparationStatus,
    },
    Recognizing {
        source: AudioObservationSource,
        generation: u64,
        sequence: u64,
        text: String,
    },
    NoSpeech {
        source: AudioObservationSource,
        generation: u64,
        sequence: u64,
    },
    Final {
        source: AudioObservationSource,
        generation: u64,
        sequence: u64,
        text: String,
        #[serde(flatten)]
        speaker: Option<HearingSpeakerMetadata>,
    },
    Warning {
        kind: String,
        message: String,
    },
    Error {
        kind: String,
        message: String,
    },
    Closed,
}

fn default_hearing_protocol_version() -> u8 {
    1
}

const HEARING_SPEAKER_METADATA_KEYS: &[&str] = &[
    "segmentId",
    "audioStartMs",
    "audioEndMs",
    "speakerId",
    "speakerRegistryId",
    "speakerStatus",
    "speakerSegments",
    "speakerCorrections",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedHearingEvent {
    event: HearingEvent,
    speaker_metadata_reason: Option<&'static str>,
}

impl DecodedHearingEvent {
    pub fn into_parts(self) -> (HearingEvent, Option<&'static str>) {
        (self.event, self.speaker_metadata_reason)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HearingEventDecodeError {
    InvalidJson,
    InvalidEnvelope,
}

pub fn decode_hearing_event(bytes: &[u8]) -> Result<DecodedHearingEvent, HearingEventDecodeError> {
    let value =
        serde_json::from_slice::<Value>(bytes).map_err(|_| HearingEventDecodeError::InvalidJson)?;
    let Some(object) = value.as_object() else {
        return Err(HearingEventDecodeError::InvalidEnvelope);
    };
    if object.get("event").and_then(Value::as_str) != Some("final") {
        let event =
            serde_json::from_value(value).map_err(|_| HearingEventDecodeError::InvalidEnvelope)?;
        return Ok(DecodedHearingEvent {
            event,
            speaker_metadata_reason: None,
        });
    }

    let mut required = object.clone();
    let mut speaker_metadata = serde_json::Map::new();
    for key in HEARING_SPEAKER_METADATA_KEYS {
        if let Some(value) = required.remove(*key) {
            speaker_metadata.insert((*key).to_owned(), value);
        }
    }
    let required_event = serde_json::from_value::<HearingEvent>(Value::Object(required))
        .map_err(|_| HearingEventDecodeError::InvalidEnvelope)?;
    let HearingEvent::Final {
        source,
        generation,
        sequence,
        text,
        speaker: _,
    } = required_event
    else {
        return Err(HearingEventDecodeError::InvalidEnvelope);
    };

    if speaker_metadata.is_empty() {
        return Ok(DecodedHearingEvent {
            event: HearingEvent::Final {
                source,
                generation,
                sequence,
                text,
                speaker: None,
            },
            speaker_metadata_reason: None,
        });
    }

    match serde_json::from_value::<HearingSpeakerMetadata>(Value::Object(speaker_metadata)) {
        Ok(metadata) => Ok(DecodedHearingEvent {
            event: HearingEvent::Final {
                source,
                generation,
                sequence,
                text,
                speaker: Some(metadata),
            },
            speaker_metadata_reason: None,
        }),
        // serde error には metadata の値を含み得るため、wire 境界では固定理由だけを返す。
        Err(_) => Ok(DecodedHearingEvent {
            event: HearingEvent::Final {
                source,
                generation,
                sequence,
                text,
                speaker: None,
            },
            speaker_metadata_reason: Some("metadata.decode"),
        }),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerIdentificationPreparationStatus {
    Preparing,
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HearingSpeakerMetadata {
    pub segment_id: String,
    pub audio_start_ms: u64,
    pub audio_end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_registry_id: Option<String>,
    pub speaker_status: SpeakerIdentificationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_segments: Vec<HearingSpeakerSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_corrections: Vec<HearingSpeakerCorrection>,
}

impl HearingSpeakerMetadata {
    pub fn validation_reason_for(&self, source: AudioObservationSource) -> Option<String> {
        if source != AudioObservationSource::Speaker {
            return Some("metadata.source".to_owned());
        }
        if self.segment_id.is_empty() || uuid::Uuid::parse_str(&self.segment_id).is_err() {
            return Some("metadata.segment-id".to_owned());
        }
        if self.audio_end_ms <= self.audio_start_ms {
            return Some("metadata.period-bounds".to_owned());
        }
        if self
            .speaker_registry_id
            .as_deref()
            .is_some_and(|value| uuid::Uuid::parse_str(value).is_err())
        {
            return Some("metadata.registry-id".to_owned());
        }
        for (index, segment) in self.speaker_segments.iter().enumerate() {
            if let Some(reason) = segment.validation_reason_for_segment(&self.segment_id) {
                return Some(format!("metadata.speakerSegments[{index}].{reason}"));
            }
        }
        if self.speaker_corrections.len() > 512 {
            return Some("metadata.correction-count".to_owned());
        }
        for (index, correction) in self.speaker_corrections.iter().enumerate() {
            if let Some(reason) = correction.validation_reason() {
                return Some(format!("metadata.speakerCorrections[{index}].{reason}"));
            }
        }
        if self.speaker_corrections.windows(2).any(|pair| {
            pair[0].speaker_registry_id != pair[1].speaker_registry_id
                || pair[0].model_package_digest != pair[1].model_package_digest
        }) {
            return Some("metadata.correction-consistency".to_owned());
        }
        if self
            .speaker_registry_id
            .as_ref()
            .is_some_and(|registry_id| {
                self.speaker_corrections
                    .iter()
                    .any(|correction| &correction.speaker_registry_id != registry_id)
            })
        {
            return Some("metadata.correction-registry".to_owned());
        }
        match self.speaker_status {
            SpeakerIdentificationStatus::Identified => {
                if !self
                    .speaker_id
                    .as_deref()
                    .is_some_and(crate::speaker_id::is_valid_speaker_id)
                {
                    return Some("metadata.speaker-id".to_owned());
                }
                if self.speaker_registry_id.is_none() {
                    return Some("metadata.identified-registry".to_owned());
                }
            }
            SpeakerIdentificationStatus::Unknown
            | SpeakerIdentificationStatus::Mixed
            | SpeakerIdentificationStatus::Unavailable => {
                if self.speaker_id.is_some() || self.speaker_registry_id.is_some() {
                    return Some("metadata.status-identifiers".to_owned());
                }
            }
        }
        None
    }

    pub fn is_valid_for(&self, source: AudioObservationSource) -> bool {
        self.validation_reason_for(source).is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HearingSpeakerCorrection {
    pub segment_id: String,
    pub audio_start_ms: u64,
    pub audio_end_ms: u64,
    pub speaker_id: String,
    pub speaker_registry_id: String,
    pub model_package_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_details: Option<crate::speaker_decision::SpeakerDecisionDetails>,
}

impl HearingSpeakerCorrection {
    pub fn validation_reason(&self) -> Option<String> {
        if self.segment_id.is_empty() || uuid::Uuid::parse_str(&self.segment_id).is_err() {
            return Some("correction.segment-id".to_owned());
        }
        if self.audio_end_ms <= self.audio_start_ms {
            return Some("correction.period-bounds".to_owned());
        }
        if !crate::speaker_id::is_valid_speaker_id(&self.speaker_id) {
            return Some("correction.speaker-id".to_owned());
        }
        if uuid::Uuid::parse_str(&self.speaker_registry_id).is_err() {
            return Some("correction.registry-id".to_owned());
        }
        if !is_valid_sha256_hex(&self.model_package_digest) {
            return Some("correction.model-digest".to_owned());
        }
        let details = self.decision_details.as_ref()?;
        if let Some(reason) = details.validation_reason_for_segment(&self.segment_id) {
            return Some(format!("correction.decisionDetails.{reason}"));
        }
        if details.start_ms != self.audio_start_ms || details.end_ms != self.audio_end_ms {
            return Some("correction.decision-period".to_owned());
        }
        if details.registry_id.as_deref() != Some(self.speaker_registry_id.as_str()) {
            return Some("correction.decision-registry".to_owned());
        }
        if details.model_package_digest != self.model_package_digest {
            return Some("correction.decision-digest".to_owned());
        }
        let valid_phase = match details.phase {
            crate::speaker_decision::SpeakerDecisionPhase::BackfillAnchor
            | crate::speaker_decision::SpeakerDecisionPhase::BackfillSamples => {
                // 再送時のspeakerIdは手動aliasを解決するが、当時の候補IDは書き換えない。
                !details.candidates.is_empty()
            }
            crate::speaker_decision::SpeakerDecisionPhase::InitialRecent => {
                details.reason == "recent-consensus"
                    && details.candidates.is_empty()
                    && details.candidate_count == 0
            }
            crate::speaker_decision::SpeakerDecisionPhase::Initial => false,
        };
        if !valid_phase {
            return Some("correction.phase".to_owned());
        }
        None
    }

    pub fn is_valid(&self) -> bool {
        self.validation_reason().is_none()
    }
}

fn is_valid_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// 話者の交代で区切られた区間内の連続時間。話者 ID は期間ごとに付く。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HearingSpeakerSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_registry_id: Option<String>,
    pub status: SpeakerIdentificationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_details: Vec<crate::speaker_decision::SpeakerDecisionDetails>,
}

impl HearingSpeakerSegment {
    fn common_validation_reason(&self) -> Option<&'static str> {
        if self.end_ms <= self.start_ms {
            return Some("segment.period-bounds");
        }
        if self.decision_details.len() > 65 {
            return Some("segment.decision-count");
        }
        if self
            .speaker_registry_id
            .as_deref()
            .is_some_and(|value| uuid::Uuid::parse_str(value).is_err())
        {
            return Some("segment.registry-id");
        }
        None
    }

    fn status_validation_reason(&self) -> Option<&'static str> {
        match self.status {
            SpeakerIdentificationStatus::Identified => {
                if !self
                    .speaker_id
                    .as_deref()
                    .is_some_and(crate::speaker_id::is_valid_speaker_id)
                {
                    return Some("segment.speaker-id");
                }
                if self.speaker_registry_id.is_none() {
                    return Some("segment.identified-registry");
                }
            }
            // unavailable は区間レベルの状態であり、期間には出ない。
            SpeakerIdentificationStatus::Unknown | SpeakerIdentificationStatus::Mixed => {
                if self.speaker_id.is_some() || self.speaker_registry_id.is_some() {
                    return Some("segment.status-identifiers");
                }
            }
            SpeakerIdentificationStatus::Unavailable => return Some("segment.status"),
        }
        None
    }

    fn validation_reason(&self) -> Option<&'static str> {
        if let Some(reason) = self.common_validation_reason() {
            return Some(reason);
        }
        if self.decision_details.iter().any(|details| {
            !details.is_valid() || details.start_ms < self.start_ms || details.end_ms > self.end_ms
        }) {
            return Some("segment.decision-details");
        }
        self.status_validation_reason()
    }

    pub fn is_valid(&self) -> bool {
        self.validation_reason().is_none()
    }

    pub(crate) fn validation_reason_for_segment(&self, parent_segment_id: &str) -> Option<String> {
        if let Some(reason) = self.common_validation_reason() {
            return Some(reason.to_owned());
        }
        for (index, details) in self.decision_details.iter().enumerate() {
            if let Some(reason) = details.validation_reason_for_segment(parent_segment_id) {
                return Some(format!("segment.decisionDetails[{index}].{reason}"));
            }
            if details.start_ms < self.start_ms || details.end_ms > self.end_ms {
                return Some(format!("segment.decisionDetails[{index}].period-bounds"));
            }
        }
        self.status_validation_reason().map(str::to_owned)
    }

    pub(crate) fn is_valid_for_segment(&self, parent_segment_id: &str) -> bool {
        self.validation_reason_for_segment(parent_segment_id)
            .is_none()
    }
}

impl HearingEvent {
    pub fn is_valid_required_final_for_source(&self, source: AudioObservationSource) -> bool {
        match self {
            HearingEvent::Final {
                source: event_source,
                generation,
                sequence,
                text,
                ..
            } => {
                event_source == &source
                    && *generation > 0
                    && *sequence > 0
                    && !text.trim().is_empty()
            }
            _ => true,
        }
    }

    pub fn is_valid_for_source(&self, source: AudioObservationSource) -> bool {
        if !self.is_valid_required_final_for_source(source) {
            return false;
        }
        match self {
            HearingEvent::Final { speaker, .. } => speaker
                .as_ref()
                .is_none_or(|metadata| metadata.validation_reason_for(source).is_none()),
            HearingEvent::SpeakerIdentification { .. } => source == AudioObservationSource::Speaker,
            _ => true,
        }
    }

    pub fn detach_invalid_speaker_metadata(
        self,
        source: AudioObservationSource,
    ) -> Option<(Self, String)> {
        if !self.is_valid_required_final_for_source(source) {
            return None;
        }
        let HearingEvent::Final {
            source: event_source,
            generation,
            sequence,
            text,
            speaker: Some(metadata),
        } = self
        else {
            return None;
        };
        let reason = metadata.validation_reason_for(source)?;
        Some((
            HearingEvent::Final {
                source: event_source,
                generation,
                sequence,
                text,
                speaker: None,
            },
            reason,
        ))
    }
}

pub enum HearingCommand {
    Cancel {
        completed: oneshot::Sender<Result<(), PortError>>,
    },
}

#[derive(Clone)]
pub struct HearingSessionControl {
    commands: mpsc::Sender<HearingCommand>,
    cancel_requested: CancellationToken,
}

pub struct HearingSession {
    control: HearingSessionControl,
    events: mpsc::Receiver<Result<HearingEvent, PortError>>,
}

impl HearingSession {
    pub fn from_channels(
        commands: mpsc::Sender<HearingCommand>,
        events: mpsc::Receiver<Result<HearingEvent, PortError>>,
    ) -> Self {
        Self::from_channels_with_cancellation(commands, events, CancellationToken::new())
    }

    pub fn from_channels_with_cancellation(
        commands: mpsc::Sender<HearingCommand>,
        events: mpsc::Receiver<Result<HearingEvent, PortError>>,
        cancel_requested: CancellationToken,
    ) -> Self {
        Self {
            control: HearingSessionControl {
                commands,
                cancel_requested,
            },
            events,
        }
    }

    pub fn control(&self) -> HearingSessionControl {
        self.control.clone()
    }

    pub async fn next_event(&mut self) -> Option<Result<HearingEvent, PortError>> {
        self.events.recv().await
    }
}

impl HearingSessionControl {
    pub async fn cancel(&self) -> Result<(), PortError> {
        let (completed, result) = oneshot::channel();
        self.cancel_requested.cancel();
        if self
            .commands
            .send(HearingCommand::Cancel { completed })
            .await
            .is_err()
        {
            return Ok(());
        }
        match result.await {
            Ok(result) => result,
            Err(_) => Ok(()),
        }
    }
}

#[async_trait]
pub trait HearingPort: Send + Sync {
    async fn start(
        &self,
        locale: &str,
        input_device: &str,
        sources: Vec<AudioObservationSource>,
        debug_dump_dir: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<HearingSession, PortError>;

    async fn start_with_options(
        &self,
        locale: &str,
        input_device: &str,
        sources: Vec<AudioObservationSource>,
        debug_dump_dir: Option<&str>,
        options: HearingStartOptions,
        cancellation: CancellationToken,
    ) -> Result<HearingSession, PortError> {
        let _ = options;
        self.start(locale, input_device, sources, debug_dump_dir, cancellation)
            .await
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HearingStartOptions {
    pub speaker_identification_enabled: bool,
    pub speaker_model: Option<PathBuf>,
    pub speaker_ledger: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeechInputDevice {
    pub id: String,
    pub name: String,
}

pub trait SpeechInputDevicePort: Send + Sync {
    fn input_devices(&self) -> Result<Vec<SpeechInputDevice>, PortError>;
}

pub trait SpeechKeyStatePort: Send + Sync {
    fn primary_key_pressed(&self, shortcut: &str) -> Result<bool, PortError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunningApplication {
    pub bundle_id: String,
    pub name: String,
    pub icon_png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApplication {
    pub process_id: i32,
    pub bundle_id: Option<String>,
}

pub trait ForegroundApplicationPort: Send + Sync {
    fn frontmost_application(&self) -> Result<Option<ForegroundApplication>, PortError>;
    fn activate_application(&self, application: &ForegroundApplication) -> Result<(), PortError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplicationCapture {
    pub path: PathBuf,
    pub window_id: u32,
    pub window_bounds: WindowBounds,
    pub display: ScreenDisplay,
}

#[async_trait]
pub trait ApplicationCapturePort: Send + Sync {
    fn running_applications(&self) -> Result<Vec<RunningApplication>, PortError>;
    async fn capture_application(
        &self,
        bundle_id: &str,
        destination_directory: &Path,
        window_limit: usize,
        cancellation: CancellationToken,
    ) -> Result<Vec<ApplicationCapture>, PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCapturePermissionKind {
    NotDetermined,
    Granted,
    Denied,
    Restricted,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenCapturePermission {
    pub kind: ScreenCapturePermissionKind,
    pub requestable: bool,
    pub capture_verified: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenCapturePresentation {
    pub status: &'static str,
    pub message: Option<&'static str>,
}

impl ScreenCapturePermission {
    pub fn from_preflight(granted: bool, request_already_attempted: bool) -> Self {
        if granted {
            Self {
                kind: ScreenCapturePermissionKind::Granted,
                requestable: false,
                capture_verified: None,
            }
        } else {
            Self {
                kind: if request_already_attempted {
                    ScreenCapturePermissionKind::Denied
                } else {
                    ScreenCapturePermissionKind::NotDetermined
                },
                requestable: !request_already_attempted,
                capture_verified: None,
            }
        }
    }

    pub fn after_request(accepted: bool, preflight_granted: bool) -> Self {
        if !accepted {
            return Self {
                kind: ScreenCapturePermissionKind::Denied,
                requestable: false,
                capture_verified: None,
            };
        }
        Self {
            kind: ScreenCapturePermissionKind::Granted,
            requestable: false,
            capture_verified: (!preflight_granted).then_some(false),
        }
    }

    pub fn with_capture_result(mut self, succeeded: bool) -> Self {
        self.capture_verified = Some(succeeded);
        self
    }

    pub fn requires_restart(self) -> bool {
        self.kind == ScreenCapturePermissionKind::Granted && self.capture_verified == Some(false)
    }

    pub fn presentation(self) -> ScreenCapturePresentation {
        self.presentation_for_locale(crate::locale::Locale::Ja)
    }

    pub fn presentation_for_locale(
        self,
        locale: crate::locale::Locale,
    ) -> ScreenCapturePresentation {
        match (self.kind, self.capture_verified) {
            (ScreenCapturePermissionKind::Granted, Some(false)) => ScreenCapturePresentation {
                status: "not-granted",
                message: Some(crate::locale::text(
                    crate::locale::TextKey::ScreenPermissionRestart,
                    locale,
                )),
            },
            (ScreenCapturePermissionKind::Granted, _) => ScreenCapturePresentation {
                status: "granted",
                message: None,
            },
            (ScreenCapturePermissionKind::NotDetermined, _)
            | (ScreenCapturePermissionKind::Denied, _) => ScreenCapturePresentation {
                status: "not-granted",
                message: Some(crate::locale::text(
                    crate::locale::TextKey::ScreenPermissionAllow,
                    locale,
                )),
            },
            (ScreenCapturePermissionKind::Restricted, _) => ScreenCapturePresentation {
                status: "not-granted",
                message: Some(crate::locale::text(
                    crate::locale::TextKey::ScreenPermissionRestricted,
                    locale,
                )),
            },
            (ScreenCapturePermissionKind::Unavailable, _) => ScreenCapturePresentation {
                status: "unknown",
                message: Some(crate::locale::text(
                    crate::locale::TextKey::ScreenPermissionUnavailable,
                    locale,
                )),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OwnWindowBounds {
    /// ウィンドウの位置・寸法・表示状態が変わるたびに進む世代。
    pub revision: u64,
    pub captured_at: DateTime<Utc>,
    /// ディスプレイ倍率に依存しない、デスクトップ全体の論理座標系の矩形。
    pub bounds: Vec<WindowBounds>,
}

impl OwnWindowBounds {
    pub fn is_fresh_at(&self, now: DateTime<Utc>) -> bool {
        let age = now.signed_duration_since(self.captured_at);
        age >= chrono::Duration::zero() && age <= chrono::Duration::seconds(5)
    }
}

#[async_trait]
pub trait OwnWindowBoundsPort: Send + Sync {
    async fn read_own_window_bounds(&self) -> Result<OwnWindowBounds, PortError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub idle_ms: u64,
    pub front_app: Option<String>,
    pub front_app_bundle_id: Option<String>,
}

#[async_trait]
pub trait ActivityPort: Send + Sync {
    async fn read_activity(&self) -> Result<ActivitySnapshot, PortError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrTextBlock {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub confidence: f32,
}

#[async_trait]
pub trait OcrPort: Send + Sync {
    async fn recognize(
        &self,
        path: &Path,
        level: &str,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<Vec<OcrTextBlock>, PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Sleep,
    Wake,
    Lock,
    Unlock,
}

#[async_trait]
pub trait PowerEventPort: Send + Sync {
    async fn next(&mut self) -> Result<Option<PowerEvent>, PortError>;
}

#[async_trait]
pub trait NotificationPort: Send + Sync {
    async fn show(
        &self,
        message: &str,
        priority: &str,
        duration: Duration,
    ) -> Result<(), PortError>;
}

pub trait RuntimeLogger: Send + Sync {
    fn write(&self, level: &str, message: &str) -> Result<(), std::io::Error>;
}

