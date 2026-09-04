use super::{helpers, CompanionError};
use crate::prompts::CompanionPromptData;
use crate::state::{parse_observation, ObservationRecord, DEFAULT_OBSERVATION_LIMITS};

pub(super) fn observation_values(
    observations: &[ObservationRecord],
) -> Result<Vec<serde_json::Value>, CompanionError> {
    observations.iter().map(observation_value).collect()
}

pub(super) fn observation_value(
    observation: &ObservationRecord,
) -> Result<serde_json::Value, CompanionError> {
    let value = serde_json::to_value(observation).map_err(|_| CompanionError::ObservationPrompt)?;
    parse_observation(value.clone(), DEFAULT_OBSERVATION_LIMITS)
        .map_err(|_| CompanionError::ObservationPrompt)?;
    Ok(value)
}

pub(super) fn build_observation_prompt_data(
    companion_name: &str,
    selected: &[&ObservationRecord],
    omitted: &[&ObservationRecord],
    observations: &[ObservationRecord],
    stuck_after_ms: u64,
    previous_summary: Option<String>,
    observation_log_directory: Option<String>,
) -> Result<CompanionPromptData, CompanionError> {
    Ok(CompanionPromptData {
        companion_name: companion_name.to_owned(),
        observations: selected
            .iter()
            .map(|observation| observation_value(observation))
            .collect::<Result<Vec<_>, _>>()?,
        observation_frame_paths: std::collections::HashMap::new(),
        observation_log_directory,
        omitted_observations: Some(
            omitted
                .iter()
                .map(|observation| observation_value(observation))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        compact_observations: true,
        omitted_summary: None,
        omitted_ids: omitted.iter().map(|value| value.id().to_owned()).collect(),
        last_observation: observations.last().map(observation_value).transpose()?,
        elapsed_ms: observations
            .iter()
            .rev()
            .find_map(|observation| match observation {
                ObservationRecord::NoChange(value) => value
                    .stagnation
                    .as_ref()
                    .map(|stagnation| stagnation.elapsed_ms),
                ObservationRecord::Visual(_) | ObservationRecord::Audio(_) => None,
            }),
        stuck_after_ms: Some(stuck_after_ms),
        repeated_error_count: helpers::repeated_error_count(observations),
        previous_summary,
        recent_conversation_jsonl: None,
        user_message: None,
        user_message_id: None,
        user_attachment: false,
        attachment_ocr_text: None,
        pending_frame_context: None,
        memory_block: None,
        context_notice: None,
    })
}
