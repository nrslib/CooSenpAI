use crate::bubbles::{
    self, BubbleAction, BubbleInteraction, BubbleOption, BubbleSecretInput, BubbleSelect,
};
use crate::command_guard::CommandContext;
use crate::state::{ConfigCommitError, DesktopState};
use coosenpai_core::config::Config;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::observer::read_observations_by_ids;
use coosenpai_core::runtime::{CompanionDecision, RuntimeError};
use coosenpai_core::state::{ConversationEntry, ConversationMessageKind, ObservationRecord};
use coosenpai_core::utterance_feedback::{
    FeedbackReasonCode, UtteranceFeedbackInput, UtteranceFeedbackResult, UtteranceFeedbackStore,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(crate) const OPEN_ACTION: &str = "utterance-feedback-open";
pub(crate) const REASON_ACTION: &str = "utterance-feedback-reason";
pub(crate) const OTHER_ACTION: &str = "utterance-feedback-other";
pub(crate) const CANCEL_ACTION: &str = "utterance-feedback-cancel";
pub(crate) const TOGGLE_ACTION: &str = "utterance-feedback-toggle";

pub(crate) fn is_feedback_action(action: &str) -> bool {
    matches!(
        action,
        OPEN_ACTION | REASON_ACTION | OTHER_ACTION | CANCEL_ACTION | TOGGLE_ACTION
    )
}

pub(crate) fn is_optional_text_action(action: &str) -> bool {
    action == OTHER_ACTION
}

pub(crate) fn interaction_for_speech(
    config: &Config,
    message_kind: &str,
    recorded: bool,
) -> Option<BubbleInteraction> {
    let kind = ConversationMessageKind::from_wire(message_kind)?;
    if !config.debug.enabled || !kind.is_normal_speech() {
        return None;
    }
    let locale = Locale::from_config(&config.ui.language);
    if recorded {
        Some(BubbleInteraction {
            select: None,
            secret_input: None,
            actions: vec![BubbleAction {
                id: TOGGLE_ACTION.to_owned(),
                label: text(TextKey::UtteranceFeedbackRecorded, locale).to_owned(),
            }],
            detail: None,
            technical_detail: None,
        })
    } else {
        Some(BubbleInteraction {
            select: None,
            secret_input: None,
            actions: vec![BubbleAction {
                id: OPEN_ACTION.to_owned(),
                label: text(TextKey::UtteranceFeedbackOpen, locale).to_owned(),
            }],
            detail: None,
            technical_detail: None,
        })
    }
}

fn reason_interaction(locale: Locale) -> BubbleInteraction {
    BubbleInteraction {
        select: Some(BubbleSelect {
            options: vec![
                reason_option(
                    "no-activity",
                    TextKey::UtteranceFeedbackReasonNoActivity,
                    locale,
                ),
                reason_option("repeated", TextKey::UtteranceFeedbackReasonRepeated, locale),
                reason_option(
                    "misunderstood",
                    TextKey::UtteranceFeedbackReasonMisunderstood,
                    locale,
                ),
                reason_option(
                    "bad-timing",
                    TextKey::UtteranceFeedbackReasonBadTiming,
                    locale,
                ),
                reason_option("other", TextKey::UtteranceFeedbackReasonOther, locale),
            ],
            selected: "no-activity".to_owned(),
            action: REASON_ACTION.to_owned(),
            confirm_label: text(TextKey::UtteranceFeedbackSubmit, locale).to_owned(),
        }),
        secret_input: None,
        actions: vec![cancel_action(locale)],
        detail: None,
        technical_detail: None,
    }
}

fn other_interaction(locale: Locale) -> BubbleInteraction {
    BubbleInteraction {
        select: None,
        secret_input: Some(BubbleSecretInput {
            label: text(TextKey::UtteranceFeedbackFreeTextLabel, locale).to_owned(),
            placeholder: text(TextKey::UtteranceFeedbackFreeTextPlaceholder, locale).to_owned(),
            action: OTHER_ACTION.to_owned(),
            submit_label: text(TextKey::UtteranceFeedbackSubmit, locale).to_owned(),
        }),
        actions: vec![cancel_action(locale)],
        detail: None,
        technical_detail: None,
    }
}

fn cancel_action(locale: Locale) -> BubbleAction {
    BubbleAction {
        id: CANCEL_ACTION.to_owned(),
        label: text(TextKey::UtteranceFeedbackCancel, locale).to_owned(),
    }
}

fn reason_option(value: &str, key: TextKey, locale: Locale) -> BubbleOption {
    BubbleOption {
        value: value.to_owned(),
        label: text(key, locale).to_owned(),
    }
}

impl DesktopState {
    pub(crate) async fn handle_utterance_feedback_interaction(
        self: &Arc<Self>,
        _permit: &CommandContext,
        id: &str,
        action: &str,
        value: Option<&str>,
    ) -> Result<(), ConfigCommitError> {
        let config = self.runtime_config();
        if !config.debug.enabled {
            return Err(feedback_error(
                TextKey::UtteranceFeedbackUnavailable,
                &config,
            ));
        }
        let locale = Locale::from_config(&config.ui.language);
        match action {
            OPEN_ACTION => {
                self.set_feedback_interaction(id, reason_interaction(locale))
                    .await
            }
            REASON_ACTION => {
                let reason = value
                    .and_then(FeedbackReasonCode::parse)
                    .ok_or_else(|| feedback_error(TextKey::InvalidBubbleAction, &config))?;
                if reason == FeedbackReasonCode::Other {
                    self.set_feedback_interaction(id, other_interaction(locale))
                        .await
                } else {
                    self.record_feedback(id, reason, None, &config).await
                }
            }
            OTHER_ACTION => {
                self.record_feedback(id, FeedbackReasonCode::Other, value, &config)
                    .await
            }
            CANCEL_ACTION => {
                self.set_feedback_interaction(
                    id,
                    interaction_for_speech(&config, &self.message_kind(id).await?, false)
                        .ok_or_else(|| {
                            feedback_error(TextKey::UtteranceFeedbackUnavailable, &config)
                        })?,
                )
                .await
            }
            TOGGLE_ACTION => {
                let store = UtteranceFeedbackStore::from_paths(&self.paths);
                let utterance_id = id.to_owned();
                tokio::task::spawn_blocking(move || {
                    store.cancel(&utterance_id, chrono::Utc::now())
                })
                .await
                .map_err(|error| feedback_runtime_error(error.to_string()))?
                .map_err(|error| feedback_runtime_error(error.to_string()))?;
                self.set_feedback_recorded_state(id, false);
                self.set_feedback_interaction(
                    id,
                    interaction_for_speech(&config, &self.message_kind(id).await?, false)
                        .ok_or_else(|| {
                            feedback_error(TextKey::UtteranceFeedbackUnavailable, &config)
                        })?,
                )
                .await
            }
            _ => Err(feedback_error(TextKey::InvalidBubbleAction, &config)),
        }
    }

    async fn record_feedback(
        self: &Arc<Self>,
        id: &str,
        reason_code: FeedbackReasonCode,
        free_text: Option<&str>,
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
        let primary_observation = observations.first();
        let primary_judge_input_ids = primary_observation.map(judge_input_ids).unwrap_or_default();
        let judge_trace = primary_observation.and_then(|observation| {
            self.core_runtime()
                .judge_trace_for_observation(observation.id())
                .or_else(|| {
                    primary_judge_input_ids
                        .iter()
                        .find_map(|input_id| self.core_runtime().judge_trace_for_input(input_id))
                })
        });
        let judge_decision = judge_trace
            .as_ref()
            .and_then(|trace| trace.decision.clone())
            .or_else(|| {
                snapshot
                    .latest_judge_decision
                    .clone()
                    .filter(|decision| primary_judge_input_ids.contains(&decision.input_id))
            });
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
        let result = tokio::task::spawn_blocking(move || store.record(input))
            .await
            .map_err(|error| feedback_runtime_error(error.to_string()))?
            .map_err(|error| feedback_runtime_error(error.to_string()))?;
        if !matches!(
            result,
            UtteranceFeedbackResult::Recorded(_) | UtteranceFeedbackResult::AlreadyRecorded(_)
        ) {
            return Err(feedback_error(
                TextKey::UtteranceFeedbackUnavailable,
                config,
            ));
        }
        self.set_feedback_recorded_state(id, true);
        self.set_feedback_interaction(
            id,
            interaction_for_speech(config, &self.message_kind(id).await?, true)
                .ok_or_else(|| feedback_error(TextKey::UtteranceFeedbackUnavailable, config))?,
        )
        .await
    }

    async fn message_kind(&self, id: &str) -> Result<String, ConfigCommitError> {
        self.snapshot()
            .await
            .conversation
            .into_iter()
            .find(|entry| entry.id == id)
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

    fn set_feedback_recorded_state(&self, id: &str, recorded: bool) {
        if let Ok(mut snapshot) = self.snapshot.lock() {
            if recorded {
                snapshot
                    .recorded_utterance_feedback_ids
                    .insert(id.to_owned());
            } else {
                snapshot.recorded_utterance_feedback_ids.remove(id);
            }
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

fn judge_input_ids(observation: &coosenpai_core::state::ObservationRecord) -> Vec<String> {
    match observation {
        coosenpai_core::state::ObservationRecord::Visual(value) => value
            .source_frame_ids
            .iter()
            .cloned()
            .chain(value.source_frame_paths.keys().cloned())
            .chain(
                value
                    .audio_segments
                    .iter()
                    .map(|segment| segment.id.clone()),
            )
            .collect(),
        coosenpai_core::state::ObservationRecord::Audio(value) => vec![value.id.clone()],
        coosenpai_core::state::ObservationRecord::NoChange(_) => Vec::new(),
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

