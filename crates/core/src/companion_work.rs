use super::*;
use crate::state::ConversationRole;
use crate::work::{ChatWorkError, ChatWorkExecutor, WorkBrief, WorkProposal, WorkRequest};

const BRIEF_LIMIT: usize = 16 * 1024;
const PRIOR_MESSAGE_LIMIT: usize = 4;

pub(crate) enum ChatWorkOutcome {
    Completed(String),
    Failed(String),
    Stopped(String),
}

impl ChatWorkOutcome {
    pub(crate) fn can_continue(&self) -> bool {
        !matches!(self, Self::Stopped(_))
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Completed(message) | Self::Failed(message) | Self::Stopped(message) => message,
        }
    }
}

impl CompanionAgent {
    pub fn with_work_executor(mut self, executor: Arc<dyn ChatWorkExecutor>) -> Self {
        self.work_executor = Some(executor);
        self
    }

    pub(super) async fn complete_chat_work(
        &mut self,
        outcome: &mut CompanionCallOutcome,
        inputs: &[crate::companion_storage::PendingUserMessage],
        input_ids: &[String],
        events: Option<Arc<dyn crate::provider::ProviderEventSink>>,
        cancellation: CancellationToken,
    ) -> Result<(), CompanionError> {
        let input_id = input_ids.first().ok_or(CompanionError::Output)?;
        let deadline = tokio::time::Instant::now() + crate::work::WORK_TIME_LIMIT;
        let mut failed = Vec::<(WorkProposal, String)>::new();
        let mut step = 0_u64;
        while let Some(proposal) = outcome.response.work_request.take() {
            if cancellation.is_cancelled() {
                return Err(CompanionError::Cancelled);
            }
            let operation_id = format!("{input_id}-work-{step}");
            step = step.checked_add(1).ok_or(CompanionError::Output)?;
            self.append_progress_message(
                format!("{operation_id}-progress"),
                super::support::require_user_message(&outcome.response)?,
                input_ids,
            )?;
            if let Some(events) = &events {
                events.message_committed();
            }
            let answer = if let Some((_, reason)) =
                failed.iter().find(|(prior, _)| prior == &proposal)
            {
                ChatWorkOutcome::Stopped(format!(
                    "同じ操作で失敗したため、繰り返しを停止しました。{reason}"
                ))
            } else if tokio::time::Instant::now() >= deadline {
                ChatWorkOutcome::Stopped("操作の制限時間に達したため停止しました".into())
            } else {
                let operation_cancel = cancellation.child_token();
                let execution = self.execute_chat_work(
                    proposal.clone(),
                    &operation_id,
                    inputs,
                    operation_cancel.clone(),
                );
                tokio::pin!(execution);
                tokio::select! {
                    result = &mut execution => result?,
                    _ = tokio::time::sleep_until(deadline) => {
                        operation_cancel.cancel();
                        let _ = execution.await;
                        ChatWorkOutcome::Stopped("操作の制限時間に達したため停止しました".into())
                    }
                }
            };
            if cancellation.is_cancelled() {
                return Err(CompanionError::Cancelled);
            }
            if !answer.can_continue() {
                outcome.response.message = Some(answer.message().to_owned());
                outcome.response.work_request = None;
                outcome.response.emotion_delta = None;
                break;
            }
            if let ChatWorkOutcome::Failed(reason) = &answer {
                failed.push((proposal, reason.clone()));
            }
            let response_cancel = cancellation.child_token();
            let completion = async {
                self.prepare_call_session(true, response_cancel.clone())
                    .await?;
                let mut completion_data = outcome.data.clone();
                self.apply_session_context(&mut completion_data, true, input_ids)?;
                self.call_provider(
                    ProviderTurn {
                        work_result: Some(answer.message()),
                        data: &completion_data,
                        user: true,
                        image_paths: &[],
                        events: events.clone(),
                        source_ids: input_ids,
                        additional_inputs: None,
                        tutorial_response_key: None,
                    },
                    response_cancel.clone(),
                )
                .await
            };
            tokio::pin!(completion);
            let completion = tokio::select! {
                result = &mut completion => Some(result?),
                _ = tokio::time::sleep_until(deadline) => {
                    response_cancel.cancel();
                    let _ = completion.await;
                    None
                }
            };
            let Some(completion) = completion else {
                outcome.response.message = Some("操作の制限時間に達したため停止しました".into());
                outcome.response.emotion_delta = None;
                break;
            };
            outcome.response = completion.response;
            outcome.call_id = Some(completion.call_id);
        }
        if cancellation.is_cancelled() {
            return Err(CompanionError::Cancelled);
        }
        Ok(())
    }

    fn append_progress_message(
        &mut self,
        id: String,
        message: String,
        input_ids: &[String],
    ) -> Result<(), CompanionError> {
        // 再試行で同じ操作へ戻っても、既に見せた発言を変更しない。
        let history = match &self.storage {
            Some(storage) => storage.load_all_conversation()?,
            None => self.conversation.clone(),
        };
        if let Some(existing) = history.iter().find(|entry| entry.id == id) {
            if existing.message_kind != Some(crate::state::ConversationMessageKind::Progress)
                || existing.caused_by_ids != input_ids
            {
                return Err(CompanionError::Output);
            }
            return Ok(());
        }
        self.append_conversation_once(crate::state::ConversationEntry {
            schema_version: 1,
            id,
            created_at: self
                .clock
                .now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            role: ConversationRole::Companion,
            message,
            message_kind: Some(crate::state::ConversationMessageKind::Progress),
            attachment_path: None,
            attachment_text: None,
            tutorial_response_key: None,
            screen_context: None,
            caused_by_ids: input_ids.to_vec(),
            notification_priority: "none".to_owned(),
        })
    }

    pub(super) async fn execute_chat_work(
        &self,
        proposal: WorkProposal,
        operation_id: &str,
        inputs: &[crate::companion_storage::PendingUserMessage],
        cancellation: CancellationToken,
    ) -> Result<ChatWorkOutcome, CompanionError> {
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
            return Ok(ChatWorkOutcome::Stopped(
                "依頼の文脈が長すぎます。作業内容を短くまとめて、もう一度依頼してください。".into(),
            ));
        }
        let result = executor
            .execute(
                &input.id,
                operation_id,
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
            Ok(result) if result.changed_files.is_empty() => {
                ChatWorkOutcome::Completed(result.answer)
            }
            Ok(result) => ChatWorkOutcome::Completed(format!(
                "{}\n\n変更したファイル：{}",
                result.answer,
                result.changed_files.join("、")
            )),
            Err(ChatWorkError::Failed(error)) => ChatWorkOutcome::Failed(error),
            Err(ChatWorkError::Stopped(error)) => ChatWorkOutcome::Stopped(error),
        })
    }
}

