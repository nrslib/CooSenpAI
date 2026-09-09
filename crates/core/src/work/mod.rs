mod approval;
mod chat;
mod execution;
mod harness;
mod roots;

pub use approval::{
    ApprovalDecision, ApprovalMode, ApprovalRequest, ApprovalStatus, WorkApprovals,
};
pub use chat::{ChatWorkExecutor, WorkBrief, WorkProposal, WorkRequest, WorkResult};
pub use execution::{execute, WorkExecution, WORK_TIME_LIMIT};
pub use harness::{build_prompt, harness_arguments, harness_environment, Harness, HarnessLaunch};
pub use roots::{AllowedRoot, RootPolicy, RootStatus, WorkKind};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkConfig {
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    #[serde(default)]
    pub allowed_roots: Vec<AllowedRoot>,
}
