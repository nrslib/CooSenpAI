use crate::bubbles::{self, BubbleAction, BubbleInteraction, BubbleSecretInput};
use crate::command_guard::CommandContext;
use crate::state::{ConfigCommitError, DesktopState};
use coosenpai_core::config::Config;
use coosenpai_core::judge::{JudgeFeedApplyResult, JudgeFeedSign};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::observer::read_observations_by_ids;
use coosenpai_core::runtime::{CompanionDecision, RuntimeError};
use coosenpai_core::state::{ConversationEntry, ConversationMessageKind, ObservationRecord};
use coosenpai_core::utterance_feedback::{
    FeedbackFeedStatus, FeedbackReasonCode, UtteranceFeedbackInput, UtteranceFeedbackRecord,
    UtteranceFeedbackResult, UtteranceFeedbackStore,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[path = "utterance_feedback_bundle.rs"]
mod bundle;

pub(crate) const POSITIVE_ACTION: &str = "utterance-feedback-positive";
pub(crate) const NEGATIVE_ACTION: &str = "utterance-feedback-negative";
pub(crate) const REASON_ACTION: &str = "utterance-feedback-reason";
pub(crate) const OTHER_ACTION: &str = "utterance-feedback-other";
pub(crate) const CANCEL_ACTION: &str = "utterance-feedback-cancel";
pub(crate) const TOGGLE_ACTION: &str = "utterance-feedback-toggle";

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackSummary {
    pub revision: u64,
    pub sign: Option<JudgeFeedSign>,
    pub comment: Option<String>,
    pub feed_status: Option<FeedbackFeedStatus>,
}

impl From<&UtteranceFeedbackRecord> for FeedbackSummary {
    fn from(record: &UtteranceFeedbackRecord) -> Self {
        Self {
            revision: record.revision,
            sign: (!record.cancelled).then_some(record.sign),
            comment: (!record.cancelled)
                .then(|| record.free_text.clone())
                .flatten(),
            feed_status: record.feed.as_ref().map(|feed| feed.status.clone()),
        }
    }
}

pub(crate) fn load_feedback_summaries(
    paths: &coosenpai_core::config::ConfigPaths,
) -> Result<
    std::collections::BTreeMap<String, FeedbackSummary>,
    coosenpai_core::persistence::PersistenceError,
> {
    let records = coosenpai_core::persistence::JsonlStore::new(paths.utterance_feedback.clone())
        .read::<UtteranceFeedbackRecord>()?;
    Ok(records
        .into_iter()
        .map(|record| (record.utterance.id.clone(), FeedbackSummary::from(&record)))
        .collect())
}

pub(crate) fn is_feedback_action(action: &str) -> bool {
    matches!(
        action,
        POSITIVE_ACTION
            | NEGATIVE_ACTION
            | REASON_ACTION
            | OTHER_ACTION
            | CANCEL_ACTION
            | TOGGLE_ACTION
    )
}

pub(crate) fn is_optional_text_action(action: &str) -> bool {
    action == OTHER_ACTION
}

pub(crate) fn interaction_for_speech(
    config: &Config,
    message_kind: &str,
    feedback: Option<&FeedbackSummary>,
) -> Option<BubbleInteraction> {
    if !config.debug.feedback_enabled
        || !ConversationMessageKind::from_wire(message_kind)?.is_normal_speech()
    {
        return None;
    }
    let locale = Locale::from_config(&config.ui.language);
    let sign = feedback.and_then(|feedback| feedback.sign);
    let mut actions = vec![
        BubbleAction {
            id: POSITIVE_ACTION.into(),
            label: if sign == Some(JudgeFeedSign::Positive) {
                "Good ✓"
            } else {
                "Good"
            }
            .into(),
        },
        BubbleAction {
            id: NEGATIVE_ACTION.into(),
            label: if sign == Some(JudgeFeedSign::Negative) {
                "Bad ✓"
            } else {
                "Bad"
            }
            .into(),
        },
    ];
    if sign.is_some() {
        actions.push(BubbleAction {
            id: REASON_ACTION.into(),
            label: text(TextKey::UtteranceFeedbackAddReason, locale).into(),
        });
        actions.push(BubbleAction {
            id: TOGGLE_ACTION.into(),
            label: if locale == Locale::Ja {
                "評価を取り消す"
            } else {
                "Cancel rating"
            }
            .into(),
        });
    }
    Some(BubbleInteraction {
        select: None,
        secret_input: None,
        actions,
        detail: feedback.map(|feedback| feedback_status_text(feedback, locale)),
        technical_detail: None,
    })
}

pub(crate) fn feedback_status_text(feedback: &FeedbackSummary, locale: Locale) -> String {
    let saved = text(
        if feedback.sign.is_none() {
            TextKey::UtteranceFeedbackCancelled
        } else {
            TextKey::UtteranceFeedbackSaved
        },
        locale,
    );
    let key = match feedback.feed_status {
        Some(FeedbackFeedStatus::Applied) => Some(TextKey::UtteranceFeedbackFeedApplied),
        Some(FeedbackFeedStatus::NotApplied) => Some(TextKey::UtteranceFeedbackFeedNotApplied),
        Some(FeedbackFeedStatus::Unknown) => Some(TextKey::UtteranceFeedbackFeedUnknown),
        _ => None,
    };
    key.map(|key| format!("{}（{}）", saved, text(key, locale)))
        .unwrap_or_else(|| saved.to_owned())
}

fn comment_interaction(locale: Locale, comment: Option<String>) -> BubbleInteraction {
    BubbleInteraction {
        select: None,
        secret_input: Some(BubbleSecretInput {
            label: text(TextKey::UtteranceFeedbackFreeTextLabel, locale).into(),
            placeholder: text(TextKey::UtteranceFeedbackFreeTextPlaceholder, locale).into(),
            action: OTHER_ACTION.into(),
            submit_label: text(TextKey::UtteranceFeedbackSubmit, locale).into(),
            value: comment,
        }),
        actions: vec![BubbleAction {
            id: CANCEL_ACTION.into(),
            label: text(TextKey::UtteranceFeedbackCancel, locale).into(),
        }],
        detail: None,
        technical_detail: None,
    }
}

impl DesktopState {
    pub(crate) async fn export_utterance_feedback(&self) -> Result<String, ConfigCommitError> {
        let relative = tokio::task::spawn_blocking({
            let paths = self.paths.clone();
            move || bundle::export_feedback_bundle(&paths)
        })
        .await
        .map_err(|error| feedback_runtime_error(error.to_string()))?
        .map_err(feedback_runtime_error)?;
        Ok(self.paths.root.join(relative).display().to_string())
    }

    pub(crate) async fn handle_utterance_feedback_interaction(
        self: &Arc<Self>,
        _permit: &CommandContext,
        id: &str,
        action: &str,
        value: Option<&str>,
    ) -> Result<(), ConfigCommitError> {
        let config = self.runtime_config();
        if !config.debug.feedback_enabled {
            return Err(feedback_error(
                TextKey::UtteranceFeedbackUnavailable,
                &config,
            ));
        }
        let locale = Locale::from_config(&config.ui.language);
        // チャット履歴は吹き出しの保持期間に依存しない。
        let kind = self.message_kind(id).await?;
        let previous = self.latest_feedback(id).await?;
        let active = previous.as_ref().filter(|record| !record.cancelled);
        match action {
            POSITIVE_ACTION | NEGATIVE_ACTION => {
                let sign = if action == POSITIVE_ACTION {
                    JudgeFeedSign::Positive
                } else {
                    JudgeFeedSign::Negative
                };
                if active.is_some_and(|record| record.sign == sign) {
                    return self.cancel_feedback(id, &config).await;
                }
                self.record_feedback(
                    id,
                    sign,
                    active
                        .map(|record| record.reason_code)
                        .unwrap_or(FeedbackReasonCode::None),
                    active.and_then(|record| record.free_text.as_deref()),
                    active.is_some(),
                    &config,
                )
                .await
            }
            REASON_ACTION => {
                let record = active.ok_or_else(|| {
                    feedback_error(TextKey::UtteranceFeedbackUnavailable, &config)
                })?;
                self.set_feedback_interaction(
                    id,
                    comment_interaction(locale, record.free_text.clone()),
                )
                .await
            }
            OTHER_ACTION => {
                let record = active.ok_or_else(|| {
                    feedback_error(TextKey::UtteranceFeedbackUnavailable, &config)
                })?;
                let comment = value.filter(|value| !value.trim().is_empty());
                self.record_feedback(
                    id,
                    record.sign,
                    if comment.is_some() {
                        FeedbackReasonCode::Other
                    } else {
                        FeedbackReasonCode::None
                    },
                    comment,
                    true,
                    &config,
                )
                .await?;
                let snapshot = self.snapshot().await;
                bubbles::mutate_checked(
                    &self.ui,
                    bubbles::BubbleMutation::SetInteraction {
                        id: id.to_owned(),
                        interaction: interaction_for_speech(
                            &config,
                            &kind,
                            snapshot.utterance_feedback.get(id),
                        )
                        .map(Box::new),
                    },
                )
                .await
                .map_err(feedback_runtime_error)?;
                Ok(())
            }
            CANCEL_ACTION => {
                self.set_feedback_interaction(
                    id,
                    interaction_for_speech(
                        &config,
                        &kind,
                        previous.as_ref().map(FeedbackSummary::from).as_ref(),
                    )
                    .ok_or_else(|| {
                        feedback_error(TextKey::UtteranceFeedbackUnavailable, &config)
                    })?,
                )
                .await
            }
            TOGGLE_ACTION => self.cancel_feedback(id, &config).await,
            _ => Err(feedback_error(TextKey::InvalidBubbleAction, &config)),
        }
    }

    async fn cancel_feedback(&self, id: &str, config: &Config) -> Result<(), ConfigCommitError> {
        let store = UtteranceFeedbackStore::from_paths(&self.paths);
        let utterance_id = id.to_owned();
        let result =
            tokio::task::spawn_blocking(move || store.cancel(&utterance_id, chrono::Utc::now()))
                .await
                .map_err(|error| feedback_runtime_error(error.to_string()))?
                .map_err(|error| feedback_runtime_error(error.to_string()))?;
        let UtteranceFeedbackResult::Cancelled(record) = result else {
            return Err(feedback_error(
                TextKey::UtteranceFeedbackUnavailable,
                config,
            ));
        };
        self.publish_feedback(&record).await;
        let (status, reason) = self.cancel_feed(&record).await;
        let record = self
            .persist_feed_status(&record, status, reason)
            .await
            .map_err(|_| feedback_feed_state_error(config))?;
        self.publish_feedback(&record).await;
        Ok(())
    }

    async fn record_feedback(
        self: &Arc<Self>,
        id: &str,
        sign: JudgeFeedSign,
        reason_code: FeedbackReasonCode,
        free_text: Option<&str>,
        revise: bool,
        config: &Config,
    ) -> Result<(), ConfigCommitError> {
        let snapshot = self.snapshot().await;
        let utterance = snapshot
            .conversation
            .iter()
            .find(|entry| entry.id == id && entry.is_normal_speech())
            .cloned()
            .ok_or_else(|| feedback_error(TextKey::UtteranceFeedbackUnavailable, config))?;
        let observation_ids = observation_ids_for_utterance(&utterance, &snapshot.conversation);
        let embedded_observations = observations_for_utterance(&utterance, &snapshot.conversation);
        let observation_directory = self.paths.observations.clone();
        let requested_observation_ids = observation_ids.clone();
        let persisted_observations = tokio::task::spawn_blocking(move || {
            read_observations_by_ids(&observation_directory, &requested_observation_ids)
        })
        .await
        .map_err(|error| feedback_runtime_error(error.to_string()))?
        .map_err(|error| feedback_runtime_error(error.to_string()))?;
        let observations = merge_observations(
            &observation_ids,
            &embedded_observations,
            persisted_observations,
        );
        let judge_trace = observations.iter().find_map(|observation| {
            let input_id = primary_judge_input_id(observation)?;
            self.core_runtime()
                .judge_trace_for_input(&input_id)
                .filter(|trace| trace.input_id == input_id)
        });
        let judge_feedable_input_id = judge_trace
            .as_ref()
            .filter(|trace| trace.feedable)
            .map(|trace| trace.input_id.clone());
        let judge_decision = judge_trace
            .as_ref()
            .and_then(|trace| trace.decision.clone());
        let llm = snapshot
            .latest_companion_decision
            .as_ref()
            .filter(|decision| companion_decision_matches(decision, &utterance, &observation_ids))
            .map(|decision| {
                (
                    coosenpai_core::utterance_feedback::FeedbackLlmDecision {
                        emit: decision.emit,
                        message_kind: decision.message_kind.clone(),
                    },
                    decision.call_id.clone(),
                )
            });
        let input = UtteranceFeedbackInput {
            trigger: feedback_trigger(&utterance, &snapshot.conversation, &observations),
            utterance,
            observation_ids,
            observations,
            judge_trace,
            judge_decision,
            llm_decision: llm.as_ref().map(|(decision, _)| decision.clone()),
            llm_call_id: llm.and_then(|(_, call_id)| call_id),
            reason_code,
            free_text: free_text.map(str::to_owned),
        };
        let store = UtteranceFeedbackStore::from_paths(&self.paths);
        let result = tokio::task::spawn_blocking(move || {
            if revise {
                store.revise_with_sign(input, sign)
            } else {
                store.record_with_sign(input, sign)
            }
        })
        .await
        .map_err(|error| feedback_runtime_error(error.to_string()))?
        .map_err(|error| feedback_runtime_error(error.to_string()))?;
        let (record, should_apply) = match result {
            UtteranceFeedbackResult::Recorded(record) => (record, true),
            UtteranceFeedbackResult::AlreadyRecorded(record) => {
                let should_retry = record.feed.as_ref().is_some_and(|feed| {
                    !matches!(feed.status, FeedbackFeedStatus::Applied) && feed.input_id.is_some()
                });
                (record, should_retry)
            }
            UtteranceFeedbackResult::Cancelled(_) => {
                return Err(feedback_error(
                    TextKey::UtteranceFeedbackUnavailable,
                    config,
                ));
            }
        };
        self.publish_feedback(&record).await;
        let (status, reason) = if should_apply {
            self.apply_feed(&record, revise, judge_feedable_input_id.as_deref())
                .await
        } else {
            record
                .feed
                .as_ref()
                .map(|feed| (feed.status.clone(), feed.reason.clone()))
                .unwrap_or((
                    FeedbackFeedStatus::NotApplied,
                    Some("feed-event-missing".to_owned()),
                ))
        };
        let status_record = if should_apply {
            self.persist_feed_status(&record, status, reason)
                .await
                .map_err(|_| feedback_feed_state_error(config))?
        } else {
            record
        };
        self.publish_feedback(&status_record).await;
        Ok(())
    }

    async fn latest_feedback(
        &self,
        id: &str,
    ) -> Result<Option<UtteranceFeedbackRecord>, ConfigCommitError> {
        let store = UtteranceFeedbackStore::from_paths(&self.paths);
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || store.latest_for_utterance(&id))
            .await
            .map_err(|error| feedback_runtime_error(error.to_string()))?
            .map_err(|error| feedback_runtime_error(error.to_string()))
    }

    async fn publish_feedback(&self, record: &UtteranceFeedbackRecord) {
        self.publish_event(
            crate::snapshot_presenter::SnapshotEvent::UtteranceFeedbackChanged {
                id: record.utterance.id.clone(),
                feedback: FeedbackSummary::from(record),
            },
        )
        .await;
    }

    async fn apply_feed(
        &self,
        record: &UtteranceFeedbackRecord,
        revise: bool,
        judge_feedable_input_id: Option<&str>,
    ) -> (FeedbackFeedStatus, Option<String>) {
        let Some(feed) = record.feed.as_ref() else {
            return (
                FeedbackFeedStatus::NotApplied,
                Some("feed-event-missing".to_owned()),
            );
        };
        let Some(input_id) = feed.input_id.clone() else {
            return (
                FeedbackFeedStatus::NotApplied,
                Some("judge-input-unavailable".to_owned()),
            );
        };
        if !self.core_runtime().judge_feed_available() {
            return (
                FeedbackFeedStatus::NotApplied,
                Some("judge-unavailable".to_owned()),
            );
        }
        if judge_feedable_input_id != Some(input_id.as_str()) {
            return (
                FeedbackFeedStatus::NotApplied,
                Some("judge-not-feedable".to_owned()),
            );
        }
        let result = if revise {
            self.core_runtime()
                .correct_judge_feed(feed.event_id.clone(), input_id, feed.sign, feed.strength)
                .await
        } else {
            self.core_runtime()
                .feed_judge_with_event_id(feed.event_id.clone(), input_id, feed.sign, feed.strength)
                .await
        };
        match result {
            Ok(JudgeFeedApplyResult::Applied) => (FeedbackFeedStatus::Applied, None),
            Ok(JudgeFeedApplyResult::NotApplied) => (
                FeedbackFeedStatus::NotApplied,
                Some("judge-feed-not-applied".to_owned()),
            ),
            Ok(JudgeFeedApplyResult::Unknown) => (
                FeedbackFeedStatus::Unknown,
                Some("judge-feed-unknown".to_owned()),
            ),
            Err(_) => (
                FeedbackFeedStatus::Unknown,
                Some("judge-feed-failed".to_owned()),
            ),
        }
    }

    async fn cancel_feed(
        &self,
        record: &UtteranceFeedbackRecord,
    ) -> (FeedbackFeedStatus, Option<String>) {
        let Some(feed) = record.feed.as_ref() else {
            return (
                FeedbackFeedStatus::NotApplied,
                Some("feed-event-missing".to_owned()),
            );
        };
        let Some(input_id) = feed.input_id.clone() else {
            return (
                FeedbackFeedStatus::NotApplied,
                Some("judge-input-unavailable".to_owned()),
            );
        };
        if !self.core_runtime().judge_feed_available() {
            return (
                FeedbackFeedStatus::NotApplied,
                Some("judge-unavailable".to_owned()),
            );
        }
        match self
            .core_runtime()
            .cancel_judge_feed(feed.event_id.clone(), input_id, feed.sign, feed.strength)
            .await
        {
            Ok(JudgeFeedApplyResult::Applied) => (FeedbackFeedStatus::Applied, None),
            Ok(JudgeFeedApplyResult::NotApplied) => (
                FeedbackFeedStatus::NotApplied,
                Some("judge-feed-cancel-not-applied".to_owned()),
            ),
            Ok(JudgeFeedApplyResult::Unknown) => (
                FeedbackFeedStatus::Unknown,
                Some("judge-feed-cancel-unknown".to_owned()),
            ),
            Err(_) => (
                FeedbackFeedStatus::Unknown,
                Some("judge-feed-cancel-failed".to_owned()),
            ),
        }
    }

    async fn persist_feed_status(
        &self,
        record: &UtteranceFeedbackRecord,
        status: FeedbackFeedStatus,
        reason: Option<String>,
    ) -> Result<UtteranceFeedbackRecord, ConfigCommitError> {
        let store = UtteranceFeedbackStore::from_paths(&self.paths);
        let record_id = record.record_id.clone();
        tokio::task::spawn_blocking(move || store.update_feed(&record_id, None, status, reason))
            .await
            .map_err(|error| feedback_runtime_error(error.to_string()))?
            .map_err(|error| feedback_runtime_error(error.to_string()))
    }

    async fn message_kind(&self, id: &str) -> Result<String, ConfigCommitError> {
        self.snapshot()
            .await
            .conversation
            .into_iter()
            .find(|entry| entry.id == id && !id.trim().is_empty() && entry.is_normal_speech())
            .and_then(|entry| entry.message_kind)
            .map(|kind| kind.as_wire().to_owned())
            .ok_or_else(|| {
                feedback_error(
                    TextKey::UtteranceFeedbackUnavailable,
                    &self.runtime_config(),
                )
            })
    }

    async fn set_feedback_interaction(
        &self,
        id: &str,
        interaction: BubbleInteraction,
    ) -> Result<(), ConfigCommitError> {
        let changed = bubbles::mutate_checked(
            &self.ui,
            bubbles::BubbleMutation::SetInteraction {
                id: id.to_owned(),
                interaction: Some(Box::new(interaction)),
            },
        )
        .await
        .map_err(feedback_runtime_error)?;
        if changed {
            Ok(())
        } else {
            Err(feedback_error(
                TextKey::UtteranceFeedbackUnavailable,
                &self.runtime_config(),
            ))
        }
    }
}

