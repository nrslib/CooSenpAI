use super::*;

impl CompanionAgent {
    pub fn with_context_notice(mut self, notice: String) -> Self {
        self.pending_context_notice = Some(notice);
        self
    }

    pub fn with_memory_context(mut self, context: MemoryContext) -> Self {
        self.memory_context = Some(context);
        self
    }

    pub(super) fn apply_memory_context(
        &self,
        data: &mut CompanionPromptData,
        observations: &[ObservationRecord],
        source_ids: &[String],
    ) -> Result<(), CompanionError> {
        let Some(context) = &self.memory_context else {
            return Ok(());
        };
        let query = data.user_message.clone().unwrap_or_else(|| {
            let mut observations = observations.iter().collect::<Vec<_>>();
            observations.sort_by_key(|observation| observation.id());
            let joined = observations
                .into_iter()
                .filter_map(|observation| match observation {
                    ObservationRecord::Visual(value) => Some(value.data.activity.as_str()),
                    ObservationRecord::NoChange(_) => None,
                    ObservationRecord::Audio(value) => Some(value.text.as_str()),
                })
                .collect::<Vec<_>>()
                .join(" ");
            truncate_utf8(&joined, 2_048).to_owned()
        });
        let recent_ids = self
            .conversation
            .iter()
            .map(|entry| entry.id.clone())
            .chain(source_ids.iter().cloned())
            .collect::<HashSet<_>>();
        let block = context.build(&query, &recent_ids, self.clock.now())?;
        if !block.serialized.is_empty() {
            data.memory_block = Some(block.serialized);
        }
        Ok(())
    }

    pub(super) fn store_fact_proposals(
        &self,
        response: &CompanionResponse,
        data: &CompanionPromptData,
        source_ids: &[String],
    ) -> Result<(), CompanionError> {
        let Some(context) = &self.memory_context else {
            return Ok(());
        };
        if !context.config().enabled || !context.config().provider_consent {
            return Ok(());
        }
        if response.fact_candidates.is_empty() && response.fact_updates.is_empty() {
            return Ok(());
        }
        let allowed = context_user_ids(data.recent_conversation_jsonl.as_deref())
            .into_iter()
            .chain(source_ids.iter().cloned())
            .collect::<HashSet<_>>();
        context
            .facts()
            .validate_candidates(
                &serde_json::Value::Array(response.fact_candidates.clone()),
                &serde_json::Value::Array(response.fact_updates.clone()),
                &allowed,
                &self
                    .clock
                    .now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                context.config(),
            )
            .map_err(MemoryContextError::Facts)?;
        Ok(())
    }

    pub(super) fn apply_session_context(
        &self,
        data: &mut CompanionPromptData,
        user: bool,
        source_ids: &[String],
    ) -> Result<(), CompanionError> {
        data.recent_proactive_utterances = self.confirmed_recent_proactive_utterances();
        if self.needs_session_context {
            data.previous_summary = self.previous_summary.clone();
            data.recent_conversation_jsonl = self.conversation_jsonl_excluding(source_ids)?;
            if user {
                data.last_observation = None;
            }
        } else {
            data.previous_summary = None;
            data.recent_conversation_jsonl = None;
            data.last_observation = None;
        }
        data.context_notice = match (
            data.context_notice.take(),
            self.pending_context_notice.as_deref(),
        ) {
            (Some(runtime), Some(session)) => Some(format!("{runtime}\n{session}")),
            (Some(runtime), None) => Some(runtime),
            (None, Some(session)) => Some(session.to_owned()),
            (None, None) => None,
        };
        Ok(())
    }

    fn confirmed_recent_proactive_utterances(
        &self,
    ) -> Vec<crate::prompts::RecentProactiveUtterance> {
        let pending_unconfirmed_ids = self
            .pending_remarks
            .iter()
            .filter(|pending| !pending.delivery.enqueued)
            .map(|pending| pending.delivery.remark_id.as_str())
            .collect::<HashSet<_>>();
        let utterances = self
            .conversation
            .iter()
            .filter(|entry| {
                entry.is_normal_speech()
                    && entry.message_kind != Some(crate::state::ConversationMessageKind::Chat)
                    && !entry.caused_by_ids.is_empty()
                    && !pending_unconfirmed_ids.contains(entry.id.as_str())
                    && !entry.is_response_failure()
            })
            .filter_map(|entry| {
                let message_kind = entry.message_kind?;
                Some(crate::prompts::RecentProactiveUtterance {
                    id: entry.id.clone(),
                    created_at: entry.created_at.clone(),
                    message_kind: message_kind.as_wire().to_owned(),
                    message: entry.message.clone(),
                    observation_ids: entry.caused_by_ids.clone(),
                })
            })
            .collect::<Vec<_>>();
        crate::prompts::bound_recent_proactive_utterances(&utterances)
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    &value[..end]
}

fn context_user_ids(jsonl: Option<&str>) -> Vec<String> {
    jsonl
        .into_iter()
        .flat_map(str::lines)
        .filter_map(|line| serde_json::from_str::<ConversationEntry>(line).ok())
        .filter(|entry| entry.role == ConversationRole::User)
        .map(|entry| entry.id)
        .collect()
}
