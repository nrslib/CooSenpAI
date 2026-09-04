use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalRecord {
    Conversation {
        id: String,
        created_at_ms: i64,
        role: String,
        message: String,
    },
    Observation {
        id: String,
        created_at_ms: i64,
        activity: String,
        outline: String,
        changes: Vec<String>,
        guess: Option<String>,
    },
}

impl CanonicalRecord {
    pub fn id(&self) -> &str {
        match self {
            Self::Conversation { id, .. } | Self::Observation { id, .. } => id,
        }
    }

    pub(crate) fn created_at_ms(&self) -> i64 {
        match self {
            Self::Conversation { created_at_ms, .. } | Self::Observation { created_at_ms, .. } => {
                *created_at_ms
            }
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Conversation { .. } => "conversation",
            Self::Observation { .. } => "observation",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSnapshot {
    #[serde(skip)]
    pub canonical_bytes: Vec<u8>,
    pub source_digest: String,
    pub source_ids: Vec<String>,
    pub truncated: bool,
    pub skipped_invalid_count: usize,
}
