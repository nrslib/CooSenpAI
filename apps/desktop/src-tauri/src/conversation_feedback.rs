use crate::snapshot::AppSnapshot;
use crate::ui_events::{UiEffect, UiEvent, UiTask};
use crate::utterance_feedback;
use coosenpai_core::judge::JudgeFeedSign;
use coosenpai_core::locale::Locale;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FeedbackAction {
    Good,
    Bad,
    Cancel,
    Comment,
    Edit,
    Save,
    Dismiss,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeedbackRow {
    pub available: bool,
    pub revision: u64,
    pub sign: Option<JudgeFeedSign>,
    pub comment: Option<String>,
    pub editing: bool,
    pub draft: String,
    pub busy: bool,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub(crate) struct FeedbackTask {
    pub token: u64,
    pub conversation_generation: u64,
    pub id: String,
    pub action: FeedbackAction,
    pub value: Option<String>,
    pub revision: u64,
}

#[derive(Default)]
pub(crate) struct ConversationFeedback {
    pub rows: BTreeMap<String, FeedbackRow>,
    generation: Option<u64>,
    next_token: u64,
    operations: BTreeMap<u64, (u64, String, FeedbackAction)>,
}

impl ConversationFeedback {
    pub(crate) fn observe(&mut self, snapshot: &AppSnapshot) {
        if self.generation != Some(snapshot.selected_conversation_generation) {
            self.rows.clear();
            self.generation = Some(snapshot.selected_conversation_generation);
        }
        self.rows
            .retain(|id, _| snapshot.conversation.iter().any(|entry| &entry.id == id));
        let locale = Locale::from_config(&snapshot.config.ui.language);
        for entry in snapshot
            .conversation
            .iter()
            .filter(|entry| entry.role == coosenpai_core::state::ConversationRole::Companion)
        {
            let row = self.rows.entry(entry.id.clone()).or_default();
            row.available = snapshot.config.debug.feedback_enabled
                && !entry.id.trim().is_empty()
                && entry.is_normal_speech()
                && !snapshot.onboarding.setup_required
                && !snapshot.onboarding.tutorial_active;
            let feedback = snapshot.utterance_feedback.get(&entry.id);
            row.revision = feedback.map_or(0, |feedback| feedback.revision);
            row.sign = feedback.and_then(|feedback| feedback.sign);
            row.comment = feedback.and_then(|feedback| feedback.comment.clone());
            row.status =
                feedback.map(|feedback| utterance_feedback::feedback_status_text(feedback, locale));
            if row.sign.is_none() {
                row.editing = false;
            }
            row.busy = self.operations.values().any(|(generation, id, _)| {
                *generation == snapshot.selected_conversation_generation && id == &entry.id
            });
        }
    }

    pub(crate) fn input(
        &mut self,
        id: String,
        action: FeedbackAction,
        value: Option<String>,
        revision: u64,
        generation: u64,
        snapshot: Option<&Arc<AppSnapshot>>,
    ) -> Vec<UiEffect> {
        let Some(snapshot) = snapshot else {
            return vec![];
        };
        if generation != snapshot.selected_conversation_generation {
            return vec![];
        }
        let Some(row) = self
            .rows
            .get_mut(&id)
            .filter(|row| row.available && !row.busy)
        else {
            return vec![];
        };
        match action {
            FeedbackAction::Comment if row.sign.is_some() => {
                row.editing = true;
                row.draft = row.comment.clone().unwrap_or_default();
                row.error = None;
            }
            FeedbackAction::Edit if row.editing => {
                if let Some(value) = value {
                    row.draft = value;
                }
            }
            FeedbackAction::Dismiss => {
                row.editing = false;
                row.error = None;
            }
            FeedbackAction::Good
            | FeedbackAction::Bad
            | FeedbackAction::Cancel
            | FeedbackAction::Save => {
                if revision != row.revision {
                    row.error = Some(
                        if Locale::from_config(&snapshot.config.ui.language) == Locale::Ja {
                            "評価が更新されました。内容を確認してもう一度操作してください"
                        } else {
                            "The rating changed. Review it and try again."
                        }
                        .into(),
                    );
                    return vec![];
                }
                if matches!(action, FeedbackAction::Cancel | FeedbackAction::Save)
                    && row.sign.is_none()
                {
                    return vec![];
                }
                if action == FeedbackAction::Save {
                    let Some(value) = value.as_ref().filter(|_| row.editing) else {
                        return vec![];
                    };
                    row.draft = value.clone();
                }
                self.next_token += 1;
                self.operations
                    .insert(self.next_token, (generation, id.clone(), action));
                row.busy = true;
                row.error = None;
                return vec![UiEffect::Spawn(UiTask::ConversationFeedback(
                    FeedbackTask {
                        token: self.next_token,
                        conversation_generation: generation,
                        id,
                        action,
                        value,
                        revision,
                    },
                ))];
            }
            _ => {}
        }
        vec![]
    }

    pub(crate) fn complete(
        &mut self,
        token: u64,
        generation: u64,
        result: Result<(), String>,
        snapshot: Option<&Arc<AppSnapshot>>,
    ) {
        let Some((current, _, _)) = self.operations.get(&token) else {
            return;
        };
        if *current != generation {
            return;
        }
        let (_, id, action) = self
            .operations
            .remove(&token)
            .expect("matched feedback operation");
        if let Some(snapshot) = snapshot {
            self.observe(snapshot);
        }
        if self.generation != Some(generation) {
            return;
        }
        if let Some(row) = self.rows.get_mut(&id) {
            row.busy = false;
            match result {
                Ok(()) => {
                    row.error = None;
                    if action == FeedbackAction::Save {
                        row.editing = false;
                    }
                }
                Err(error) => row.error = Some(error),
            }
        }
    }
}

pub(crate) async fn run(state: Arc<crate::state::DesktopState>, task: FeedbackTask) -> UiEvent {
    use crate::command_guard::{CommandSource, DesktopCommand, DispatchError};
    let FeedbackTask {
        token,
        conversation_generation,
        id,
        action,
        value,
        revision,
    } = task;
    let handler = state.clone();
    let result = state
        .dispatch(
            CommandSource::IpcMain,
            DesktopCommand::UtteranceFeedback,
            move |context| async move {
                let snapshot = handler.snapshot().await;
                if snapshot.selected_conversation_generation != conversation_generation
                    || snapshot
                        .utterance_feedback
                        .get(&id)
                        .map_or(0, |feedback| feedback.revision)
                        != revision
                {
                    return Err(DispatchError::handler(
                        "評価が更新されました。内容を確認してもう一度操作してください",
                    ));
                }
                let action = match action {
                    FeedbackAction::Good => utterance_feedback::POSITIVE_ACTION,
                    FeedbackAction::Bad => utterance_feedback::NEGATIVE_ACTION,
                    FeedbackAction::Cancel => utterance_feedback::TOGGLE_ACTION,
                    FeedbackAction::Save => utterance_feedback::OTHER_ACTION,
                    _ => return Err(DispatchError::handler("評価操作が不正です")),
                };
                handler
                    .handle_utterance_feedback_interaction(&context, &id, action, value.as_deref())
                    .await
                    .map_err(DispatchError::handler)
            },
        )
        .await
        .map_err(|error| {
            error.format_for_locale(Locale::from_config(&state.runtime_config().ui.language))
        });
    UiEvent::Conversation(
        crate::conversation_presenter::ConversationEvent::FeedbackCompleted {
            token,
            generation: conversation_generation,
            result,
        },
    )
}