fn feedback_error(key: TextKey, config: &Config) -> ConfigCommitError {
    ConfigCommitError::Runtime(RuntimeError::Factory(
        text(key, Locale::from_config(&config.ui.language)).to_owned(),
    ))
}

fn feedback_runtime_error(message: String) -> ConfigCommitError {
    ConfigCommitError::Runtime(RuntimeError::Factory(message))
}

fn feedback_feed_state_error(config: &Config) -> ConfigCommitError {
    feedback_error(TextKey::UtteranceFeedbackFeedStatusSaveFailed, config)
}

fn companion_decision_matches(
    decision: &CompanionDecision,
    utterance: &ConversationEntry,
    observation_ids: &[String],
) -> bool {
    if !decision.utterance_observation_ids.is_empty() {
        return decision.utterance_observation_ids.iter().any(|id| {
            utterance
                .caused_by_ids
                .iter()
                .any(|source_id| source_id == id)
        });
    }
    if decision
        .call_id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty())
    {
        return decision.observation_ids.iter().any(|id| {
            utterance
                .caused_by_ids
                .iter()
                .any(|source_id| source_id == id)
        });
    }
    decision.observation_ids.iter().any(|id| {
        utterance
            .caused_by_ids
            .iter()
            .any(|source_id| source_id == id)
            || observation_ids.iter().any(|source_id| source_id == id)
    })
}

