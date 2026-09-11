use crate::companion::AttachmentOcrFailureKind;
use crate::config::PENDING_DELIVERY_ITEM_MAX_BYTES;
use crate::persistence::PersistenceError;
use crate::state::{
    ConversationEntry, ConversationRole, ObservationRecord, PendingFrameContext, UserScreenContext,
};
use serde::{de::Error as DeError, ser::Error as SerError, Deserialize, Serialize};
use serde_json::Value;

pub const MAX_USER_RESPONSE_ATTEMPTS: u8 = 3;

pub(crate) const OWNED_USER_ID_PREFIX: &str = "runtime-user-";

#[derive(Debug, Clone)]
pub struct CursorSnapshot {
    pub emotion_updates_enabled: bool,
    pub companion_emotions: crate::emotion::EmotionState,
    pub emotion_epoch: u64,
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
    pub pending: Vec<PendingObservation>,
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

impl Default for CursorSnapshot {
    fn default() -> Self {
        Self {
            companion_emotions: Default::default(),
            emotion_epoch: 0,
            emotion_updates_enabled: true,
            user_operation_generation: 0,
            user_epoch: 0,
            next_user_seq: 0,
            next_dispatch_seq: 0,
            user_dispatch: None,
            active_turn_commit: None,
            ids: Vec::new(),
            pending: Vec::new(),
            failed: Vec::new(),
            observation_attempts: Vec::new(),
            cancelled_input_ids: Vec::new(),
            pending_inputs: Vec::new(),
            pending_deliveries: Vec::new(),
            pending_frame_contexts: Vec::new(),
            consumed_frame_context_ids: Vec::new(),
            observation_consumptions: Vec::new(),
            turn_commit_recovery_attempts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserDispatchLease {
    #[serde(default)]
    pub conversation_generation: u64,
    pub dispatch_seq: u64,
    pub input_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveTurnCommit {
    #[serde(default)]
    pub conversation_generation: u64,
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
    pub conversation_generation: u64,
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
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::hearing_context::deserialize_contexts"
    )]
    pub hearing_context: Vec<crate::hearing_context::HearingContext>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "crate::hearing_context::deserialize_pending_audio"
    )]
    pub pending_audio: Vec<crate::state::AudioObservation>,
    /// 音声本文は共有 journal を正本にし、cursor には参照 ID だけを保存する。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_audio_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub observation_in_progress: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_response: Option<PreparedUserResponse>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub response_commit_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_failure: Option<PendingAttachmentFailure>,
    #[serde(default)]
    pub response_attempts: u8,
    #[serde(default, skip_serializing_if = "is_false")]
    pub response_terminal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tutorial_response_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingObservation {
    pub conversation_generation: u64,
    pub observation: ObservationRecord,
}

impl PendingObservation {
    pub fn new(conversation_generation: u64, observation: ObservationRecord) -> Self {
        Self {
            conversation_generation,
            observation,
        }
    }

    pub fn id(&self) -> &str {
        self.observation.id()
    }

    pub fn source_frame_ids(&self) -> &[String] {
        self.observation.source_frame_ids()
    }
}

impl Serialize for PendingObservation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut value = serde_json::to_value(&self.observation).map_err(S::Error::custom)?;
        let object = value.as_object_mut().ok_or_else(|| {
            S::Error::custom("pending observation は JSON object でなければなりません")
        })?;
        object.insert(
            "conversationGeneration".to_owned(),
            Value::from(self.conversation_generation),
        );
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PendingObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        let object = value.as_object_mut().ok_or_else(|| {
            D::Error::custom("pending observation は JSON object でなければなりません")
        })?;
        let conversation_generation = match object.remove("conversationGeneration") {
            Some(value) => value
                .as_u64()
                .ok_or_else(|| D::Error::custom("pending observation の世代が不正です"))?,
            None => 0,
        };
        let observation = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self {
            conversation_generation,
            observation,
        })
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl PendingUserMessage {
    pub fn pending_audio_ids(&self) -> Vec<String> {
        let mut ids = std::collections::HashSet::new();
        self.pending_audio_ids
            .iter()
            .chain(self.pending_audio.iter().map(|audio| &audio.id))
            .filter(|id| ids.insert((*id).clone()))
            .cloned()
            .collect()
    }

    pub fn attachment_is_terminal(&self) -> bool {
        self.attachment_failure
            .as_ref()
            .is_some_and(|failure| failure.terminal)
    }

    pub fn is_terminal(&self) -> bool {
        self.attachment_is_terminal() || self.response_terminal
    }

    pub fn conversation_entry(&self) -> ConversationEntry {
        let screen_context = UserScreenContext {
            observations: self.observations.clone(),
            pending_frames: self.pending_frames.clone(),
            hearing_context: self.hearing_context.clone(),
            pending_audio: Vec::new(),
            pending_audio_ids: self.pending_audio_ids(),
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_ids: Vec<String>,
    #[serde(default)]
    pub emotion_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emotion_delta: Option<crate::emotion::EmotionDelta>,
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
    #[serde(default)]
    pub conversation_generation: u64,
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

