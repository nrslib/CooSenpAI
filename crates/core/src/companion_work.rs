use super::*;
use crate::state::ConversationRole;
use crate::work::{ChatWorkExecutor, WorkBrief, WorkProposal, WorkRequest};

const BRIEF_LIMIT: usize = 16 * 1024;
const PRIOR_MESSAGE_LIMIT: usize = 4;

impl CompanionAgent {
    pub fn with_work_executor(mut self, executor: Arc<dyn ChatWorkExecutor>) -> Self {
        self.work_executor = Some(executor);
        self
    }

    pub(super) async fn execute_chat_work(
        &self,
        proposal: WorkProposal,
        inputs: &[crate::companion_storage::PendingUserMessage],
        cancellation: CancellationToken,
    ) -> Result<String, CompanionError> {
        let executor = self.work_executor.as_ref().ok_or(CompanionError::Output)?;
        let input = inputs.first().ok_or(CompanionError::Output)?;
        let history = match &self.storage {
            Some(storage) => storage.load_conversation()?,
            None => self.conversation.clone(),
        };
        // 今回の入力と直近の実ユーザー発言だけをホストが渡す。添付本文や観察は含めない。
        let prior_user_messages = history
            .iter()
            .filter(|entry| {
                entry.role == ConversationRole::User
                    && entry.tutorial_response_key.is_none()
                    && !inputs.iter().any(|input| input.id == entry.id)
            })
            .rev()
            .take(PRIOR_MESSAGE_LIMIT)
            .map(|entry| entry.message.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let brief = WorkBrief {
            current_inputs: inputs.iter().map(|input| input.message.clone()).collect(),
            prior_user_messages,
            proposal: proposal.summary,
        };
        let size = brief
            .current_inputs
            .iter()
            .chain(&brief.prior_user_messages)
            .map(String::len)
            .sum::<usize>()
            + brief.proposal.len();
        if size > BRIEF_LIMIT {
            return Ok(
                "依頼の文脈が長すぎます。作業内容を短くまとめて、もう一度依頼してください。".into(),
            );
        }
        let result = executor
            .execute(
                &input.id,
                WorkRequest {
                    kind: proposal.kind,
                    cwd: proposal.cwd.into(),
                    brief,
                },
                cancellation.clone(),
            )
            .await;
        if cancellation.is_cancelled() {
            return Err(CompanionError::Cancelled);
        }
        Ok(match result {
            Ok(result) if result.changed_files.is_empty() => result.answer,
            Ok(result) => format!(
                "{}\n\n変更したファイル：{}",
                result.answer,
                result.changed_files.join("、")
            ),
            Err(error) => format!("作業を完了できませんでした。{error}"),
        })
    }
}

