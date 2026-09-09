use super::{RootStatus, WorkKind};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Cooが会話から決めた提案。cwdは絶対パスまたは `~` 始まり。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkProposal {
    pub kind: WorkKind,
    pub cwd: String,
    pub summary: String,
}

/// ハーネスへ渡す依頼。ホストが会話から組み立て、AIの言い換えは含めない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkBrief {
    pub current_inputs: Vec<String>,
    pub prior_user_messages: Vec<String>,
    pub proposal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkRequest {
    pub kind: WorkKind,
    pub cwd: PathBuf,
    pub brief: WorkBrief,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkResult {
    pub answer: String,
    pub cwd: PathBuf,
    pub root: RootStatus,
    pub changed_files: Vec<String>,
    pub stderr_summary: String,
}

#[async_trait]
pub trait ChatWorkExecutor: Send + Sync {
    async fn execute(
        &self,
        input_id: &str,
        request: WorkRequest,
        cancellation: CancellationToken,
    ) -> Result<WorkResult, String>;
}
