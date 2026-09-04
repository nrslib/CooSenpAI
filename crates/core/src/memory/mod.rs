mod canonical;
mod context;
mod facts;
mod jobs;
mod prompts;
mod retrieval;
mod schedule;
mod service;
mod store;
mod types;

pub use canonical::{
    canonical_daily_source, memory_job_id, CanonicalError, SourceInput, SourceInputKind,
};
pub use context::{MemoryContext, MemoryContextError};
pub use facts::{
    FactCandidate, FactCandidatesSnapshot, FactRecord, FactStore, FactUpdate, FactUpdateOperation,
    MemoryFactError,
};
pub use jobs::{
    DailySummary, JobFailureKind, MemoryJob, MemoryJobKind, MemoryJobPhase, SummaryState,
    WeeklyDependency, WeeklySummary, MEMORY_PROMPT_VERSION, MEMORY_SCHEMA_VERSION,
};
pub use prompts::{daily_summary_prompt, memory_summary_schema, weekly_summary_prompt};
pub use retrieval::{
    build_memory_block, MemoryBlock, MemoryRetrievalError, MemoryRetrievalInput, MemorySearchRecord,
};
pub use schedule::{select_schedule, MemorySchedule, ScheduleInput};
pub use service::{MemoryErrorKind, MemoryService, MemoryServiceError, MemoryStatus};
pub use store::{memory_job_kind_for_period, MemoryStore, MemoryStoreError};
pub use types::{CanonicalRecord, SourceSnapshot};