fn primary_judge_input_id(
    observation: &coosenpai_core::state::ObservationRecord,
) -> Option<String> {
    match observation {
        coosenpai_core::state::ObservationRecord::Visual(value) => {
            value.source_frame_ids.first().cloned().or_else(|| {
                value
                    .audio_segments
                    .first()
                    .map(|segment| segment.id.clone())
            })
        }
        coosenpai_core::state::ObservationRecord::Audio(value) => Some(value.id.clone()),
        coosenpai_core::state::ObservationRecord::NoChange(_) => None,
    }
}

fn observation_ids_for_utterance(
    utterance: &ConversationEntry,
    conversation: &[ConversationEntry],
) -> Vec<String> {
    let mut queue = utterance.caused_by_ids.clone();
    let mut index = 0;
    let mut seen = HashSet::new();
    let mut observation_seen = HashSet::new();
    let mut observations = Vec::new();
    if let Some(context) = &utterance.screen_context {
        for observation in &context.observations {
            if observation_seen.insert(observation.id().to_owned()) {
                observations.push(observation.id().to_owned());
            }
        }
        for audio_id in &context.pending_audio_ids {
            if observation_seen.insert(audio_id.clone()) {
                observations.push(audio_id.clone());
            }
        }
        for audio in &context.pending_audio {
            if observation_seen.insert(audio.id.clone()) {
                observations.push(audio.id.clone());
            }
        }
    }
    while index < queue.len() {
        let id = queue[index].clone();
        index += 1;
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(entry) = conversation.iter().find(|entry| entry.id == id) {
            queue.extend(entry.caused_by_ids.iter().cloned());
            if let Some(context) = &entry.screen_context {
                for observation in &context.observations {
                    if observation_seen.insert(observation.id().to_owned()) {
                        observations.push(observation.id().to_owned());
                    }
                }
                for audio_id in &context.pending_audio_ids {
                    if observation_seen.insert(audio_id.clone()) {
                        observations.push(audio_id.clone());
                    }
                }
                for audio in &context.pending_audio {
                    if observation_seen.insert(audio.id.clone()) {
                        observations.push(audio.id.clone());
                    }
                }
            }
        } else {
            if observation_seen.insert(id.clone()) {
                observations.push(id);
            }
        }
    }
    observations
}

