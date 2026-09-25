use crate::config::ConfigPaths;
use crate::judge::{JudgeAction, JudgeDecision, JudgeFeedSign, JudgeTrace};
use crate::persistence::{JsonlStore, PersistenceError, SiblingLock};
use crate::state::{AudioObservationSource, ConversationEntry, ObservationRecord};
#[path = "utterance_feedback_archive.rs"]
mod archive;
#[path = "utterance_feedback_materials.rs"]
mod materials;
#[path = "utterance_feedback_security.rs"]
mod security;
use archive::{project_observation, publish_archive, sanitize_json, sanitize_text};
use chrono::{DateTime, Utc};
use materials::{MaterialCollectionInput, MaterialContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

pub const UTTERANCE_FEEDBACK_SCHEMA_VERSION: u8 = 2;
pub const UTTERANCE_FEEDBACK_FILE_NAME: &str = "utterance-feedback.jsonl";
pub const UTTERANCE_FEEDBACK_ARCHIVE_DIRECTORY: &str = "utterance-feedback";
pub const UTTERANCE_FEEDBACK_FREE_TEXT_MAX_CHARS: usize = 500;

fn default_feedback_sign() -> JudgeFeedSign {
    JudgeFeedSign::Negative
}

fn default_feedback_revision() -> u64 {
    1
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackReasonCode {
    None,
    NoActivity,
    Repeated,
    Misunderstood,
    BadTiming,
    Other,
}

impl FeedbackReasonCode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "none" => Self::None,
            "no-activity" => Self::NoActivity,
            "repeated" => Self::Repeated,
            "misunderstood" => Self::Misunderstood,
            "bad-timing" => Self::BadTiming,
            "other" => Self::Other,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UtteranceFeedbackRecord {
    #[serde(default)]
    pub trigger: FeedbackTrigger,
    pub schema_version: u8,
    pub record_id: String,
    pub recorded_at: String,
    pub cancelled: bool,
    #[serde(default = "default_feedback_sign")]
    pub sign: JudgeFeedSign,
    #[serde(default = "default_feedback_revision")]
    pub revision: u64,
    pub utterance: UtteranceFeedbackUtterance,
    pub observation_ids: Vec<String>,
    #[serde(default)]
    pub observations: Vec<serde_json::Value>,
    #[serde(default)]
    pub audio_segments: Vec<FeedbackAudioSegment>,
    #[serde(default)]
    pub observation_archive_paths: BTreeMap<String, String>,
    #[serde(default)]
    pub frame_archive_paths: BTreeMap<String, String>,
    pub judge: Option<FeedbackJudge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_unavailable_reason: Option<String>,
    pub llm: Option<FeedbackLlmDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_call_id: Option<String>,
    pub reason_code: FeedbackReasonCode,
    #[serde(default)]
    pub free_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed: Option<FeedbackFeed>,
    pub archive: FeedbackArchive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackFeedStatus {
    Pending,
    Applied,
    NotApplied,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackFeed {
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    pub sign: JudgeFeedSign,
    pub strength: f64,
    pub status: FeedbackFeedStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UtteranceFeedbackUtterance {
    pub id: String,
    pub created_at: String,
    pub message: String,
    pub message_kind: String,
}

impl From<&ConversationEntry> for UtteranceFeedbackUtterance {
    fn from(entry: &ConversationEntry) -> Self {
        Self {
            id: entry.id.clone(),
            created_at: entry.created_at.clone(),
            message: sanitize_text(&entry.message),
            message_kind: entry
                .message_kind
                .map_or_else(|| "chat".to_owned(), |kind| kind.as_wire().to_owned()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackAudioSegment {
    pub id: String,
    pub time: String,
    pub source: AudioObservationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackJudge {
    pub modules: Vec<FeedbackJudgeModule>,
    pub composition: Option<JudgeDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackJudgeModule {
    pub module_index: usize,
    pub novelty: Option<f64>,
    pub relevance: Option<f64>,
    pub action: Option<JudgeAction>,
    pub readiness: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackLlmDecision {
    pub emit: bool,
    pub message_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackArchiveStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackArchive {
    pub directory: String,
    pub status: FeedbackArchiveStatus,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackTrigger {
    UserReply,
    Proactive,
    #[default]
    Unknown,
}

#[derive(Debug, Clone)]
pub struct UtteranceFeedbackInput {
    pub trigger: FeedbackTrigger,
    pub utterance: ConversationEntry,
    pub observation_ids: Vec<String>,
    pub observations: Vec<ObservationRecord>,
    pub judge_trace: Option<JudgeTrace>,
    pub judge_decision: Option<JudgeDecision>,
    pub llm_decision: Option<FeedbackLlmDecision>,
    pub llm_call_id: Option<String>,
    pub reason_code: FeedbackReasonCode,
    pub free_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UtteranceFeedbackResult {
    Recorded(UtteranceFeedbackRecord),
    AlreadyRecorded(UtteranceFeedbackRecord),
    Cancelled(UtteranceFeedbackRecord),
}

#[derive(Debug, Error)]
pub enum UtteranceFeedbackError {
    #[error("発言評価の入力が不正です: {0}")]
    Invalid(String),
    #[error("発言評価の永続化に失敗しました: {0}")]
    Persistence(#[from] PersistenceError),
}

#[derive(Debug, Clone)]
pub struct UtteranceFeedbackStore {
    path: PathBuf,
    archive_directory: PathBuf,
    observation_directory: PathBuf,
    transcript_directory: PathBuf,
    frame_directory: PathBuf,
    debug_directory: PathBuf,
}

impl UtteranceFeedbackStore {
    pub fn from_paths(paths: &ConfigPaths) -> Self {
        Self {
            path: paths.utterance_feedback.clone(),
            archive_directory: paths.utterance_feedback_archive.clone(),
            observation_directory: paths.observations.clone(),
            transcript_directory: paths.transcripts.clone(),
            frame_directory: paths.frame_buffer.clone(),
            debug_directory: paths.debug.clone(),
        }
    }

    pub fn record(
        &self,
        input: UtteranceFeedbackInput,
    ) -> Result<UtteranceFeedbackResult, UtteranceFeedbackError> {
        self.record_at_with_sign(input, Utc::now(), JudgeFeedSign::Negative, false)
    }

    pub fn record_at(
        &self,
        input: UtteranceFeedbackInput,
        recorded_at: DateTime<Utc>,
    ) -> Result<UtteranceFeedbackResult, UtteranceFeedbackError> {
        self.record_at_with_sign(input, recorded_at, JudgeFeedSign::Negative, false)
    }

    pub fn record_with_sign(
        &self,
        input: UtteranceFeedbackInput,
        sign: JudgeFeedSign,
    ) -> Result<UtteranceFeedbackResult, UtteranceFeedbackError> {
        self.record_at_with_sign(input, Utc::now(), sign, false)
    }

    pub fn revise_with_sign(
        &self,
        input: UtteranceFeedbackInput,
        sign: JudgeFeedSign,
    ) -> Result<UtteranceFeedbackResult, UtteranceFeedbackError> {
        self.record_at_with_sign(input, Utc::now(), sign, true)
    }

    fn record_at_with_sign(
        &self,
        input: UtteranceFeedbackInput,
        recorded_at: DateTime<Utc>,
        sign: JudgeFeedSign,
        revise: bool,
    ) -> Result<UtteranceFeedbackResult, UtteranceFeedbackError> {
        validate_input(&input)?;
        let free_text = normalize_free_text(input.free_text.clone())?;
        let _operation_lock = SiblingLock::acquire(&self.operation_lock_path())?;
        let previous = self.latest_for_utterance(&input.utterance.id)?;
        if let Some(previous) = &previous {
            if !previous.cancelled && !revise {
                return Ok(UtteranceFeedbackResult::AlreadyRecorded(previous.clone()));
            }
            if !previous.cancelled
                && revise
                && previous.sign == sign
                && previous.reason_code == input.reason_code
                && previous.free_text.as_deref() == free_text.as_deref()
            {
                return Ok(UtteranceFeedbackResult::AlreadyRecorded(previous.clone()));
            }
        }
        let record_id = Uuid::new_v4().to_string();
        validate_record_id(&record_id)?;
        let revision = previous
            .as_ref()
            .map_or(1, |record| record.revision.saturating_add(1));
        let event_id = previous
            .as_ref()
            .and_then(|record| record.feed.as_ref())
            .map(|feed| feed.event_id.clone())
            .unwrap_or_else(|| format!("coosenpai:explicit:{}", Uuid::new_v4()));
        let judge_input_id = input
            .judge_trace
            .as_ref()
            .map(|trace| trace.input_id.clone())
            .or_else(|| {
                input
                    .judge_decision
                    .as_ref()
                    .map(|decision| decision.input_id.clone())
            });
        let source_ids = if input.observation_ids.is_empty() {
            unique_ids(
                input
                    .utterance
                    .observation_ids()
                    .chain(input.observations.iter().map(ObservationRecord::id)),
            )
        } else {
            unique_ids(input.observation_ids.iter().map(String::as_str))
        };
        validate_source_references(&input, &source_ids)?;
        let context = MaterialContext::collect(MaterialCollectionInput {
            observation_directory: &self.observation_directory,
            transcript_directory: &self.transcript_directory,
            frame_directory: &self.frame_directory,
            debug_directory: &self.debug_directory,
            source_ids: &source_ids,
            provided_observations: &input.observations,
            judge_trace: input.judge_trace.as_ref(),
            judge_input_id: input
                .judge_decision
                .as_ref()
                .map(|decision| decision.input_id.as_str()),
            llm_call_id: input.llm_call_id.as_deref(),
        });
        let judge_unavailable_reason = judge_unavailable_reason(
            &source_ids,
            &context.observations,
            input.judge_trace.as_ref(),
            input.judge_decision.as_ref(),
        );
        let mut context = context;
        if let Some(reason) = judge_unavailable_reason.as_deref() {
            context.issues.push(format!("judge-unavailable:{reason}"));
        }
        let prepared = self.prepare_archive(
            &record_id,
            &input.utterance,
            &source_ids,
            &context,
            sign,
            revision,
        );
        let mut record = UtteranceFeedbackRecord {
            trigger: if input.trigger == FeedbackTrigger::Unknown {
                previous
                    .as_ref()
                    .map_or(FeedbackTrigger::Unknown, |record| record.trigger)
            } else {
                input.trigger
            },
            schema_version: UTTERANCE_FEEDBACK_SCHEMA_VERSION,
            record_id,
            recorded_at: timestamp(recorded_at),
            cancelled: false,
            sign,
            revision,
            utterance: UtteranceFeedbackUtterance::from(&input.utterance),
            observation_ids: source_ids,
            observations: context
                .observations
                .iter()
                .map(|observation| project_observation(observation, &context.frame_archive_paths))
                .collect(),
            audio_segments: context.audio_segments,
            observation_archive_paths: context.observation_archive_paths,
            frame_archive_paths: context.frame_archive_paths,
            judge: feedback_judge(input.judge_trace.as_ref(), input.judge_decision.as_ref()),
            judge_unavailable_reason,
            llm: input.llm_decision.map(|decision| FeedbackLlmDecision {
                emit: decision.emit,
                message_kind: sanitize_text(&decision.message_kind),
            }),
            llm_call_id: input.llm_call_id.as_deref().map(sanitize_text),
            reason_code: input.reason_code,
            free_text: free_text.as_deref().map(sanitize_text),
            feed: Some(FeedbackFeed {
                event_id,
                input_id: judge_input_id,
                sign,
                strength: 1.0,
                status: FeedbackFeedStatus::Pending,
                reason: None,
            }),
            archive: FeedbackArchive {
                directory: prepared.directory.clone(),
                status: if prepared.issues.is_empty() {
                    FeedbackArchiveStatus::Complete
                } else {
                    FeedbackArchiveStatus::Partial
                },
                files: prepared.files.keys().cloned().collect(),
                issues: prepared.issues.clone(),
            },
        };
        match security::validate_publication(&record, &prepared.files) {
            Ok(()) => {
                if let Err(error) = publish_archive(&prepared.path, prepared.files) {
                    record.archive.status = if matches!(error, PersistenceError::Invalid(_)) {
                        FeedbackArchiveStatus::Partial
                    } else {
                        FeedbackArchiveStatus::Failed
                    };
                    record
                        .archive
                        .issues
                        .push(sanitize_text(&format!("archive-failed:{error}")));
                    record.archive.files.clear();
                }
            }
            Err(category) => {
                record = rejected_publication_record(record, category);
                if security::record_is_forbidden(&record) {
                    return Err(UtteranceFeedbackError::Invalid(
                        "発言評価の記録に除去できない秘密が残っています".to_owned(),
                    ));
                }
            }
        }
        JsonlStore::new(self.path.clone()).append(&record)?;
        Ok(UtteranceFeedbackResult::Recorded(record))
    }

    pub fn cancel(
        &self,
        utterance_id: &str,
        recorded_at: DateTime<Utc>,
    ) -> Result<UtteranceFeedbackResult, UtteranceFeedbackError> {
        if utterance_id.trim().is_empty() {
            return Err(UtteranceFeedbackError::Invalid(
                "発言 ID が空です".to_owned(),
            ));
        }
        let _operation_lock = SiblingLock::acquire(&self.operation_lock_path())?;
        let Some(previous) = self.latest_for_utterance(utterance_id)? else {
            return Err(UtteranceFeedbackError::Invalid(
                "記録済みの発言評価がありません".to_owned(),
            ));
        };
        if previous.cancelled {
            return Ok(UtteranceFeedbackResult::Cancelled(previous));
        }
        let mut record = previous;
        record.recorded_at = timestamp(recorded_at);
        record.cancelled = true;
        record.revision = record.revision.saturating_add(1);
        if record.feed.is_none() {
            record.feed = Some(FeedbackFeed {
                event_id: format!("coosenpai:explicit:{}", Uuid::new_v4()),
                input_id: None,
                sign: record.sign,
                strength: 1.0,
                status: FeedbackFeedStatus::Pending,
                reason: None,
            });
        } else if let Some(feed) = record.feed.as_mut() {
            feed.status = FeedbackFeedStatus::Pending;
            feed.reason = None;
        }
        JsonlStore::new(self.path.clone()).append(&record)?;
        Ok(UtteranceFeedbackResult::Cancelled(record))
    }

    pub fn update_feed(
        &self,
        record_id: &str,
        input_id: Option<String>,
        status: FeedbackFeedStatus,
        reason: Option<String>,
    ) -> Result<UtteranceFeedbackRecord, UtteranceFeedbackError> {
        validate_record_id(record_id).map_err(UtteranceFeedbackError::Persistence)?;
        if input_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(UtteranceFeedbackError::Invalid(
                "判断役の input ID が空です".to_owned(),
            ));
        }
        let _operation_lock = SiblingLock::acquire(&self.operation_lock_path())?;
        let records = JsonlStore::new(self.path.clone()).read::<UtteranceFeedbackRecord>()?;
        let Some(mut record) = records
            .into_iter()
            .rev()
            .find(|record| record.record_id == record_id)
        else {
            return Err(UtteranceFeedbackError::Invalid(
                "発言評価の記録がありません".to_owned(),
            ));
        };
        let Some(feed) = record.feed.as_mut() else {
            return Err(UtteranceFeedbackError::Invalid(
                "発言評価に feed event がありません".to_owned(),
            ));
        };
        if let Some(input_id) = input_id {
            feed.input_id = Some(input_id);
        }
        feed.status = status;
        feed.reason = reason
            .filter(|value| !value.trim().is_empty())
            .map(|value| sanitize_text(&value));
        JsonlStore::new(self.path.clone()).append(&record)?;
        Ok(record)
    }

    pub fn latest_for_utterance(
        &self,
        utterance_id: &str,
    ) -> Result<Option<UtteranceFeedbackRecord>, UtteranceFeedbackError> {
        if utterance_id.trim().is_empty() {
            return Ok(None);
        }
        Ok(JsonlStore::new(self.path.clone())
            .read::<UtteranceFeedbackRecord>()?
            .into_iter()
            .rev()
            .find(|record| record.utterance.id == utterance_id))
    }

    pub fn recorded_utterance_ids(&self) -> Result<HashSet<String>, UtteranceFeedbackError> {
        let mut ids = HashSet::new();
        for record in JsonlStore::new(self.path.clone()).read::<UtteranceFeedbackRecord>()? {
            if record.cancelled {
                ids.remove(&record.utterance.id);
            } else {
                ids.insert(record.utterance.id);
            }
        }
        Ok(ids)
    }

    fn operation_lock_path(&self) -> PathBuf {
        self.path
            .with_file_name(".utterance-feedback.operation.lock")
    }

    pub(crate) fn cleanup_stale_archives(&self) -> Result<(), UtteranceFeedbackError> {
        archive::cleanup_stale_archive_temps(&self.archive_directory, &self.path)
            .map_err(PersistenceError::Io)?;
        Ok(())
    }

    fn prepare_archive(
        &self,
        record_id: &str,
        utterance: &ConversationEntry,
        observation_ids: &[String],
        context: &MaterialContext,
        sign: JudgeFeedSign,
        revision: u64,
    ) -> PreparedArchive {
        let directory = format!("{UTTERANCE_FEEDBACK_ARCHIVE_DIRECTORY}/{record_id}");
        let path = self.archive_directory.join(record_id);
        let mut files = context.materials.clone();
        let mut issues = context
            .issues
            .iter()
            .map(|issue| sanitize_text(issue))
            .collect::<Vec<_>>();
        if let Ok(value) = serde_json::to_value(UtteranceFeedbackUtterance::from(utterance)) {
            let bytes = serde_json::to_vec(&sanitize_json(&value)).unwrap_or_default();
            files.insert("utterance.json".to_owned(), bytes);
        } else {
            issues.push("utterance-serialize-failed".to_owned());
        }
        let mut file_names = files.keys().cloned().collect::<Vec<_>>();
        file_names.push("manifest.json".to_owned());
        let manifest = json!({
            "schemaVersion": UTTERANCE_FEEDBACK_SCHEMA_VERSION,
            "commitMarker": "complete",
            "recordId": record_id,
            "utteranceId": utterance.id,
            "observationIds": observation_ids,
            "sign": sign,
            "revision": revision,
            "files": file_names,
            "issues": issues,
        });
        match serde_json::to_vec(&manifest) {
            Ok(bytes) => {
                files.insert("manifest.json".to_owned(), bytes);
            }
            Err(error) => issues.push(sanitize_text(&format!("manifest-serialize-failed:{error}"))),
        }
        PreparedArchive {
            directory,
            path,
            files,
            issues,
        }
    }
}

/// 別端末へ渡す bundle 用に、既存の発言評価 scrubber を全 JSON 値へ適用する。
/// キーは archive の相対参照を残すため削除せず、値だけを scrub する。
pub fn sanitize_feedback_export(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_feedback_export(value)))
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(sanitize_feedback_export).collect())
        }
        serde_json::Value::String(value) => serde_json::Value::String(sanitize_text(value)),
        value => value.clone(),
    }
}

pub fn sanitize_feedback_export_text(value: &str) -> String {
    sanitize_text(value)
}

pub fn is_allowed_feedback_archive_path(path: &str) -> bool {
    security::allowed_material_path(path)
}

struct PreparedArchive {
    directory: String,
    path: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
    issues: Vec<String>,
}

/// 公開前検証の拒否時は raw 材料を保存せず、 sanitized 済みの発言概要と理由だけを残す。
fn rejected_publication_record(
    mut record: UtteranceFeedbackRecord,
    category: &str,
) -> UtteranceFeedbackRecord {
    record.observation_ids = Vec::new();
    record.observations = Vec::new();
    record.audio_segments = Vec::new();
    record.observation_archive_paths = BTreeMap::new();
    record.frame_archive_paths = BTreeMap::new();
    record.judge = None;
    record.llm = None;
    record.llm_call_id = None;
    record.archive.status = FeedbackArchiveStatus::Partial;
    record.archive.files = Vec::new();
    record.archive.issues = vec![format!("archive-rejected:{category}")];
    record
}

fn validate_input(input: &UtteranceFeedbackInput) -> Result<(), UtteranceFeedbackError> {
    if !input.utterance.is_normal_speech() {
        return Err(UtteranceFeedbackError::Invalid(
            "通常の Coo 発言だけ評価できます".to_owned(),
        ));
    }
    if input.utterance.id.trim().is_empty() {
        return Err(UtteranceFeedbackError::Invalid(
            "発言 ID が空です".to_owned(),
        ));
    }
    if input.reason_code != FeedbackReasonCode::Other && input.free_text.is_some() {
        return Err(UtteranceFeedbackError::Invalid(
            "自由記述はその他の理由でだけ指定できます".to_owned(),
        ));
    }
    if input.observation_ids.iter().any(|id| id.trim().is_empty()) {
        return Err(UtteranceFeedbackError::Invalid(
            "観測 ID が空です".to_owned(),
        ));
    }
    if input
        .observations
        .iter()
        .any(|observation| observation.id().trim().is_empty())
    {
        return Err(UtteranceFeedbackError::Invalid(
            "観測の ID が空です".to_owned(),
        ));
    }
    if let (Some(trace), Some(decision)) = (&input.judge_trace, &input.judge_decision) {
        if trace.input_id != decision.input_id {
            return Err(UtteranceFeedbackError::Invalid(
                "判断役の trace と結果の input ID が一致しません".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_source_references(
    input: &UtteranceFeedbackInput,
    source_ids: &[String],
) -> Result<(), UtteranceFeedbackError> {
    if input.observation_ids.len() != source_ids.len() && !input.observation_ids.is_empty() {
        return Err(UtteranceFeedbackError::Invalid(
            "観測 ID に空または重複があります".to_owned(),
        ));
    }
    if input
        .observations
        .iter()
        .any(|observation| !source_ids.iter().any(|id| id == observation.id()))
    {
        return Err(UtteranceFeedbackError::Invalid(
            "発言に紐づかない観測を指定できません".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_free_text(value: Option<String>) -> Result<Option<String>, UtteranceFeedbackError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > UTTERANCE_FEEDBACK_FREE_TEXT_MAX_CHARS {
        return Err(UtteranceFeedbackError::Invalid(format!(
            "自由記述は {UTTERANCE_FEEDBACK_FREE_TEXT_MAX_CHARS} 文字以内で指定してください"
        )));
    }
    Ok(Some(value))
}

fn feedback_judge(
    trace: Option<&JudgeTrace>,
    fallback: Option<&JudgeDecision>,
) -> Option<FeedbackJudge> {
    let composition = trace
        .and_then(|trace| trace.decision.clone())
        .or_else(|| fallback.cloned());
    let has_module_result = trace.is_some_and(|trace| {
        trace
            .responses
            .iter()
            .any(|module| module.evaluation.is_some())
    });
    if composition.is_none() && !has_module_result {
        return None;
    }
    let modules = trace
        .map(|trace| {
            trace
                .responses
                .iter()
                .map(|module| FeedbackJudgeModule {
                    module_index: module.module_index,
                    novelty: module.evaluation.as_ref().and_then(|value| value.novelty),
                    relevance: module.evaluation.as_ref().and_then(|value| value.relevance),
                    action: module.evaluation.as_ref().map(|value| value.action),
                    readiness: module
                        .evaluation
                        .as_ref()
                        .map(|value| value.readiness.clone()),
                })
                .collect()
        })
        .unwrap_or_default();
    let judge = FeedbackJudge {
        modules,
        composition,
    };
    serde_json::to_value(judge)
        .ok()
        .map(|value| sanitize_json(&value))
        .and_then(|value| serde_json::from_value(value).ok())
}

fn judge_unavailable_reason(
    source_ids: &[String],
    observations: &[ObservationRecord],
    trace: Option<&JudgeTrace>,
    decision: Option<&JudgeDecision>,
) -> Option<String> {
    if source_ids.is_empty() {
        return Some("observation-id-missing".to_owned());
    }
    if observations.is_empty() {
        return Some("observation-not-found".to_owned());
    }
    if trace.is_none() && decision.is_none() {
        return Some("judge-not-run".to_owned());
    }
    None
}

fn validate_record_id(record_id: &str) -> Result<(), PersistenceError> {
    if Uuid::parse_str(record_id).is_err() {
        return Err(PersistenceError::Invalid(
            "発言評価の record_id が不正です".to_owned(),
        ));
    }
    Ok(())
}

fn unique_ids<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.filter(|id| !id.trim().is_empty() && seen.insert(*id))
        .map(str::to_owned)
        .collect()
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
