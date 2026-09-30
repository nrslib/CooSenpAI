use super::{CompanionError, CompanionResponse};
use crate::config::{local_date_at, ConfigPaths};
use crate::persistence::JsonlStore;
use crate::provider::{
    ProviderEventSink, ProviderMidTurnInput, ProviderResult, ProviderUsage, SessionRequest,
};
use crate::state::{
    ConversationEntry, ConversationMessageKind, ConversationRole, ObservationRecord,
};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOwnership {
    Owner,
    None,
}

pub(super) struct ProviderInvocation<'a> {
    pub prompt: &'a str,
    pub source_ids: &'a [String],
    pub interrupted_input_ids: &'a [String],
    pub user: bool,
    pub image_paths: &'a [PathBuf],
    pub session: SessionRequest,
    pub events: Option<Arc<dyn ProviderEventSink>>,
    pub additional_inputs: Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
    pub tutorial_response_key: Option<&'a str>,
    pub allowed_transcript_paths: Option<Vec<String>>,
}

pub(super) struct CompanionTurn {
    pub data: crate::prompts::CompanionPromptData,
    pub user: bool,
    pub observations: Vec<ObservationRecord>,
    pub image_paths: Vec<PathBuf>,
    pub events: Option<Arc<dyn ProviderEventSink>>,
    pub requested_source_ids: Vec<String>,
    pub user_operation_generation: Option<u64>,
    pub user_prompt_batch: Option<super::user_prompt::UserPromptBatch>,
    pub additional_inputs: Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
    pub accepted_mid_turn_ids: Option<Arc<Mutex<HashSet<String>>>>,
    pub tutorial_response_key: Option<String>,
}

pub(crate) struct CompanionCallOutcome {
    pub(crate) decision_produced: bool,
    pub(crate) deferred: bool,
    pub(crate) response: CompanionResponse,
    pub(crate) data: crate::prompts::CompanionPromptData,
    pub(crate) observations: Vec<ObservationRecord>,
    pub(crate) consumed_observations: Vec<ObservationRecord>,
    pub(crate) source_ids: Vec<String>,
    pub(crate) remark_created: bool,
    pub(crate) counted_emit: bool,
    pub(crate) usage: Option<ProviderUsage>,
    pub(crate) call_id: Option<String>,
}

/// 自発発言の確定結果。`consumed_ids` は消費済み観測、`utterance_observation_ids` は
/// 発話を実際に生成した観測の ID で、発言評価が発言と LLM 判断を対応付ける正本。
pub(crate) struct ProactiveCommitOutcome {
    pub(crate) response: CompanionResponse,
    pub(crate) consumed_ids: Vec<String>,
    pub(crate) call_id: Option<String>,
    pub(crate) utterance_observation_ids: Vec<String>,
}

pub(super) struct ProviderCallOutcome {
    pub response: CompanionResponse,
    pub usage: Option<ProviderUsage>,
    pub call_id: String,
}

pub(super) struct ProviderTurn<'a> {
    pub work_result: Option<&'a str>,
    pub data: &'a crate::prompts::CompanionPromptData,
    pub user: bool,
    pub image_paths: &'a [PathBuf],
    pub events: Option<Arc<dyn ProviderEventSink>>,
    pub source_ids: &'a [String],
    pub interrupted_input_ids: &'a [String],
    pub additional_inputs: Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
    pub tutorial_response_key: Option<&'a str>,
}

pub(super) struct MeasuredProviderEvents {
    downstream: Option<Arc<dyn ProviderEventSink>>,
    usage: Mutex<Option<ProviderUsage>>,
}

impl MeasuredProviderEvents {
    pub(super) fn new(downstream: Option<Arc<dyn ProviderEventSink>>) -> Self {
        Self {
            downstream,
            usage: Mutex::new(None),
        }
    }

