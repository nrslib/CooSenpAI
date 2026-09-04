use super::{CompanionError, CompanionResponse};
use crate::config::{local_date_at, ConfigPaths};
use crate::persistence::JsonlStore;
use crate::provider::{
    ProviderEventSink, ProviderMidTurnInput, ProviderResult, ProviderUsage, SessionRequest,
};
use crate::state::{ConversationEntry, ConversationRole, ObservationRecord};
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
    pub user: bool,
    pub image_paths: &'a [PathBuf],
    pub session: SessionRequest,
    pub events: Option<Arc<dyn ProviderEventSink>>,
    pub additional_inputs: Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
    pub tutorial_response_key: Option<&'a str>,
}

pub(super) struct CompanionTurn {
    pub data: crate::prompts::CompanionPromptData,
    pub user: bool,
    pub observations: Vec<ObservationRecord>,
    pub image_paths: Vec<PathBuf>,
    pub events: Option<Arc<dyn ProviderEventSink>>,
    pub requested_source_ids: Vec<String>,
    pub additional_inputs: Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
    pub accepted_mid_turn_ids: Option<Arc<Mutex<HashSet<String>>>>,
    pub tutorial_response_key: Option<String>,
}

pub(crate) struct CompanionCallOutcome {
    pub(crate) response: CompanionResponse,
    pub(crate) data: crate::prompts::CompanionPromptData,
    pub(crate) observations: Vec<ObservationRecord>,
    pub(crate) consumed_observations: Vec<ObservationRecord>,
    pub(crate) source_ids: Vec<String>,
    pub(crate) remark_created: bool,
    pub(crate) counted_emit: bool,
    pub(crate) usage: Option<ProviderUsage>,
}

pub(super) struct ProviderCallOutcome {
    pub response: CompanionResponse,
    pub usage: Option<ProviderUsage>,
}

pub(super) struct ProviderTurn<'a> {
    pub data: &'a crate::prompts::CompanionPromptData,
    pub user: bool,
    pub image_paths: &'a [PathBuf],
    pub events: Option<Arc<dyn ProviderEventSink>>,
    pub source_ids: &'a [String],
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
}

pub(super) fn session_mode(session: &SessionRequest) -> &'static str {
    match session {
        SessionRequest::New => "new",
        SessionRequest::Resume(_) => "resume",
        SessionRequest::Ephemeral => "ephemeral",
    }
}

pub(super) fn parse_response(result: &ProviderResult) -> Result<CompanionResponse, CompanionError> {
    let value = result.value.clone().ok_or(CompanionError::Output)?;
    let response: CompanionResponse =
        serde_json::from_value(value).map_err(|_| CompanionError::Output)?;
    if !matches!(
        response.notification_priority.as_str(),
        "none" | "info" | "warning" | "critical"
    ) || !matches!(
        response.message_kind.as_str(),
        "advice" | "encouragement" | "nudge" | "celebration" | "summary" | "chat"
    ) || (response.emit && response.message.as_deref().unwrap_or("").is_empty())
        || response.thought.as_deref().is_some_and(|value| {
            value.trim().is_empty()
                || value.chars().count() > 500
                || value.contains('\n')
                || value.contains('\r')
        })
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
        emit: false,
        message: None,
        message_kind: "advice".to_owned(),
        notification_priority: "none".to_owned(),
        thought: None,
        fact_candidates: Vec::new(),
        fact_updates: Vec::new(),
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
    ConversationEntry {
        schema_version: 1,
        id: Uuid::new_v4().to_string(),
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        role,
        message,
        attachment_path: None,
        attachment_text: None,
        tutorial_response_key: None,
        screen_context: None,
        caused_by_ids,
        notification_priority: priority.to_owned(),
    }
}
