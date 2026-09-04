use crate::companion::AttachmentOcrFailureKind;
use crate::config::PENDING_DELIVERY_ITEM_MAX_BYTES;
use crate::persistence::PersistenceError;
use crate::state::{
    ConversationEntry, ConversationRole, ObservationRecord, PendingFrameContext, UserScreenContext,
};
use serde::{Deserialize, Serialize};

pub(crate) const OWNED_USER_ID_PREFIX: &str = "runtime-user-";

#[derive(Debug, Clone, Default)]
pub struct CursorSnapshot {
    pub user_operation_generation: u64,
    /// ユーザーキューの横取りを無効化する単調増加の世代。
    pub user_epoch: u64,
    /// 次に割り当てるユーザー入力の seq。
    pub next_user_seq: u64,
    /// 次に割り当てる user turn の dispatch seq。
    pub next_dispatch_seq: u64,
    /// 現在貸し出し中の有限 user batch。再起動後も同じ境界を再利用する。
    pub user_dispatch: Option<UserDispatchLease>,
    /// 予約済み TurnCommit。再起動時は phase と対象 ID を使って安全に recovery する。
    pub active_turn_commit: Option<ActiveTurnCommit>,
    pub ids: Vec<String>,
    pub pending: Vec<ObservationRecord>,
    pub failed: Vec<String>,
    pub observation_attempts: Vec<ObservationAttempt>,
    pub cancelled_input_ids: Vec<String>,
    pub pending_inputs: Vec<PendingInput>,
    pub pending_deliveries: Vec<PendingDelivery>,
    pub pending_frame_contexts: Vec<PendingFrameContext>,
    pub consumed_frame_context_ids: Vec<String>,
    /// TurnCommit で実際に消費した観察の監査記録。
    pub observation_consumptions: Vec<ObservationConsumption>,
    /// 起動時に不成立と判定した TurnCommit の recovery 記録。
    pub turn_commit_recovery_attempts: Vec<TurnCommitRecoveryAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserDispatchLease {
    pub dispatch_seq: u64,
    pub input_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveTurnCommit {
    pub turn_id: String,
    pub kind: TurnCommitKind,
    pub phase: TurnCommitPhase,
    pub target_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_user_epoch: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TurnCommitKind {
    User,
    Proactive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TurnCommitPhase {
    Reserved,
    Persisting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnCommitRecoveryAttempt {
    pub turn_id: String,
    pub kind: TurnCommitKind,
    pub target_ids: Vec<String>,
    pub reason: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationConsumption {
    pub observation_id: String,
    pub turn_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationAttempt {
    pub observation_id: String,
    pub attempts: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PendingInput {
    UserMessage(PendingUserMessage),
}

impl PendingInput {
    pub fn id(&self) -> &str {
        match self {
            Self::UserMessage(input) => &input.id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingUserMessage {
    pub id: String,
    #[serde(default)]
    pub user_seq: u64,
    pub created_at: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_text: Option<String>,
    pub observations: Vec<ObservationRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_frames: Vec<PendingFrameContext>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub observation_in_progress: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_response: Option<PreparedUserResponse>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub response_commit_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_failure: Option<PendingAttachmentFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tutorial_response_key: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl PendingUserMessage {
    pub fn attachment_is_terminal(&self) -> bool {
        self.attachment_failure
            .as_ref()
            .is_some_and(|failure| failure.terminal)
    }

    pub fn conversation_entry(&self) -> ConversationEntry {
        let screen_context = UserScreenContext {
            observations: self.observations.clone(),
            pending_frames: self.pending_frames.clone(),
        };
        ConversationEntry {
            schema_version: 1,
            id: self.id.clone(),
            created_at: self.created_at.clone(),
            role: ConversationRole::User,
            message: self.message.clone(),
            attachment_path: self.attachment_path.clone(),
            attachment_text: self.attachment_text.clone(),
            tutorial_response_key: self.tutorial_response_key.clone(),
            screen_context: (!screen_context.is_empty()).then_some(screen_context),
            caused_by_ids: Vec::new(),
            notification_priority: "none".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingAttachmentFailure {
    pub reason: AttachmentOcrFailureKind,
    pub attempts: u8,
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedUserResponse {
    pub id: String,
    pub created_at: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_summary: Option<String>,
}

impl PreparedUserResponse {
    pub fn conversation_entry(&self, input_ids: &[String]) -> ConversationEntry {
        ConversationEntry {
            schema_version: 1,
            id: self.id.clone(),
            created_at: self.created_at.clone(),
            role: ConversationRole::Companion,
            message: self.message.clone(),
            attachment_path: None,
            attachment_text: None,
            tutorial_response_key: None,
            screen_context: None,
            caused_by_ids: input_ids.to_vec(),
            notification_priority: "none".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingDelivery {
    pub remark_id: String,
    pub created_at: String,
    pub proactive_date: String,
    pub message: String,
    pub message_kind: String,
    pub notification_priority: String,
    pub observation_ids: Vec<String>,
    pub enqueued: bool,
}

impl PendingDelivery {
    pub fn payload_size_bytes(&self) -> usize {
        [
            self.remark_id.len(),
            self.created_at.len(),
            self.proactive_date.len(),
            self.message.len(),
            self.message_kind.len(),
            self.notification_priority.len(),
        ]
        .into_iter()
        .chain(self.observation_ids.iter().map(String::len))
        .fold(0, usize::saturating_add)
    }

    pub(crate) fn validate_payload_size(&self) -> Result<(), PersistenceError> {
        if self.payload_size_bytes() > PENDING_DELIVERY_ITEM_MAX_BYTES {
            return Err(PersistenceError::Invalid(
                "pending delivery の payload が上限を超えています".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn conversation_entry(&self) -> ConversationEntry {
        ConversationEntry {
            schema_version: 1,
            id: self.remark_id.clone(),
            created_at: self.created_at.clone(),
            role: ConversationRole::Companion,
            message: self.message.clone(),
            attachment_path: None,
            attachment_text: None,
            tutorial_response_key: None,
            screen_context: None,
            caused_by_ids: self.observation_ids.clone(),
            notification_priority: self.notification_priority.clone(),
        }
    }
}

