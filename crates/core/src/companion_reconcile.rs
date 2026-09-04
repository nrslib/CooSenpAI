use super::*;
use crate::companion_cursor::OWNED_USER_ID_PREFIX;
use crate::companion_storage::{CursorSnapshot, PendingInput, PendingUserMessage};
use std::collections::HashSet;

impl CompanionStorage {
    pub(crate) fn clear_transient_observation_markers(&self) -> Result<(), PersistenceError> {
        self.update_cursor(|cursor| {
            for input in &mut cursor.pending_inputs {
                let PendingInput::UserMessage(message) = input;
                message.observation_in_progress = false;
            }
            Ok(())
        })
    }

    pub(crate) fn reconcile_pending_user_inputs(&self) -> Result<CursorSnapshot, PersistenceError> {
        self.reconcile_pending_user_inputs_with_pruning(true)
    }

    pub(crate) fn reconcile_pending_user_inputs_without_pruning(
        &self,
    ) -> Result<CursorSnapshot, PersistenceError> {
        self.reconcile_pending_user_inputs_with_pruning(false)
    }

    fn reconcile_pending_user_inputs_with_pruning(
        &self,
        prune_conversation: bool,
    ) -> Result<CursorSnapshot, PersistenceError> {
        if prune_conversation {
            crate::persistence::prune_daily_jsonl(
                &self.conversation_directory,
                self.retention_days,
                u64::MAX,
            )?;
        }
        let conversation = self.load_all_conversation()?;
        let unanswered = unanswered_user_entries(&conversation);
        let retained_user_ids = conversation
            .iter()
            .filter(|entry| entry.role == ConversationRole::User)
            .map(|entry| entry.id.as_str())
            .collect::<HashSet<_>>();
        self.update_cursor(|cursor| {
            cursor
                .cancelled_input_ids
                .retain(|id| retained_user_ids.contains(id.as_str()));
            for entry in &unanswered {
                if cursor.cancelled_input_ids.iter().any(|id| id == &entry.id) {
                    continue;
                }
                if cursor
                    .pending_inputs
                    .iter()
                    .any(|input| input.id() == entry.id)
                {
                    continue;
                }
                let user_seq = cursor.next_user_seq.checked_add(1).ok_or_else(|| {
                    PersistenceError::Invalid("user seq が上限に達しました".to_owned())
                })?;
                cursor.next_user_seq = user_seq;
                cursor.user_epoch = cursor.user_epoch.checked_add(1).ok_or_else(|| {
                    PersistenceError::Invalid("user epoch が上限に達しました".to_owned())
                })?;
                cursor
                    .pending_inputs
                    .push(PendingInput::UserMessage(PendingUserMessage {
                        id: entry.id.clone(),
                        user_seq,
                        created_at: entry.created_at.clone(),
                        message: entry.message.clone(),
                        attachment_path: entry.attachment_path.clone(),
                        attachment_text: entry.attachment_text.clone(),
                        observations: entry
                            .screen_context
                            .as_ref()
                            .map_or_else(Vec::new, |context| context.observations.clone()),
                        pending_frames: entry
                            .screen_context
                            .as_ref()
                            .map_or_else(Vec::new, |context| context.pending_frames.clone()),
                        observation_in_progress: false,
                        prepared_response: None,
                        response_commit_started: false,
                        attachment_failure: None,
                        tutorial_response_key: entry.tutorial_response_key.clone(),
                    }));
            }
            Ok(cursor.clone())
        })
    }
}

fn unanswered_user_entries(conversation: &[ConversationEntry]) -> Vec<&ConversationEntry> {
    let user_ids = conversation
        .iter()
        .filter(|entry| {
            entry.role == ConversationRole::User && entry.id.starts_with(OWNED_USER_ID_PREFIX)
        })
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let mut waiting = Vec::<&ConversationEntry>::new();
    let mut answered = HashSet::<&str>::new();
    for entry in conversation {
        match entry.role {
            ConversationRole::User if entry.id.starts_with(OWNED_USER_ID_PREFIX) => {
                waiting.push(entry)
            }
            ConversationRole::User => {}
            ConversationRole::Companion => {
                let causes = entry.observation_ids().collect::<Vec<_>>();
                let explicit = causes
                    .iter()
                    .copied()
                    .filter(|id| user_ids.contains(*id))
                    .collect::<Vec<_>>();
                if causes.is_empty() {
                    if let Some(user) = waiting.pop() {
                        answered.insert(user.id.as_str());
                    }
                } else {
                    for id in explicit {
                        answered.insert(id);
                        waiting.retain(|user| user.id != id);
                    }
                }
            }
        }
    }
    conversation
        .iter()
        .filter(|entry| {
            entry.role == ConversationRole::User
                && entry.id.starts_with(OWNED_USER_ID_PREFIX)
                && !answered.contains(entry.id.as_str())
        })
        .collect()
}

