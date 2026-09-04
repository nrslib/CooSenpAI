use serde::{Deserialize, Serialize};

pub const MEMORY_SCHEMA_VERSION: u8 = 1;
pub const MEMORY_PROMPT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryJobKind {
    Daily,
    Weekly,
}

impl MemoryJobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryJobPhase {
    Reserved,
    Calling,
    Generated,
    Committed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobFailureKind {
    Provider,
    InvalidOutput,
    Persistence,
    ConsentWithdrawn,
    SourceChanged,
    Indeterminate,
    Expired,
    Capacity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryJob {
    pub schema_version: u8,
    pub job_id: String,
    pub kind: MemoryJobKind,
    pub period: String,
    pub day: String,
    pub phase: MemoryJobPhase,
    pub source_digest: String,
    pub source_ids: Vec<String>,
    pub source_truncated: bool,
    pub skipped_invalid_count: usize,
    pub prompt_version: u32,
    pub provider: String,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<JobFailureKind>,
}

impl MemoryJob {
    pub fn recover_after_crash(&mut self, current_day: &str, now: &str) {
        match self.phase {
            MemoryJobPhase::Calling => {
                self.fail(JobFailureKind::Indeterminate, now);
            }
            MemoryJobPhase::Reserved if self.day.as_str() < current_day => {
                self.fail(JobFailureKind::Expired, now);
            }
            MemoryJobPhase::Reserved
            | MemoryJobPhase::Generated
            | MemoryJobPhase::Committed
            | MemoryJobPhase::Failed => {}
        }
    }

    pub fn mark_calling(&mut self, now: &str) {
        self.phase = MemoryJobPhase::Calling;
        self.failure_kind = None;
        self.updated_at = now.to_owned();
    }

    pub fn release_after_preemption(&mut self, now: &str) {
        if self.phase == MemoryJobPhase::Calling {
            self.phase = MemoryJobPhase::Reserved;
            self.failure_kind = None;
            self.updated_at = now.to_owned();
        }
    }

    pub fn mark_generated(&mut self, text: String, now: &str) {
        self.phase = MemoryJobPhase::Generated;
        self.generated_text = Some(text);
        self.failure_kind = None;
        self.updated_at = now.to_owned();
    }

    pub fn mark_committed(&mut self, now: &str) {
        self.phase = MemoryJobPhase::Committed;
        self.failure_kind = None;
        self.updated_at = now.to_owned();
    }

    pub fn fail(&mut self, kind: JobFailureKind, now: &str) {
        self.phase = MemoryJobPhase::Failed;
        self.failure_kind = Some(kind);
        self.updated_at = now.to_owned();
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SummaryState {
    Current,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DailySummary {
    pub schema_version: u8,
    pub local_date: String,
    pub time_zone_id: String,
    pub source_digest: String,
    pub source_ids: Vec<String>,
    pub truncated: bool,
    pub prompt_version: u32,
    pub provider: String,
    pub model: String,
    pub generated_at: String,
    pub text: String,
    pub text_digest: String,
    pub state: SummaryState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeeklyDependency {
    pub local_date: String,
    pub text_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeeklySummary {
    pub schema_version: u8,
    pub period: String,
    pub time_zone_id: String,
    pub source_digest: String,
    pub source_ids: Vec<String>,
    pub truncated: bool,
    pub prompt_version: u32,
    pub provider: String,
    pub model: String,
    pub generated_at: String,
    pub text: String,
    pub text_digest: String,
    pub state: SummaryState,
    pub depends_on: Vec<WeeklyDependency>,
}
