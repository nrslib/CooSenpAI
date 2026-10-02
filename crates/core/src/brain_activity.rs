use crate::judge::{JudgeDecision, JudgeMode, JudgeModuleEvaluation, JudgeTrace};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const MAX_ACTIVITY_NEURONS: usize = 1_024;
pub const MAX_ACTIVITY_FRAMES: usize = 17;
const MAX_ACTIVITY_MODULES: usize = 8;
const MAX_ACTIVITY_BYTES: usize = 224 * 1_024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RateActivity {
    pub schema: String,
    pub input_id: String,
    pub image_sha256: String,
    pub graph_fingerprint: String,
    pub total_neurons: usize,
    pub positioned_neurons: usize,
    pub total_steps: usize,
    pub hmax: f64,
    pub encoding: String,
    pub coordinate_source: String,
    pub neurons: Vec<ActivityNeuron>,
    pub frames: Vec<ActivityFrame>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActivityNeuron {
    pub body_id: String,
    pub position: [f64; 3],
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActivityFrame {
    pub step: usize,
    pub values: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivityObservation {
    pub input_id: String,
    pub occurred_at: String,
    pub mode: Option<JudgeMode>,
    pub decision: Option<JudgeDecision>,
    pub modules: Vec<BrainActivityModule>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivityModule {
    pub module_index: usize,
    pub status: String,
    pub unavailable_reason: Option<String>,
    pub activity: Option<RateActivity>,
    pub evaluation: Option<JudgeModuleEvaluation>,
    pub hold_reason: Option<String>,
    pub error: Option<String>,
    pub preview: Option<BrainActivityPreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivityPreview {
    pub image_sha256: String,
    pub width: u32,
    pub height: u32,
    pub png_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivityModuleSummary {
    pub module_index: usize,
    pub status: String,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivitySummary {
    pub input_id: String,
    pub occurred_at: String,
    pub mode: Option<JudgeMode>,
    pub decision: Option<JudgeDecision>,
    pub modules: Vec<BrainActivityModuleSummary>,
}

impl BrainActivityObservation {
    pub(crate) fn summary(&self) -> BrainActivitySummary {
        BrainActivitySummary {
            input_id: self.input_id.clone(),
            occurred_at: self.occurred_at.clone(),
            mode: self.mode,
            decision: self.decision.clone(),
            modules: self
                .modules
                .iter()
                .map(|module| BrainActivityModuleSummary {
                    module_index: module.module_index,
                    status: module.status.clone(),
                    unavailable_reason: module.unavailable_reason.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivityEntry {
    pub record_id: u64,
    pub observation: BrainActivitySummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivityHistory {
    pub generation: u64,
    pub revision: u64,
    pub latest: Option<BrainActivitySummary>,
    pub entries: Vec<BrainActivityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrainActivityRecord {
    pub generation: u64,
    pub record_id: u64,
    pub observation: BrainActivityObservation,
}

pub(crate) fn bounded_activity_reason(value: &str) -> String {
    const MAX_REASON_BYTES: usize = 2_048;
    if value.len() <= MAX_REASON_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_REASON_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn validate_activity(
    value: Value,
    input_id: &str,
    digest: Option<&str>,
) -> Result<RateActivity, ()> {
    if serde_json::to_vec(&value).map_err(|_| ())?.len() > MAX_ACTIVITY_BYTES
        || value["neurons"].as_array().ok_or(())?.len() > MAX_ACTIVITY_NEURONS
        || value["frames"].as_array().ok_or(())?.len() > MAX_ACTIVITY_FRAMES
    {
        return Err(());
    }
    let activity: RateActivity = serde_json::from_value(value).map_err(|_| ())?;
    if activity.schema != "rate-activity-v1"
        || activity.encoding != "f32-le-hex"
        || activity.coordinate_source != "MaleCNS v1.0 somaLocation"
        || activity.input_id != input_id
        || digest != Some(activity.image_sha256.as_str())
        || !valid_hash(&activity.image_sha256)
        || !valid_hash(&activity.graph_fingerprint)
        || activity.total_neurons == 0
        || activity.total_neurons > 1_000_000
        || activity.positioned_neurons > activity.total_neurons
        || activity.neurons.is_empty()
        || activity.neurons.len() > activity.positioned_neurons
        || activity.total_steps == 0
        || activity.total_steps > 1_000_000
        || !activity.hmax.is_finite()
        || activity.hmax <= 0.0
        || activity.hmax > 1_000_000.0
        || activity.frames.is_empty()
    {
        return Err(());
    }
    let mut bodies = HashSet::new();
    for neuron in &activity.neurons {
        let id = neuron.body_id.parse::<u64>().map_err(|_| ())?;
        if id == 0
            || id.to_string() != neuron.body_id
            || !bodies.insert(id)
            || neuron
                .position
                .iter()
                .any(|value| !value.is_finite() || value.abs() > 100_000_000.0)
        {
            return Err(());
        }
    }
    if activity.frames.first().ok_or(())?.step != 0
        || activity.frames.last().ok_or(())?.step != activity.total_steps
        || activity
            .frames
            .windows(2)
            .any(|pair| pair[0].step >= pair[1].step)
    {
        return Err(());
    }
    for frame in &activity.frames {
        if frame.step > activity.total_steps
            || frame.values.len() != activity.neurons.len() * 8
            || !frame.values.is_ascii()
        {
            return Err(());
        }
        for offset in (0..frame.values.len()).step_by(8) {
            let mut bytes = [0; 4];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(
                    &frame.values[offset + index * 2..offset + index * 2 + 2],
                    16,
                )
                .map_err(|_| ())?;
            }
            let value = f32::from_le_bytes(bytes);
            if !value.is_finite() || value < 0.0 || value > activity.hmax as f32 {
                return Err(());
            }
        }
    }
    Ok(activity)
}

pub(crate) fn extract_activity(trace: &mut JudgeTrace) -> BrainActivityObservation {
    let mut modules = Vec::new();
    for response in &mut trace.responses {
        // Large optional display data must never accumulate in the 1,024-entry judge history.
        let result = response
            .response
            .as_mut()
            .and_then(|value| value.get_mut("result"))
            .and_then(Value::as_object_mut);
        let hold_reason = result
            .as_ref()
            .and_then(|result| result.get("hold_reason"))
            .and_then(Value::as_str)
            .map(bounded_activity_reason);
        let (payload, digest) = match result {
            Some(result) => (
                result.remove("activity"),
                result
                    .get("image_sha256")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ),
            None => (None, None),
        };
        if modules.len() >= MAX_ACTIVITY_MODULES {
            continue;
        }
        let mut module = BrainActivityModule {
            module_index: response.module_index,
            status: "unavailable".into(),
            unavailable_reason: Some("not-provided".into()),
            activity: None,
            evaluation: response.evaluation.clone().map(|mut evaluation| {
                evaluation.readiness = bounded_activity_reason(&evaluation.readiness);
                evaluation
            }),
            hold_reason,
            error: response.error.as_deref().map(bounded_activity_reason),
            preview: None,
        };
        // A commit failure retains a successfully parsed evaluation and its activity.
        if response.error.is_some() && response.evaluation.is_none() {
            module.status = "failed".into();
            module.unavailable_reason = Some("evaluation-failed".into());
        } else if let Some(value) = payload {
            let reason = value.get("unavailable_reason").and_then(Value::as_str);
            if value["schema"] == "rate-activity-v1"
                && reason.is_some_and(|reason| {
                    [
                        "positions-unavailable",
                        "observation-replayed",
                        "image-cache-hit",
                    ]
                    .contains(&reason)
                })
            {
                module.unavailable_reason = reason.map(str::to_owned);
            } else {
                match validate_activity(value, &trace.input_id, digest.as_deref()) {
                    Ok(activity) => {
                        module.status = "available".into();
                        module.unavailable_reason = None;
                        module.activity = Some(activity);
                    }
                    Err(()) => {
                        module.status = "invalid".into();
                        module.unavailable_reason = Some("invalid-payload".into());
                    }
                }
            }
        }
        modules.push(module);
    }
    BrainActivityObservation {
        input_id: trace.input_id.clone(),
        occurred_at: trace
            .decision
            .as_ref()
            .map(|value| value.occurred_at.clone())
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        mode: trace.decision.as_ref().map(|value| value.mode),
        decision: trace.decision.clone().map(|mut decision| {
            decision.readiness = bounded_activity_reason(&decision.readiness);
            decision.hold_reason = decision.hold_reason.as_deref().map(bounded_activity_reason);
            decision.fallback_reason = decision
                .fallback_reason
                .as_deref()
                .map(bounded_activity_reason);
            decision
        }),
        modules,
    }
}