    pub(super) fn measured_usage(&self) -> Option<ProviderUsage> {
        self.usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ProviderEventSink for MeasuredProviderEvents {
    fn delta(&self, text: &str) {
        if let Some(downstream) = &self.downstream {
            downstream.delta(text);
        }
    }

    fn usage(&self, usage: &ProviderUsage) {
        *self
            .usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(usage.clone());
        if let Some(downstream) = &self.downstream {
            downstream.usage(usage);
        }
    }

    fn reset(&self) {
        if let Some(downstream) = &self.downstream {
            downstream.reset();
        }
    }

    fn mid_turn_accepted(&self, source_id: &str) {
        if let Some(downstream) = &self.downstream {
            downstream.mid_turn_accepted(source_id);
        }
    }

    fn message_committed(&self) {
        if let Some(downstream) = &self.downstream {
            downstream.message_committed();
        }
    }
}

pub(super) fn session_mode(session: &SessionRequest) -> &'static str {
    match session {
        SessionRequest::New => "new",
        SessionRequest::Resume(_) => "resume",
        SessionRequest::Ephemeral | SessionRequest::Isolated => "ephemeral",
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompanionProviderFields {
    #[serde(default)]
    work_request: Option<crate::work::WorkProposal>,
    #[serde(default, deserialize_with = "crate::emotion::optional_delta")]
    emotion_delta: Option<crate::emotion::EmotionDelta>,
    emit: bool,
    message: Option<String>,
    message_kind: String,
    notification_priority: String,
    #[serde(default)]
    fact_candidates: Vec<serde_json::Value>,
    #[serde(default)]
    fact_updates: Vec<serde_json::Value>,
    #[serde(default)]
    speaker_name_proposals: Vec<crate::speaker_name_proposals::SpeakerNameProposalCandidate>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObservationCompanionEnvelope {
    #[serde(flatten)]
    fields: CompanionProviderFields,
    thought: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserCompanionEnvelope {
    #[serde(flatten)]
    fields: CompanionProviderFields,
}

impl CompanionProviderFields {
    fn into_response(self, thought: Option<String>) -> CompanionResponse {
        CompanionResponse {
            work_request: self.work_request,
            emotion_delta: self.emotion_delta,
            emit: self.emit,
            message: self.message,
            message_kind: self.message_kind,
            notification_priority: self.notification_priority,
            thought,
            fact_candidates: self.fact_candidates,
            fact_updates: self.fact_updates,
            speaker_name_proposals: self.speaker_name_proposals,
        }
    }
}

pub(super) fn parse_response(
    result: &ProviderResult,
    user_response: bool,
) -> Result<CompanionResponse, CompanionError> {
    let value = result.value.clone().ok_or(CompanionError::Output)?;
    let response = if user_response {
        if value
            .as_object()
            .is_some_and(|object| object.contains_key("thought"))
        {
            return Err(CompanionError::Output);
        }
        let envelope: UserCompanionEnvelope =
            serde_json::from_value(value).map_err(|_| CompanionError::Output)?;
        envelope.fields.into_response(None)
    } else {
        let envelope: ObservationCompanionEnvelope =
            serde_json::from_value(value).map_err(|_| CompanionError::Output)?;
        if envelope.thought.trim().is_empty()
            || envelope.thought.chars().count() > 500
            || envelope.thought.contains('\n')
            || envelope.thought.contains('\r')
        {
            return Err(CompanionError::Output);
        }
        envelope.fields.into_response(Some(envelope.thought))
    };
    if !matches!(
        response.notification_priority.as_str(),
        "none" | "info" | "warning" | "critical"
    ) || !matches!(
        response.message_kind.as_str(),
        "advice" | "encouragement" | "nudge" | "celebration" | "summary" | "chat"
    ) || (response.emit && response.message.as_deref().unwrap_or("").is_empty())
    {
        return Err(CompanionError::Output);
    }
    Ok(response)
}

pub(super) fn require_user_message(response: &CompanionResponse) -> Result<String, CompanionError> {
    if response.emit
        && response.message_kind == "chat"
        && response.notification_priority == "none"
        && response
            .message
            .as_deref()
            .is_some_and(|message| !message.trim().is_empty())
    {
        return Ok(response.message.clone().unwrap_or_default());
    }
    Err(CompanionError::Output)
}

pub(crate) fn silent_response() -> CompanionResponse {
    CompanionResponse {
        work_request: None,
        emotion_delta: None,
        emit: false,
        message: None,
        message_kind: "advice".to_owned(),
        notification_priority: "none".to_owned(),
        thought: None,
        fact_candidates: Vec::new(),
        fact_updates: Vec::new(),
        speaker_name_proposals: Vec::new(),
    }
}

pub fn conversation_store(paths: &ConfigPaths) -> JsonlStore {
    conversation_store_at(paths, Utc::now())
}

pub fn conversation_store_at(paths: &ConfigPaths, now: DateTime<Utc>) -> JsonlStore {
    let date = local_date_at(now);
    JsonlStore::new(paths.conversation.join(format!("{date}.jsonl")))
}

pub fn conversation_entry(
    role: ConversationRole,
    message: String,
    priority: &str,
) -> ConversationEntry {
    conversation_entry_at(Utc::now(), role, message, priority)
}

pub(super) fn conversation_entry_at(
    now: DateTime<Utc>,
    role: ConversationRole,
    message: String,
    priority: &str,
) -> ConversationEntry {
    conversation_entry_with_causes_at(now, role, message, priority, Vec::new())
}

pub(super) fn conversation_entry_with_causes_at(
    now: DateTime<Utc>,
    role: ConversationRole,
    message: String,
    priority: &str,
    caused_by_ids: Vec<String>,
) -> ConversationEntry {
    conversation_entry_with_kind_and_causes_at(
        now,
        role,
        message,
        priority,
        ConversationMessageKind::Chat,
        caused_by_ids,
    )
}

pub(super) fn conversation_entry_with_kind_and_causes_at(
    now: DateTime<Utc>,
    role: ConversationRole,
    message: String,
    priority: &str,
    message_kind: ConversationMessageKind,
    caused_by_ids: Vec<String>,
) -> ConversationEntry {
    ConversationEntry {
        schema_version: 1,
        id: Uuid::new_v4().to_string(),
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        role,
        message,
        message_kind: Some(message_kind),
        attachment_path: None,
        attachment_text: None,
        tutorial_response_key: None,
        screen_context: None,
        caused_by_ids,
        notification_priority: priority.to_owned(),
    }
}