fn feedback_trigger(
    utterance: &ConversationEntry,
    conversation: &[ConversationEntry],
    observations: &[ObservationRecord],
) -> coosenpai_core::utterance_feedback::FeedbackTrigger {
    use coosenpai_core::state::ConversationRole;
    use coosenpai_core::utterance_feedback::FeedbackTrigger;
    if utterance.caused_by_ids.iter().any(|id| {
        conversation
            .iter()
            .any(|entry| &entry.id == id && entry.role == ConversationRole::User)
    }) {
        FeedbackTrigger::UserReply
    } else if !utterance.caused_by_ids.is_empty()
        && utterance.caused_by_ids.iter().all(|id| {
            observations
                .iter()
                .any(|observation| observation.id() == id)
        })
    {
        FeedbackTrigger::Proactive
    } else {
        FeedbackTrigger::Unknown
    }
}

fn observations_for_utterance(
    utterance: &ConversationEntry,
    conversation: &[ConversationEntry],
) -> Vec<coosenpai_core::state::ObservationRecord> {
    let mut queue = utterance.caused_by_ids.clone();
    let mut index = 0;
    let mut seen = HashSet::new();
    let mut observations = Vec::new();
    if let Some(context) = &utterance.screen_context {
        for observation in &context.observations {
            if seen.insert(observation.id().to_owned()) {
                observations.push(observation.clone());
            }
        }
        for audio in &context.pending_audio {
            if seen.insert(audio.id.clone()) {
                observations.push(coosenpai_core::state::ObservationRecord::Audio(
                    audio.clone(),
                ));
            }
        }
    }
    while index < queue.len() {
        let id = queue[index].clone();
        index += 1;
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(entry) = conversation.iter().find(|entry| entry.id == id) else {
            continue;
        };
        queue.extend(entry.caused_by_ids.iter().cloned());
        if let Some(context) = &entry.screen_context {
            for observation in &context.observations {
                if seen.insert(observation.id().to_owned()) {
                    observations.push(observation.clone());
                }
            }
            for audio in &context.pending_audio {
                if seen.insert(audio.id.clone()) {
                    observations.push(coosenpai_core::state::ObservationRecord::Audio(
                        audio.clone(),
                    ));
                }
            }
        }
    }
    observations
}

fn merge_observations(
    observation_ids: &[String],
    embedded: &[ObservationRecord],
    persisted: Vec<ObservationRecord>,
) -> Vec<ObservationRecord> {
    let mut by_id = embedded
        .iter()
        .cloned()
        .map(|observation| (observation.id().to_owned(), observation))
        .collect::<HashMap<_, _>>();
    for observation in persisted {
        by_id
            .entry(observation.id().to_owned())
            .or_insert(observation);
    }
    observation_ids
        .iter()
        .filter_map(|id| by_id.remove(id))
        .collect()
}

