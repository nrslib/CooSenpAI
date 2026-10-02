use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use habitua::{
    Evaluation, Evidence, LearningSignal, LoadBudget, Metric, MetricDirection, Model, ModelError,
    ModelIdentity, ModelInput, ModelReport, PersistentModel, Reaction, ReactionKind, Readiness,
};
use serde::{Deserialize, Serialize};

use crate::rate::{
    RateBackward, RateForwardResult, RateGradients, RateGraph, RateModelError, RateParameters,
    final_activity_with_parameters, forward_with_parameters,
};

const CHECKPOINT_VERSION: u32 = 1;
const REST_STEPS: usize = 64;

/// Selects which parameter groups are updated by one feedback event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TrainingMode {
    /// Do not update the readout or the recurrent circuit.
    Frozen,
    /// Update only the readout parameters.
    ReadoutOnly,
    /// Update the readout and recurrent circuit parameters.
    Full,
}

/// The public feedback operation used by the food API.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FeedbackKind {
    /// Increase the value target for an input.
    Reward,
    /// Decrease the value target for an input.
    Punish,
}

/// One versioned feedback event tied to one input identifier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeedbackEvent {
    /// Unique event identifier. Re-delivery uses the same ID.
    pub event_id: String,
    /// Identifier of the observation being labelled.
    pub input_id: String,
    /// Positive or negative teacher operation.
    pub kind: FeedbackKind,
    /// BCE weight in the closed interval `[0, 1]`.
    pub strength: f64,
    /// Event time in the caller's monotonic or logical time unit.
    pub timestamp_s: f64,
    /// Time at which the event reached the model. `None` keeps compatibility
    /// with state written before delayed feedback was tracked.
    #[serde(default)]
    pub received_at_s: Option<f64>,
    /// Monotonic revision for corrections.
    pub revision: u64,
    /// Marks a newer revision as a cancellation of the event.
    #[serde(default)]
    pub cancelled: bool,
}

impl FeedbackEvent {
    /// Creates a validated feedback event.
    pub fn new(
        event_id: impl Into<String>,
        input_id: impl Into<String>,
        kind: FeedbackKind,
        strength: f64,
        timestamp_s: f64,
    ) -> Result<Self, RateModelError> {
        let event_id = event_id.into();
        let input_id = input_id.into();
        if event_id.is_empty() || input_id.is_empty() {
            return Err(RateModelError::Invalid(
                "feedback event_id and input_id must not be empty".to_owned(),
            ));
        }
        if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
            return Err(RateModelError::Invalid(
                "feedback strength must be finite and in [0, 1]".to_owned(),
            ));
        }
        if !timestamp_s.is_finite() {
            return Err(RateModelError::Invalid(
                "feedback timestamp must be finite".to_owned(),
            ));
        }
        Ok(Self {
            event_id,
            input_id,
            kind,
            strength,
            timestamp_s,
            received_at_s: Some(timestamp_s),
            revision: 0,
            cancelled: false,
        })
    }

    /// Sets the arrival time separately from the event time.
    pub fn with_received_at_s(mut self, received_at_s: f64) -> Result<Self, RateModelError> {
        if !received_at_s.is_finite() {
            return Err(RateModelError::Invalid(
                "feedback received timestamp must be finite".to_owned(),
            ));
        }
        self.received_at_s = Some(received_at_s);
        Ok(self)
    }

    /// Creates a cancellation revision for an existing event ID.
    pub fn cancellation(
        event_id: impl Into<String>,
        input_id: impl Into<String>,
        timestamp_s: f64,
        revision: u64,
    ) -> Result<Self, RateModelError> {
        let mut event = Self::new(event_id, input_id, FeedbackKind::Punish, 0.0, timestamp_s)?;
        event.revision = revision;
        event.cancelled = true;
        Ok(event)
    }

    /// Returns this event with an explicit correction revision.
    pub const fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }

    /// Converts reward/punish into the BCE target and returns its strength.
    pub fn target(&self) -> (f64, f64) {
        (
            match self.kind {
                FeedbackKind::Reward => 1.0,
                FeedbackKind::Punish => 0.0,
            },
            self.strength,
        )
    }
}

/// Deduplicated feedback storage with explicit correction revisions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FeedbackStore {
    events: BTreeMap<String, FeedbackEvent>,
}

impl FeedbackStore {
    /// Inserts an event or a strictly newer correction of an existing event.
    pub fn insert(&mut self, event: FeedbackEvent) -> Result<bool, RateModelError> {
        if let Some(previous) = self.events.get(&event.event_id)
            && event.revision <= previous.revision
        {
            return Ok(false);
        }
        self.events.insert(event.event_id.clone(), event);
        Ok(true)
    }

    /// Adds a reward event.
    pub fn reward(
        &mut self,
        event_id: impl Into<String>,
        input_id: impl Into<String>,
        strength: f64,
        timestamp_s: f64,
    ) -> Result<bool, RateModelError> {
        self.insert(FeedbackEvent::new(
            event_id,
            input_id,
            FeedbackKind::Reward,
            strength,
            timestamp_s,
        )?)
    }

    /// Adds a punish event.
    pub fn punish(
        &mut self,
        event_id: impl Into<String>,
        input_id: impl Into<String>,
        strength: f64,
        timestamp_s: f64,
    ) -> Result<bool, RateModelError> {
        self.insert(FeedbackEvent::new(
            event_id,
            input_id,
            FeedbackKind::Punish,
            strength,
            timestamp_s,
        )?)
    }

    /// Returns the current event for an ID.
    pub fn get(&self, event_id: &str) -> Option<&FeedbackEvent> {
        self.events.get(event_id)
    }

    /// Returns all events in deterministic event-ID order.
    pub fn events(&self) -> impl Iterator<Item = &FeedbackEvent> {
        self.events.values()
    }

    /// Returns events that have not been converted into labels for another input.
    pub fn for_input(&self, input_id: &str) -> Vec<&FeedbackEvent> {
        self.events
            .values()
            .filter(|event| event.input_id == input_id)
            .collect()
    }

    /// Returns a reproducible reward-shuffled control and its match rate.
    pub fn shuffled(&self, seed: u64) -> FeedbackShuffleResult {
        let mut events = self.events.values().cloned().collect::<Vec<_>>();
        let original_ids = events
            .iter()
            .map(|event| event.input_id.clone())
            .collect::<Vec<_>>();
        let mut state = seed.max(1);
        for index in (1..events.len()).rev() {
            let swap = (next_random(&mut state) as usize) % (index + 1);
            events.swap(index, swap);
        }
        let mut input_ids = original_ids.clone();
        for index in (1..input_ids.len()).rev() {
            let swap = (next_random(&mut state) as usize) % (index + 1);
            input_ids.swap(index, swap);
        }
        let mut matched = 0;
        for (event, input_id) in events.iter_mut().zip(input_ids) {
            if event.input_id == input_id {
                matched += 1;
            }
            event.input_id = input_id;
        }
        FeedbackShuffleResult {
            events,
            matched_event_count: matched,
            source_event_count: original_ids.len(),
        }
    }
}

/// Output of the reward-shuffled control.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedbackShuffleResult {
    /// Shuffled events with sign, strength, and time preserved.
    pub events: Vec<FeedbackEvent>,
    /// Number of events that retained their original input ID by chance.
    pub matched_event_count: usize,
    /// Number of source events.
    pub source_event_count: usize,
}

/// Fixed normalization and projection used by the learned readout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateReadout {
    /// Individual graph neurons read by the head.
    pub neuron_indices: Vec<u32>,
    /// Fixed per-neuron calibration means.
    pub mean: Vec<f64>,
    /// Fixed per-neuron calibration scales.
    pub std: Vec<f64>,
    /// Minimum allowed calibration scale.
    pub std_floor: f64,
    /// Symmetric clipping value after calibration.
    pub clip: f64,
    /// Row-major linear projection from calibrated activity to head features.
    pub projection: Vec<f64>,
    /// Final value-logit weights.
    pub output_weights: Vec<f64>,
    /// Final value-logit bias.
    pub output_bias: f64,
}

/// Forward values needed by the readout backward calculation.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadoutForward {
    /// Value logit before sigmoid.
    pub logit: f64,
    /// Value probability.
    pub probability: f64,
    calibrated: Vec<f64>,
    projected: Vec<f64>,
}

/// Gradients of the readout and of the selected final activity entries.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadoutGradients {
    /// Projection gradient.
    pub projection: Vec<f64>,
    /// Output weight gradient.
    pub output_weights: Vec<f64>,
    /// Output bias gradient.
    pub output_bias: f64,
    /// Gradient with respect to the full graph activity vector.
    pub activity: Vec<f64>,
}

impl RateReadout {
    /// Creates a deterministic small-random readout with fixed zero/one calibration.
    pub fn new(
        neuron_indices: Vec<u32>,
        projection_dimension: usize,
        seed: u64,
    ) -> Result<Self, RateModelError> {
        if neuron_indices.is_empty()
            || neuron_indices.windows(2).any(|pair| pair[0] >= pair[1])
            || projection_dimension == 0
        {
            return Err(RateModelError::Invalid(
                "readout indices must be non-empty and strictly ascending".to_owned(),
            ));
        }
        let mut random = seed.max(1);
        let feature_count = neuron_indices.len();
        let projection = (0..projection_dimension * feature_count)
            .map(|_| (next_unit(&mut random) - 0.5) * 0.02)
            .collect();
        let output_weights = (0..projection_dimension)
            .map(|_| (next_unit(&mut random) - 0.5) * 0.02)
            .collect();
        Ok(Self {
            neuron_indices,
            mean: vec![0.0; feature_count],
            std: vec![1.0; feature_count],
            std_floor: 1.0e-6,
            clip: 5.0,
            projection,
            output_weights,
            output_bias: 0.0,
        })
    }

    /// Returns the projection feature count.
    pub fn projection_dimension(&self) -> usize {
        self.output_weights.len()
    }

    /// Validates this readout against a graph-sized activity vector.
    pub fn validate(&self, neuron_count: usize) -> Result<(), RateModelError> {
        if self.neuron_indices.is_empty()
            || self
                .neuron_indices
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self
                .neuron_indices
                .iter()
                .any(|index| *index as usize >= neuron_count)
            || self.mean.len() != self.neuron_indices.len()
            || self.std.len() != self.neuron_indices.len()
            || self
                .std
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
            || !self.std_floor.is_finite()
            || self.std_floor <= 0.0
            || !self.clip.is_finite()
            || self.clip <= 0.0
            || self.projection.len() != self.projection_dimension() * self.neuron_indices.len()
            || self
                .projection
                .iter()
                .chain(&self.output_weights)
                .chain(std::iter::once(&self.output_bias))
                .any(|value| !value.is_finite())
        {
            return Err(RateModelError::Invalid(
                "rate readout has invalid dimensions or values".to_owned(),
            ));
        }
        Ok(())
    }

    /// Computes the value logit and probability from final activity.
    pub fn forward(&self, activity: &[f64]) -> Result<ReadoutForward, RateModelError> {
        self.validate(activity.len())?;
        if activity.iter().any(|value| !value.is_finite()) {
            return Err(RateModelError::Invalid(
                "readout activity must be finite".to_owned(),
            ));
        }
        let calibrated = self
            .neuron_indices
            .iter()
            .enumerate()
            .map(|(index, neuron)| {
                ((activity[*neuron as usize] - self.mean[index])
                    / self.std[index].max(self.std_floor))
                .clamp(-self.clip, self.clip)
            })
            .collect::<Vec<_>>();
        let mut projected = vec![0.0; self.projection_dimension()];
        for (projection_index, projected_value) in projected.iter_mut().enumerate() {
            let start = projection_index * calibrated.len();
            *projected_value = self.projection[start..start + calibrated.len()]
                .iter()
                .zip(&calibrated)
                .map(|(weight, value)| weight * value)
                .sum();
        }
        let logit = self.output_bias
            + self
                .output_weights
                .iter()
                .zip(&projected)
                .map(|(weight, value)| weight * value)
                .sum::<f64>();
        Ok(ReadoutForward {
            logit,
            probability: sigmoid(logit),
            calibrated,
            projected,
        })
    }

    /// Backpropagates a logit gradient through the fixed calibration and projection.
    pub fn backward(
        &self,
        activity: &[f64],
        forward: &ReadoutForward,
        logit_gradient: f64,
    ) -> Result<ReadoutGradients, RateModelError> {
        self.validate(activity.len())?;
        if forward.calibrated.len() != self.neuron_indices.len()
            || forward.projected.len() != self.projection_dimension()
            || !logit_gradient.is_finite()
        {
            return Err(RateModelError::Invalid(
                "readout forward state does not match readout".to_owned(),
            ));
        }
        let mut gradients = ReadoutGradients {
            projection: vec![0.0; self.projection.len()],
            output_weights: vec![0.0; self.output_weights.len()],
            output_bias: logit_gradient,
            activity: vec![0.0; activity.len()],
        };
        let mut calibrated_gradient = vec![0.0; self.neuron_indices.len()];
        for projection_index in 0..self.projection_dimension() {
            gradients.output_weights[projection_index] =
                logit_gradient * forward.projected[projection_index];
            let start = projection_index * calibrated_gradient.len();
            for (feature_index, calibrated_gradient_value) in
                calibrated_gradient.iter_mut().enumerate()
            {
                gradients.projection[start + feature_index] = logit_gradient
                    * self.output_weights[projection_index]
                    * forward.calibrated[feature_index];
                *calibrated_gradient_value += logit_gradient
                    * self.output_weights[projection_index]
                    * self.projection[start + feature_index];
            }
        }
        for (feature_index, neuron) in self.neuron_indices.iter().enumerate() {
            let raw = (activity[*neuron as usize] - self.mean[feature_index])
                / self.std[feature_index].max(self.std_floor);
            let derivative = if raw > -self.clip && raw < self.clip {
                1.0 / self.std[feature_index].max(self.std_floor)
            } else {
                0.0
            };
            gradients.activity[*neuron as usize] = calibrated_gradient[feature_index] * derivative;
        }
        Ok(gradients)
    }
}

/// Adam and constraint settings for rate-model training.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateLearningConfig {
    /// Recurrent update count for one observation.
    pub steps_per_observation: usize,
    /// Learning rate for recurrent parameters.
    pub circuit_learning_rate: f64,
    /// Learning rate for the readout.
    pub readout_learning_rate: f64,
    /// Adam first moment coefficient.
    pub beta1: f64,
    /// Adam second moment coefficient.
    pub beta2: f64,
    /// Adam numerical epsilon.
    pub epsilon: f64,
    /// Global gradient norm limit.
    pub gradient_norm_limit: f64,
    /// Minimum recurrent gain.
    pub gain_min: f64,
    /// Maximum recurrent gain.
    pub gain_max: f64,
    /// Minimum alpha after sigmoid.
    pub alpha_min: f64,
    /// Maximum alpha after sigmoid.
    pub alpha_max: f64,
    /// Mean prior coefficient for edge gains.
    pub prior_gain: f64,
    /// Mean prior coefficient for alpha logits.
    pub prior_alpha: f64,
    /// Mean prior coefficient for biases.
    pub prior_bias: f64,
}

impl Default for RateLearningConfig {
    fn default() -> Self {
        Self {
            steps_per_observation: 32,
            circuit_learning_rate: 1.0e-4,
            readout_learning_rate: 1.0e-3,
            beta1: 0.9,
            beta2: 0.999,
            epsilon: 1.0e-5,
            gradient_norm_limit: 0.5,
            gain_min: 0.5,
            gain_max: 2.0,
            alpha_min: 0.05,
            alpha_max: 0.95,
            prior_gain: 1.0e-3,
            prior_alpha: 1.0e-3,
            prior_bias: 1.0e-3,
        }
    }
}

impl RateLearningConfig {
    fn validate(&self) -> Result<(), RateModelError> {
        if self.steps_per_observation == 0
            || !self.circuit_learning_rate.is_finite()
            || self.circuit_learning_rate <= 0.0
            || !self.readout_learning_rate.is_finite()
            || self.readout_learning_rate <= 0.0
            || !(0.0..1.0).contains(&self.beta1)
            || !(0.0..1.0).contains(&self.beta2)
            || !self.epsilon.is_finite()
            || self.epsilon <= 0.0
            || !self.gradient_norm_limit.is_finite()
            || self.gradient_norm_limit <= 0.0
            || !self.gain_min.is_finite()
            || !self.gain_max.is_finite()
            || self.gain_min <= 0.0
            || self.gain_min > self.gain_max
            || !self.alpha_min.is_finite()
            || !self.alpha_max.is_finite()
            || self.alpha_min <= 0.0
            || self.alpha_max >= 1.0
            || self.alpha_min > self.alpha_max
        {
            return Err(RateModelError::Invalid(
                "rate learning configuration is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct AdamVector {
    first: Vec<f64>,
    second: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct AdamState {
    log_gain: AdamVector,
    bias: AdamVector,
    alpha_logits: AdamVector,
    projection: AdamVector,
    output_weights: AdamVector,
    output_bias_first: f64,
    output_bias_second: f64,
    step: u64,
}

impl AdamState {
    fn new(parameters: &RateParameters, readout: &RateReadout) -> Self {
        let vector = |length| AdamVector {
            first: vec![0.0; length],
            second: vec![0.0; length],
        };
        Self {
            log_gain: vector(parameters.log_gain.len()),
            bias: vector(parameters.bias.len()),
            alpha_logits: vector(parameters.alpha_logits.len()),
            projection: vector(readout.projection.len()),
            output_weights: vector(readout.output_weights.len()),
            output_bias_first: 0.0,
            output_bias_second: 0.0,
            step: 0,
        }
    }
}

/// Serializable trainable state for one rate graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateLearningState {
    /// Current recurrent parameters.
    pub parameters: RateParameters,
    /// Current learned readout.
    pub readout: RateReadout,
    /// Fixed initial parameters used by the prior.
    initial_parameters: RateParameters,
    config: RateLearningConfig,
    optimizer: AdamState,
    graph_fingerprint: String,
    #[serde(skip)]
    validated_graph_address: Option<usize>,
    #[serde(skip)]
    rest_activity: Option<Vec<f64>>,
    #[serde(skip)]
    rest_optimizer_step: Option<u64>,
    #[serde(skip)]
    rest_convergence_max_abs: Option<f64>,
}

#[derive(Serialize, Deserialize)]
struct RateCheckpoint {
    version: u32,
    state: RateLearningState,
}

/// Summary of one supervised update or frozen evaluation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingReport {
    /// Input identifier supplied by the caller.
    pub input_id: String,
    /// Event identifier supplied by the caller.
    pub event_id: String,
    /// BCE target, or zero/one for reward/punish.
    pub target: f64,
    /// BCE weighting strength.
    pub strength: f64,
    /// Probability before the update.
    pub probability_before: f64,
    /// Stable weighted BCE loss.
    pub loss: f64,
    /// Gradient norm before clipping.
    pub gradient_norm_before_clip: f64,
    /// Whether clipping was applied.
    pub gradient_was_clipped: bool,
    /// Fraction of edge log-gain entries with a non-zero reverse gradient.
    pub gradient_nonzero_log_gain_fraction: f64,
    /// Whether an optimizer update was applied.
    pub updated: bool,
    /// Optimizer step after this call.
    pub optimizer_step: u64,
}

/// Selects the automatic label used by a rate-model brain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RateBrainRole {
    /// Rewards a sufficiently changed observation and punishes a repeated one.
    Change,
    /// Rewards a repeated observation and punishes a sufficiently changed one.
    NoChange,
}

impl RateBrainRole {
    fn tag(self) -> &'static str {
        match self {
            Self::Change => "change",
            Self::NoChange => "no_change",
        }
    }

    fn kind_for(self, changed: bool) -> FeedbackKind {
        match (self, changed) {
            (Self::Change, true) | (Self::NoChange, false) => FeedbackKind::Reward,
            (Self::Change, false) | (Self::NoChange, true) => FeedbackKind::Punish,
        }
    }
}

/// Fixed automatic-food configuration for one rate-model brain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateBrainConfig {
    /// Number of previous observations considered for the nearest distance.
    pub recent_observations: usize,
    /// Threshold for the normalized root-mean-square feature distance.
    pub distance_threshold: f64,
    /// Rate-model training mode used when a label has positive strength.
    pub training_mode: TrainingMode,
}

impl RateBrainConfig {
    fn validate(&self) -> Result<(), RateModelError> {
        if self.recent_observations == 0
            || !self.distance_threshold.is_finite()
            || self.distance_threshold <= 0.0
        {
            return Err(RateModelError::Invalid(
                "rate brain history and distance threshold must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Diagnostics produced by a rate-model brain evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct RateBrainDiagnostics {
    /// Minimum feature distance to the recent history, if a history exists.
    pub distance: Option<f32>,
    /// Number of history vectors used for this evaluation.
    pub history_count: usize,
    /// Fixed automatic-label threshold.
    pub distance_threshold: f32,
    /// Whether the automatic label classified the observation as changed.
    pub automatic_change: Option<bool>,
    /// Label selected after applying the optional human overlay.
    pub selected_feedback: FeedbackKind,
    /// Feedback strength before the runner's commit weight is applied.
    pub selected_strength: f32,
    /// Whether a human feedback event replaced the automatic label.
    pub human_feedback_applied: bool,
    /// Current learned value probability.
    pub value_probability: f32,
}

/// Pending update returned to the core `BrainRunner`.
#[derive(Clone, Debug, PartialEq)]
pub struct RateBrainUpdate {
    input_id: String,
    stream_id: String,
    event_id: String,
    features: Vec<f32>,
    feedback_kind: FeedbackKind,
    strength: f64,
    timestamp_s: f64,
}

#[derive(Serialize, Deserialize)]
struct RateBrainSnapshot {
    version: u16,
    state: RateLearningState,
    histories: BTreeMap<String, VecDeque<Vec<f32>>>,
}

const RATE_BRAIN_STATE_VERSION: u16 = 1;
const RATE_BRAIN_MODEL_VERSION: u32 = 1;
const RATE_BRAIN_SCHEMA_VERSION: u32 = 1;
const MAX_RATE_BRAIN_HISTORY_STREAMS: usize = 4_096;

/// A rate-model brain registered in habitua core's `ModelRegistry`.
///
/// The connectome and the recurrent parameter state are shared only as
/// intended: all brain instances point at the same immutable graph, while
/// each instance owns its learned readout, recurrent parameters, Adam state,
/// and recent-observation history.
pub struct RateBrainModel {
    graph: Arc<RateGraph>,
    state: RefCell<RateLearningState>,
    histories: RefCell<BTreeMap<String, VecDeque<Vec<f32>>>>,
    role: RateBrainRole,
    config: RateBrainConfig,
    seed: u64,
    human_feedback: BTreeMap<String, FeedbackEvent>,
}

impl RateBrainModel {
    /// Creates a rate-model brain over an immutable shared graph.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        graph: Arc<RateGraph>,
        readout_indices: Vec<u32>,
        projection_dimension: usize,
        seed: u64,
        role: RateBrainRole,
        config: RateBrainConfig,
        learning_config: RateLearningConfig,
        human_feedback: BTreeMap<String, FeedbackEvent>,
    ) -> Result<Self, RateModelError> {
        config.validate()?;
        let state = RateLearningState::new(
            &graph,
            readout_indices,
            projection_dimension,
            seed,
            learning_config,
        )?;
        Ok(Self {
            graph,
            state: RefCell::new(state),
            histories: RefCell::new(BTreeMap::new()),
            role,
            config,
            seed,
            human_feedback,
        })
    }

    /// Returns the current learned rate state for diagnostics or checkpointing.
    pub fn learned_state(&self) -> std::cell::Ref<'_, RateLearningState> {
        self.state.borrow()
    }

    fn evaluate_rate(
        &self,
        input_id: &str,
        stream_id: &str,
        features: &[f32],
        at: habitua::Timestamp,
    ) -> Result<Evaluation<RateBrainDiagnostics, RateBrainUpdate>, ModelError> {
        if features.len() != self.graph.neuron_count()
            || features.iter().any(|value| !value.is_finite())
        {
            return Err(ModelError::Incompatible(
                "rate brain features do not match the graph".to_owned(),
            ));
        }
        let (distance, history_count) = {
            let histories = self.histories.borrow();
            let history = histories.get(stream_id);
            let recent = history
                .map(|items| items.iter().rev().take(self.config.recent_observations))
                .into_iter()
                .flatten();
            let mut count = 0;
            let mut minimum = None;
            for previous in recent {
                count += 1;
                let distance = normalized_rms_distance(features, previous);
                minimum = Some(minimum.map_or(distance, |current: f64| current.min(distance)));
            }
            (minimum, count)
        };
        let automatic_change = distance.map(|value| value > self.config.distance_threshold);
        let automatic_kind = automatic_change
            .map(|changed| self.role.kind_for(changed))
            .unwrap_or_else(|| self.role.kind_for(false));
        let role_specific_feedback = format!("{}:{input_id}", self.role.tag());
        let human_feedback = self
            .human_feedback
            .get(&role_specific_feedback)
            .or_else(|| self.human_feedback.get(input_id));
        let selected_feedback = human_feedback
            .map(|event| event.kind)
            .unwrap_or(automatic_kind);
        let selected_strength = human_feedback
            .map(|event| event.strength)
            .unwrap_or_else(|| f64::from(u8::from(automatic_change.is_some())));
        let input = features
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        let probability = self
            .state
            .borrow_mut()
            .predict_final(&self.graph, &input)
            .map_err(rate_model_error)?
            .1
            .probability;
        let readiness = if distance.is_some() {
            Readiness::Evaluated
        } else {
            Readiness::BaselineInsufficient
        };
        let novelty = distance.map(|value| {
            (value / self.config.distance_threshold)
                .clamp(0.0, 1.0)
                .to_owned() as f32
        });
        let update = RateBrainUpdate {
            input_id: input_id.to_owned(),
            stream_id: stream_id.to_owned(),
            event_id: format!("rate-brain:{}:{input_id}", self.role.tag()),
            features: features.to_vec(),
            feedback_kind: selected_feedback,
            strength: selected_strength,
            timestamp_s: at.as_duration().as_secs_f64(),
        };
        Ok(Evaluation {
            novelty_short: novelty,
            novelty_long: None,
            familiarity_short: novelty.map(|value| 1.0 - value),
            familiarity_long: None,
            recency: None,
            pattern_id: None,
            readiness,
            diagnostics: RateBrainDiagnostics {
                distance: distance.map(|value| value as f32),
                history_count,
                distance_threshold: self.config.distance_threshold as f32,
                automatic_change,
                selected_feedback,
                selected_strength: selected_strength as f32,
                human_feedback_applied: human_feedback.is_some(),
                value_probability: probability as f32,
            },
            update,
        })
    }

    fn validate_histories(
        histories: &BTreeMap<String, VecDeque<Vec<f32>>>,
        dimension: usize,
        maximum: usize,
    ) -> Result<(), io::Error> {
        if histories.len() > MAX_RATE_BRAIN_HISTORY_STREAMS {
            return Err(invalid_rate_brain_state(
                "rate brain history stream count exceeds its limit",
            ));
        }
        for history in histories.values() {
            if history.len() > maximum
                || history.iter().any(|features| {
                    features.len() != dimension || features.iter().any(|value| !value.is_finite())
                })
            {
                return Err(invalid_rate_brain_state(
                    "rate brain history has an invalid shape or value",
                ));
            }
        }
        Ok(())
    }

    fn state_element_count(&self) -> Result<usize, io::Error> {
        let state = self.state.borrow();
        let mut count = state
            .parameters
            .log_gain
            .len()
            .checked_mul(5)
            .and_then(|value| value.checked_add(state.readout.projection.len() * 4))
            .and_then(|value| value.checked_add(state.readout.output_weights.len() * 4))
            .and_then(|value| value.checked_add(state.parameters.bias.len() * 6))
            .ok_or_else(|| invalid_rate_brain_state("rate brain state element count overflowed"))?;
        for (stream, history) in self.histories.borrow().iter() {
            count = count
                .checked_add(stream.len())
                .and_then(|value| value.checked_add(history.len()))
                .and_then(|value| {
                    history
                        .iter()
                        .try_fold(value, |total, features| total.checked_add(features.len()))
                })
                .ok_or_else(|| {
                    invalid_rate_brain_state("rate brain history element count overflowed")
                })?;
        }
        Ok(count)
    }
}

impl Model for RateBrainModel {
    type Diagnostics = RateBrainDiagnostics;
    type Update = RateBrainUpdate;

    fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: habitua::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        self.evaluate_rate(stream_id, stream_id, features, at)
    }

    fn evaluate_input_with_id(
        &self,
        input_id: &str,
        stream_id: &str,
        input: ModelInput<'_>,
        at: habitua::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        match input {
            ModelInput::Features(features) => self.evaluate_rate(input_id, stream_id, features, at),
            ModelInput::Failure(_) => Err(ModelError::Incompatible(
                "rate brain requires a feature-vector input".to_owned(),
            )),
        }
    }

    fn commit(&mut self, mut update: Self::Update, weight: f32) -> Result<(), ModelError> {
        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            return Err(ModelError::InvalidLearningWeight);
        }
        update.strength *= f64::from(weight);
        let event = FeedbackEvent::new(
            update.event_id,
            update.input_id,
            update.feedback_kind,
            update.strength,
            update.timestamp_s,
        )
        .map_err(rate_model_error)?;
        let input = update
            .features
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        if event.strength > 0.0 && self.config.training_mode != TrainingMode::Frozen {
            self.state
                .borrow_mut()
                .train_observation(&self.graph, &input, &event, self.config.training_mode)
                .map_err(rate_model_error)?;
        }
        let mut histories = self.histories.borrow_mut();
        let history = histories.entry(update.stream_id).or_default();
        history.push_back(update.features);
        while history.len() > self.config.recent_observations {
            history.pop_front();
        }
        Ok(())
    }

    fn identity(&self) -> ModelIdentity {
        ModelIdentity {
            model_version: RATE_BRAIN_MODEL_VERSION,
            feature_schema_version: RATE_BRAIN_SCHEMA_VERSION,
        }
    }

    fn fingerprint(&self) -> u64 {
        let mut words = Vec::new();
        words.extend(self.graph.fingerprint().as_bytes().chunks(8).map(|chunk| {
            let mut bytes = [0_u8; 8];
            bytes[..chunk.len()].copy_from_slice(chunk);
            u64::from_le_bytes(bytes)
        }));
        words.extend([
            RATE_BRAIN_MODEL_VERSION as u64,
            RATE_BRAIN_SCHEMA_VERSION as u64,
            self.role as u64,
            self.config.recent_observations as u64,
            self.config.distance_threshold.to_bits(),
            self.config.training_mode as u64,
            self.seed,
        ]);
        habitua_pattern_hash(words)
    }
}

impl PersistentModel for RateBrainModel {
    fn state_format_version(&self) -> u16 {
        RATE_BRAIN_STATE_VERSION
    }

    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()> {
        let state = self.state.borrow();
        let histories = self.histories.borrow();
        Self::validate_histories(
            &histories,
            self.graph.neuron_count(),
            self.config.recent_observations,
        )?;
        serde_json::to_writer(
            writer,
            &RateBrainSnapshot {
                version: RATE_BRAIN_STATE_VERSION,
                state: state.clone(),
                histories: histories.clone(),
            },
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn load_state(&mut self, reader: &mut dyn Read) -> io::Result<()> {
        let snapshot: RateBrainSnapshot = serde_json::from_reader(reader)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if snapshot.version != RATE_BRAIN_STATE_VERSION {
            return Err(invalid_rate_brain_state(
                "unsupported rate brain state version",
            ));
        }
        let current = self.state.borrow();
        if snapshot.state.config != current.config
            || !readout_structure_matches(&snapshot.state.readout, &current.readout)
        {
            return Err(invalid_rate_brain_state(
                "rate brain state configuration does not match",
            ));
        }
        drop(current);
        let mut state = snapshot.state;
        state
            .validate_graph(&self.graph)
            .map_err(rate_model_io_error)?;
        Self::validate_histories(
            &snapshot.histories,
            self.graph.neuron_count(),
            self.config.recent_observations,
        )?;
        *self.state.borrow_mut() = state;
        *self.histories.borrow_mut() = snapshot.histories;
        Ok(())
    }

    fn reserve_load_budget(&self, load_budget: &mut LoadBudget) -> io::Result<()> {
        load_budget.reserve(self.state_element_count()?)
    }

    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()> {
        self.reserve_load_budget(load_budget)?;
        self.load_state(reader)
    }
}

/// Registers the rate-brain model kind for use by habitua core.
pub fn register_rate_brain_model(
    registry: &mut habitua::ModelRegistry,
    graph: Arc<RateGraph>,
    readout_indices: Vec<u32>,
    projection_dimension: usize,
    seed_base: u64,
    learning_config: RateLearningConfig,
    human_feedback: BTreeMap<String, FeedbackEvent>,
) -> Result<(), habitua::RegistryError> {
    registry.register::<RateBrainModel, _, _>(
        "rate_brain",
        move |brain| {
            let settings = match &brain.model.settings {
                habitua::ModelSettings::Custom(settings) => settings,
                _ => {
                    return Err(habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: "rate_brain requires custom settings".to_owned(),
                    });
                }
            };
            let role = match settings.get("role").map(String::as_str) {
                Some("change") => RateBrainRole::Change,
                Some("no_change") => RateBrainRole::NoChange,
                _ => {
                    return Err(habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: "rate_brain role must be change or no_change".to_owned(),
                    });
                }
            };
            let recent_observations = parse_setting(settings, "recent_observations", brain)?;
            let distance_threshold = parse_setting_f64(settings, "distance_threshold", brain)?;
            let training_mode = match settings.get("training_mode").map(String::as_str) {
                Some("frozen") => TrainingMode::Frozen,
                Some("readout_only") => TrainingMode::ReadoutOnly,
                Some("full") => TrainingMode::Full,
                _ => {
                    return Err(habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: "rate_brain training_mode is invalid".to_owned(),
                    });
                }
            };
            let seed = settings
                .get("seed")
                .map(String::as_str)
                .unwrap_or("")
                .parse::<u64>()
                .map_err(|_| habitua::RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "rate_brain seed is invalid".to_owned(),
                })?
                .wrapping_add(seed_base);
            RateBrainModel::new(
                Arc::clone(&graph),
                readout_indices.clone(),
                projection_dimension,
                seed,
                role,
                RateBrainConfig {
                    recent_observations,
                    distance_threshold,
                    training_mode,
                },
                learning_config.clone(),
                human_feedback.clone(),
            )
            .map_err(|error| habitua::RegistryError::Model {
                brain: brain.id.clone(),
                reason: error.to_string(),
            })
        },
        rate_brain_report,
    )
}

fn rate_brain_report(
    evaluation: &Evaluation<RateBrainDiagnostics, RateBrainUpdate>,
    config: &habitua::BrainConfig,
) -> ModelReport {
    let diagnostics = &evaluation.diagnostics;
    let mut metrics = vec![
        Metric::new(
            "value_probability",
            diagnostics.value_probability,
            "probability",
            MetricDirection::HigherIsMore,
        ),
        Metric::new(
            "history_count",
            diagnostics.history_count as f32,
            "count",
            MetricDirection::Informational,
        ),
        Metric::new(
            "automatic_change",
            f32::from(u8::from(diagnostics.automatic_change.unwrap_or(false))),
            "boolean",
            MetricDirection::HigherIsMore,
        ),
    ];
    if let Some(distance) = diagnostics.distance {
        metrics.push(Metric::new(
            "feature_distance",
            distance,
            "normalized-rms",
            MetricDirection::HigherIsMore,
        ));
    }
    let evidence = vec![Evidence {
        name: "automatic-food".to_owned(),
        current: diagnostics.distance,
        baseline: Some(diagnostics.distance_threshold),
        sample_count: Some(diagnostics.history_count),
        window: None,
        reference_ids: Vec::new(),
        detail: format!(
            "{} role; human overlay={}",
            config.role, diagnostics.human_feedback_applied
        ),
    }];
    let mut report = ModelReport {
        novelty_short: evaluation.novelty_short,
        novelty_long: None,
        familiarity_short: evaluation.familiarity_short,
        familiarity_long: None,
        recency: None,
        pattern_id: None,
        readiness: evaluation.readiness.clone(),
        learning_signal: if evaluation.readiness == Readiness::Evaluated {
            if diagnostics.automatic_change.unwrap_or(false) {
                LearningSignal::Abnormal
            } else {
                LearningSignal::Normal
            }
        } else {
            LearningSignal::Unknown
        },
        metrics,
        evidence,
        reactions: Vec::new(),
    };
    let role_positive = diagnostics.value_probability >= config.reactions.threshold;
    let reaction_kind = match config.role.as_str() {
        "change" => habitua::ReactionKind::Novel,
        "no_change" => habitua::ReactionKind::Repeat,
        _ => habitua::ReactionKind::Deviation,
    };
    if evaluation.readiness == Readiness::Evaluated
        && role_positive
        && config.reactions.kinds.contains(&reaction_kind)
    {
        report.reactions.push(Reaction {
            brain: config.id.clone(),
            target: config.input.target.clone(),
            kind: reaction_kind,
            strength: Some(diagnostics.value_probability),
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            readiness: report.readiness.clone(),
        });
    }
    report
}

/// Configuration for a small readout over a cached frozen-CNS response.
///
/// The readout does not retain a connectome and never calls [`RateBackward`].
/// Its input is a fixed-size response summary supplied by the caller, and its
/// own state consists of a small logistic model and bounded stream histories.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrozenReadoutConfig {
    /// Number of previous response summaries used for the distance features.
    pub recent_observations: usize,
    /// Threshold that converts the nearest recent distance into automatic food.
    pub distance_threshold: f32,
    /// Fixed scale for the absolute difference history feature.
    pub previous_absolute_difference_scale: f32,
    /// Fixed scale in seconds for the elapsed-time history feature.
    pub history_elapsed_time_scale_seconds: f32,
    /// Maximum age in seconds for the short-term history.
    #[serde(default = "default_history_max_age_seconds")]
    pub history_max_age_seconds: f32,
    /// L2 regularization coefficient for the logistic weights.
    pub l2_regularization: f32,
    /// Online gradient step size.
    pub learning_rate: f32,
    /// Probability threshold used by the model reporter.
    pub reaction_threshold: f32,
    /// Whether positive-weight commits update the readout weights.
    pub training_mode: TrainingMode,
    /// L2 distance threshold for applying a human-feedback case.
    #[serde(default = "default_case_memory_distance_threshold")]
    pub case_memory_distance_threshold: f32,
    /// Exponential decay time constant for human-feedback cases.
    #[serde(default = "default_case_memory_time_constant_seconds")]
    pub case_memory_time_constant_seconds: f32,
    /// Logit-scale applied to the weighted case-memory value.
    #[serde(default = "default_case_memory_logit_scale")]
    pub case_memory_logit_scale: f32,
    /// Maximum number of retained human-feedback cases.
    #[serde(default = "default_case_memory_max_cases")]
    pub case_memory_max_cases: usize,
}

impl FrozenReadoutConfig {
    fn validate(&self) -> Result<(), RateModelError> {
        if self.recent_observations == 0
            || !self.distance_threshold.is_finite()
            || self.distance_threshold <= 0.0
            || !self.previous_absolute_difference_scale.is_finite()
            || self.previous_absolute_difference_scale <= 0.0
            || !self.history_elapsed_time_scale_seconds.is_finite()
            || self.history_elapsed_time_scale_seconds <= 0.0
            || !self.history_max_age_seconds.is_finite()
            || self.history_max_age_seconds <= 0.0
            || !self.l2_regularization.is_finite()
            || self.l2_regularization < 0.0
            || !self.learning_rate.is_finite()
            || self.learning_rate <= 0.0
            || !self.reaction_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.reaction_threshold)
            || !self.case_memory_distance_threshold.is_finite()
            || self.case_memory_distance_threshold <= 0.0
            || !self.case_memory_time_constant_seconds.is_finite()
            || self.case_memory_time_constant_seconds <= 0.0
            || !self.case_memory_logit_scale.is_finite()
            || self.case_memory_logit_scale < 0.0
            || self.case_memory_max_cases == 0
        {
            return Err(RateModelError::Invalid(
                "frozen readout configuration is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

fn default_history_max_age_seconds() -> f32 {
    300.0
}

fn default_case_memory_distance_threshold() -> f32 {
    4.0
}

fn default_case_memory_time_constant_seconds() -> f32 {
    600.0
}

fn default_case_memory_logit_scale() -> f32 {
    4.0
}

fn default_case_memory_max_cases() -> usize {
    256
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FrozenReadoutCase {
    features: Vec<f32>,
    #[serde(default)]
    input_id: String,
    kind: FeedbackKind,
    strength: f32,
    /// Feedback event time. Case decay starts at this time, not at the
    /// observation or commit time.
    timestamp_s: f64,
    /// Arrival/commit time retained for delayed-feedback diagnostics.
    #[serde(default)]
    received_at_s: Option<f64>,
    event_id: String,
    #[serde(default)]
    revision: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FrozenReadoutCaseDiagnostics {
    adjustment: f32,
    match_count: usize,
    nearest_distance: Option<f32>,
}

/// Diagnostics from one cached-response readout evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenReadoutDiagnostics {
    /// Nearest normalized RMS distance to the recent history.
    pub feature_distance: Option<f32>,
    /// Mean absolute difference from the immediately preceding response.
    pub previous_absolute_difference: Option<f32>,
    /// Number of recent responses within the automatic-food threshold.
    pub repeat_count: usize,
    /// Seconds since the preceding response in the same stream.
    pub elapsed_seconds: Option<f32>,
    /// Automatic change classification, absent for the first observation.
    pub automatic_change: Option<bool>,
    /// Probability of the brain's role-specific class.
    pub target_probability: f32,
    /// Probability before case-memory correction.
    pub base_probability: f32,
    /// Logit correction from nearby human-feedback cases.
    pub case_memory_adjustment: f32,
    /// Number of human-feedback cases within the configured distance.
    pub case_memory_match_count: usize,
    /// Nearest matching case distance, if any.
    pub case_memory_nearest_distance: Option<f32>,
    /// Role-specific feedback selected after the human overlay.
    pub selected_feedback: FeedbackKind,
    /// Feedback strength before the runner commit weight.
    pub selected_strength: f32,
    /// Whether a human feedback event replaced the automatic label.
    pub human_feedback_applied: bool,
    /// Number of logistic parameter updates committed before this evaluation.
    pub update_count: u64,
}

/// Pending update for a cached-response readout.
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenReadoutUpdate {
    input_id: String,
    stream_id: String,
    augmented_features: Vec<f32>,
    base_features: Vec<f32>,
    feedback_kind: FeedbackKind,
    strength: f32,
    timestamp_s: f64,
    feedback_timestamp_s: Option<f64>,
    feedback_received_at_s: Option<f64>,
    human_feedback_applied: bool,
    human_feedback_event_id: Option<String>,
    human_feedback_revision: Option<u64>,
}

/// Mutable human-feedback overlay shared by the two readout instances.
///
/// The automatic label is always generated from the recent response history.
/// An event inserted here is an explicit reward or punishment override for a
/// role and input ID, which lets a caller apply feedback after construction.
#[derive(Clone, Default)]
pub struct FrozenReadoutFeedback {
    events: Arc<Mutex<BTreeMap<String, FeedbackEvent>>>,
}

impl FrozenReadoutFeedback {
    /// Creates a feedback handle initialized with the supplied events.
    pub fn new(events: BTreeMap<String, FeedbackEvent>) -> Self {
        Self {
            events: Arc::new(Mutex::new(events)),
        }
    }

    /// Inserts an event or a strictly newer correction for its key.
    pub fn insert(&self, key: impl Into<String>, event: FeedbackEvent) {
        if let Ok(mut events) = self.events.lock() {
            let key = key.into();
            if events
                .get(&key)
                .is_none_or(|previous| event.revision > previous.revision)
            {
                events.insert(key, event);
            }
        }
    }

    /// Removes all explicit feedback from the overlay.
    pub fn clear(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
    }

    /// Creates an independent copy of the current overlay for a controlled
    /// comparison branch.
    pub fn independent_clone(&self) -> Self {
        let events = self
            .events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default();
        Self::new(events)
    }
}

/// One response stored in a frozen-readout stream history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrozenReadoutHistoryEntry {
    /// Base response features before the four history features are appended.
    pub features: Vec<f32>,
    /// Observation timestamp in seconds from the adapter epoch.
    pub timestamp_s: f64,
}

/// The four history values shared by batch training and online evaluation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenReadoutHistoryFeatures {
    /// Nearest normalized RMS distance to recent observations.
    pub distance: Option<f32>,
    /// Mean absolute difference from the immediately previous observation.
    pub previous_absolute_difference: Option<f32>,
    /// Number of recent observations within the distance threshold.
    pub repeat_count: usize,
    /// Seconds since the immediately previous observation.
    pub elapsed_seconds: Option<f32>,
}

/// Computes the history values used by both batch and online frozen readouts.
///
/// The current timestamp is passed explicitly so a bounded history does not
/// change the elapsed time when old entries are removed.
pub fn frozen_readout_history_features(
    features: &[f32],
    history: &[FrozenReadoutHistoryEntry],
    now_s: f64,
    recent_observations: usize,
    distance_threshold: f32,
) -> FrozenReadoutHistoryFeatures {
    frozen_readout_history_features_with_expiry(
        features,
        history,
        now_s,
        recent_observations,
        distance_threshold,
        f32::INFINITY,
    )
}

/// Computes history values while ignoring observations older than the fixed
/// short-term history window.
pub fn frozen_readout_history_features_with_expiry(
    features: &[f32],
    history: &[FrozenReadoutHistoryEntry],
    now_s: f64,
    recent_observations: usize,
    distance_threshold: f32,
    history_max_age_seconds: f32,
) -> FrozenReadoutHistoryFeatures {
    let recent = history.iter().rev().take(recent_observations);
    let previous = recent
        .clone()
        .find(|entry| (now_s - entry.timestamp_s).max(0.0) as f32 <= history_max_age_seconds);
    let mut minimum = None;
    let mut repeat_count = 0;
    for previous in recent {
        if (now_s - previous.timestamp_s).max(0.0) as f32 > history_max_age_seconds {
            continue;
        }
        let current_distance = normalized_rms_distance_f32(features, &previous.features);
        if current_distance <= distance_threshold {
            repeat_count += 1;
        }
        minimum = Some(minimum.map_or(current_distance, |value: f32| value.min(current_distance)));
    }
    FrozenReadoutHistoryFeatures {
        distance: minimum,
        previous_absolute_difference: previous
            .map(|previous| mean_absolute_difference(features, &previous.features)),
        repeat_count,
        elapsed_seconds: previous.map(|previous| (now_s - previous.timestamp_s).max(0.0) as f32),
    }
}

/// Builds the four augmented history features in the canonical order.
pub fn frozen_readout_augmented_features(
    features: &[f32],
    history: &[FrozenReadoutHistoryEntry],
    now_s: f64,
    recent_observations: usize,
    distance_threshold: f32,
    previous_absolute_difference_scale: f32,
    history_elapsed_time_scale_seconds: f32,
) -> (FrozenReadoutHistoryFeatures, Vec<f32>) {
    frozen_readout_augmented_features_with_expiry(
        features,
        history,
        now_s,
        recent_observations,
        distance_threshold,
        previous_absolute_difference_scale,
        history_elapsed_time_scale_seconds,
        f32::INFINITY,
    )
}

/// Builds augmented history features with an explicit short-term history
/// window. Batch and online evaluation use this function with the same value.
#[allow(clippy::too_many_arguments)]
pub fn frozen_readout_augmented_features_with_expiry(
    features: &[f32],
    history: &[FrozenReadoutHistoryEntry],
    now_s: f64,
    recent_observations: usize,
    distance_threshold: f32,
    previous_absolute_difference_scale: f32,
    history_elapsed_time_scale_seconds: f32,
    history_max_age_seconds: f32,
) -> (FrozenReadoutHistoryFeatures, Vec<f32>) {
    let history_features = frozen_readout_history_features_with_expiry(
        features,
        history,
        now_s,
        recent_observations,
        distance_threshold,
        history_max_age_seconds,
    );
    let mut augmented = Vec::with_capacity(features.len() + FROZEN_READOUT_HISTORY_FEATURES);
    augmented.extend_from_slice(features);
    augmented.push(
        history_features.previous_absolute_difference.unwrap_or(0.0)
            / previous_absolute_difference_scale,
    );
    augmented.push(history_features.distance.unwrap_or(0.0) / distance_threshold);
    augmented.push(history_features.repeat_count as f32 / recent_observations.max(1) as f32);
    augmented.push(
        history_features
            .elapsed_seconds
            .unwrap_or(0.0)
            .min(history_elapsed_time_scale_seconds)
            / history_elapsed_time_scale_seconds,
    );
    (history_features, augmented)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FrozenReadoutState {
    weights: Vec<f32>,
    bias: f32,
    update_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FrozenReadoutSnapshot {
    version: u16,
    feature_dimension: usize,
    role: RateBrainRole,
    config: FrozenReadoutConfig,
    state: FrozenReadoutState,
    histories: BTreeMap<String, Vec<FrozenReadoutHistoryEntry>>,
    #[serde(default)]
    standardization: Option<FrozenReadoutStandardization>,
    #[serde(default)]
    cases: Vec<FrozenReadoutCase>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FrozenReadoutStandardization {
    mean: Vec<f32>,
    scale: Vec<f32>,
}

const FROZEN_READOUT_MODEL_VERSION: u32 = 1;
const FROZEN_READOUT_SCHEMA_VERSION: u32 = 2;
const FROZEN_READOUT_STATE_VERSION: u16 = 1;
const FROZEN_READOUT_HISTORY_FEATURES: usize = 4;
const MAX_FROZEN_READOUT_HISTORY_STREAMS: usize = 4_096;

/// A small logistic readout over a shared, precomputed frozen-CNS response.
///
/// `RateBrainModel` remains available for the earlier full rate-learning
/// path. This model is the lightweight evaluation path: both brains receive
/// the same cached response summary, while each has independent weights and
/// stream history.
pub struct FrozenReadoutModel {
    feature_dimension: usize,
    role: RateBrainRole,
    config: FrozenReadoutConfig,
    state: RefCell<FrozenReadoutState>,
    histories: RefCell<BTreeMap<String, Vec<FrozenReadoutHistoryEntry>>>,
    standardization: Option<FrozenReadoutStandardization>,
    seed: u64,
    human_feedback: FrozenReadoutFeedback,
    cases: RefCell<Vec<FrozenReadoutCase>>,
}

impl FrozenReadoutModel {
    /// Creates a readout whose input is a fixed-size cached response summary.
    pub fn new(
        feature_dimension: usize,
        seed: u64,
        role: RateBrainRole,
        config: FrozenReadoutConfig,
        human_feedback: BTreeMap<String, FeedbackEvent>,
    ) -> Result<Self, RateModelError> {
        if feature_dimension == 0 {
            return Err(RateModelError::Invalid(
                "frozen readout feature dimension must be positive".to_owned(),
            ));
        }
        config.validate()?;
        let dimension = feature_dimension
            .checked_add(FROZEN_READOUT_HISTORY_FEATURES)
            .ok_or_else(|| {
                RateModelError::Invalid("frozen readout dimension overflowed".to_owned())
            })?;
        let mut random = seed.max(1);
        let weights = (0..dimension)
            .map(|_| (next_unit(&mut random) as f32 - 0.5) * 0.01)
            .collect();
        Ok(Self {
            feature_dimension,
            role,
            config,
            state: RefCell::new(FrozenReadoutState {
                weights,
                bias: 0.0,
                update_count: 0,
            }),
            histories: RefCell::new(BTreeMap::new()),
            standardization: None,
            seed,
            human_feedback: FrozenReadoutFeedback::new(human_feedback),
            cases: RefCell::new(Vec::new()),
        })
    }

    /// Creates a readout using a feedback handle that can be updated later.
    pub fn with_feedback_handle(
        feature_dimension: usize,
        seed: u64,
        role: RateBrainRole,
        config: FrozenReadoutConfig,
        human_feedback: FrozenReadoutFeedback,
    ) -> Result<Self, RateModelError> {
        let mut model = Self::new(feature_dimension, seed, role, config, BTreeMap::new())?;
        model.human_feedback = human_feedback;
        Ok(model)
    }

    /// Returns the number of base features accepted by this readout.
    pub const fn feature_dimension(&self) -> usize {
        self.feature_dimension
    }

    /// Returns a copy of the current logistic weights for diagnostics.
    pub fn weights(&self) -> Vec<f32> {
        self.state.borrow().weights.clone()
    }

    /// Configures feature standardization for the response and history vector.
    pub fn set_standardization(
        &mut self,
        mean: Vec<f32>,
        scale: Vec<f32>,
    ) -> Result<(), RateModelError> {
        let expected = self
            .feature_dimension
            .checked_add(FROZEN_READOUT_HISTORY_FEATURES)
            .ok_or_else(|| {
                RateModelError::Invalid("frozen readout dimension overflowed".to_owned())
            })?;
        if mean.len() != expected
            || scale.len() != expected
            || mean.iter().any(|value| !value.is_finite())
            || scale
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(RateModelError::Invalid(
                "frozen readout standardization has an invalid shape or value".to_owned(),
            ));
        }
        self.standardization = Some(FrozenReadoutStandardization { mean, scale });
        Ok(())
    }

    /// Replaces the logistic parameters with a fitted batch model.
    pub fn set_fitted_parameters(
        &mut self,
        weights: Vec<f32>,
        bias: f32,
    ) -> Result<(), RateModelError> {
        let expected = self
            .feature_dimension
            .checked_add(FROZEN_READOUT_HISTORY_FEATURES)
            .ok_or_else(|| {
                RateModelError::Invalid("frozen readout dimension overflowed".to_owned())
            })?;
        if weights.len() != expected
            || weights.iter().any(|value| !value.is_finite())
            || !bias.is_finite()
        {
            return Err(RateModelError::Invalid(
                "fitted frozen readout parameters have an invalid shape or value".to_owned(),
            ));
        }
        let mut state = self.state.borrow_mut();
        state.weights = weights;
        state.bias = bias;
        Ok(())
    }

    fn standardized_features(&self, features: &[f32]) -> Result<Vec<f32>, ModelError> {
        if let Some(standardization) = &self.standardization {
            if standardization.mean.len() != features.len()
                || standardization.scale.len() != features.len()
            {
                return Err(ModelError::Incompatible(
                    "frozen readout standardization dimension does not match features".to_owned(),
                ));
            }
            Ok(features
                .iter()
                .zip(&standardization.mean)
                .zip(&standardization.scale)
                .map(|((value, mean), scale)| (*value - *mean) / *scale)
                .collect())
        } else {
            Ok(features.to_vec())
        }
    }

    fn evaluate_readout(
        &self,
        input_id: &str,
        stream_id: &str,
        features: &[f32],
        at: habitua::Timestamp,
    ) -> Result<Evaluation<FrozenReadoutDiagnostics, FrozenReadoutUpdate>, ModelError> {
        if features.len() != self.feature_dimension
            || features.iter().any(|value| !value.is_finite())
        {
            return Err(ModelError::Incompatible(
                "frozen readout features have an invalid shape or value".to_owned(),
            ));
        }
        let timestamp_s = at.as_duration().as_secs_f64();
        let (history_features, augmented_features) = {
            let histories = self.histories.borrow();
            let history = histories.get(stream_id).map_or(&[][..], Vec::as_slice);
            frozen_readout_augmented_features_with_expiry(
                features,
                history,
                timestamp_s,
                self.config.recent_observations,
                self.config.distance_threshold,
                self.config.previous_absolute_difference_scale,
                self.config.history_elapsed_time_scale_seconds,
                self.config.history_max_age_seconds,
            )
        };
        let automatic_change = history_features
            .distance
            .map(|value| value > self.config.distance_threshold);
        let automatic_kind = automatic_change
            .map(|changed| self.role.kind_for(changed))
            .unwrap_or_else(|| self.role.kind_for(false));
        let role_specific_feedback = format!("{}:{input_id}", self.role.tag());
        let feedback_guard = self.human_feedback.events.lock().map_err(|_| {
            ModelError::Incompatible("frozen readout feedback lock poisoned".to_owned())
        })?;
        let human_feedback = feedback_guard
            .get(&role_specific_feedback)
            .or_else(|| feedback_guard.get(input_id))
            .filter(|event| !event.cancelled)
            .cloned();
        let selected_feedback = human_feedback
            .as_ref()
            .map(|event| event.kind)
            .unwrap_or(automatic_kind);
        let selected_strength = human_feedback
            .as_ref()
            .map(|event| event.strength as f32)
            .unwrap_or_else(|| f32::from(u8::from(automatic_change.is_some())));
        let feedback_timestamp_s = human_feedback.as_ref().map(|event| event.timestamp_s);
        let feedback_received_at_s = human_feedback
            .as_ref()
            .and_then(|event| event.received_at_s)
            .or(feedback_timestamp_s);
        drop(feedback_guard);
        let standardized_features = self.standardized_features(&augmented_features)?;
        let base_probability = self.predict_augmented(&standardized_features)?;
        let case_diagnostics =
            self.case_memory_diagnostics(input_id, &standardized_features, timestamp_s);
        let probability =
            sigmoid_f32(logit_from_probability(base_probability) + case_diagnostics.adjustment);
        let human_feedback_applied = human_feedback.is_some();
        let human_feedback_event_id = human_feedback.as_ref().map(|event| event.event_id.clone());
        let human_feedback_revision = human_feedback.as_ref().map(|event| event.revision);
        let update_count = self.state.borrow().update_count;
        let readiness = if history_features.distance.is_some() {
            Readiness::Evaluated
        } else {
            Readiness::BaselineInsufficient
        };
        let novelty = history_features
            .distance
            .map(|value| (value / self.config.distance_threshold).clamp(0.0, 1.0));
        Ok(Evaluation {
            novelty_short: novelty,
            novelty_long: None,
            familiarity_short: novelty.map(|value| 1.0 - value),
            familiarity_long: None,
            recency: None,
            pattern_id: None,
            readiness,
            diagnostics: FrozenReadoutDiagnostics {
                feature_distance: history_features.distance,
                previous_absolute_difference: history_features.previous_absolute_difference,
                repeat_count: history_features.repeat_count,
                elapsed_seconds: history_features.elapsed_seconds,
                automatic_change,
                target_probability: probability,
                base_probability,
                case_memory_adjustment: case_diagnostics.adjustment,
                case_memory_match_count: case_diagnostics.match_count,
                case_memory_nearest_distance: case_diagnostics.nearest_distance,
                selected_feedback,
                selected_strength,
                human_feedback_applied,
                update_count,
            },
            update: FrozenReadoutUpdate {
                input_id: input_id.to_owned(),
                stream_id: stream_id.to_owned(),
                augmented_features: standardized_features,
                base_features: features.to_vec(),
                feedback_kind: selected_feedback,
                strength: selected_strength,
                timestamp_s,
                feedback_timestamp_s,
                feedback_received_at_s,
                human_feedback_applied,
                human_feedback_event_id,
                human_feedback_revision,
            },
        })
    }

    fn reconcile_case_memory(&self) {
        let events = self
            .human_feedback
            .events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default();
        self.cases.borrow_mut().retain(|case| {
            events
                .values()
                .find(|event| event.event_id == case.event_id)
                .is_none_or(|event| !event.cancelled && event.revision <= case.revision)
        });
    }

    fn case_memory_diagnostics(
        &self,
        input_id: &str,
        features: &[f32],
        timestamp_s: f64,
    ) -> FrozenReadoutCaseDiagnostics {
        self.reconcile_case_memory();
        let cases = self.cases.borrow();
        let threshold = self.config.case_memory_distance_threshold;
        let time_constant = self.config.case_memory_time_constant_seconds;
        let mut weighted_value = 0.0_f32;
        let mut match_count = 0;
        let mut nearest_distance = None;
        for case in cases.iter() {
            // The exact image hash is the primary key. Feature distance is
            // only a secondary match within that exact key.
            if case.input_id != input_id {
                continue;
            }
            let distance = l2_distance_f32(features, &case.features);
            if !distance.is_finite() || distance > threshold {
                continue;
            }
            match_count += 1;
            nearest_distance =
                Some(nearest_distance.map_or(distance, |value: f32| value.min(distance)));
            let age = (timestamp_s - case.timestamp_s).max(0.0) as f32;
            let decay = (-age / time_constant).exp();
            let proximity = (1.0 - distance / threshold).max(0.0);
            let weight = case.strength * decay * proximity;
            let sign = match case.kind {
                FeedbackKind::Reward => 1.0,
                FeedbackKind::Punish => -1.0,
            };
            weighted_value += sign * weight;
        }
        let adjustment = self.config.case_memory_logit_scale * weighted_value.clamp(-1.0, 1.0);
        FrozenReadoutCaseDiagnostics {
            adjustment,
            match_count,
            nearest_distance,
        }
    }

    fn predict_augmented(&self, features: &[f32]) -> Result<f32, ModelError> {
        let state = self.state.borrow();
        if features.len() != state.weights.len() {
            return Err(ModelError::Incompatible(
                "frozen readout augmented dimension does not match state".to_owned(),
            ));
        }
        let logit = state.bias
            + state
                .weights
                .iter()
                .zip(features)
                .map(|(weight, feature)| weight * feature)
                .sum::<f32>();
        Ok(sigmoid_f32(logit))
    }

    fn validate_histories(
        histories: &BTreeMap<String, Vec<FrozenReadoutHistoryEntry>>,
        dimension: usize,
        maximum: usize,
    ) -> Result<(), io::Error> {
        if histories.len() > MAX_FROZEN_READOUT_HISTORY_STREAMS {
            return Err(invalid_rate_brain_state(
                "frozen readout history stream count exceeds its limit",
            ));
        }
        for history in histories.values() {
            if history.len() > maximum
                || history.iter().any(|entry| {
                    entry.features.len() != dimension
                        || entry.features.iter().any(|value| !value.is_finite())
                        || !entry.timestamp_s.is_finite()
                })
            {
                return Err(invalid_rate_brain_state(
                    "frozen readout history has an invalid shape or value",
                ));
            }
        }
        Ok(())
    }
}

impl Model for FrozenReadoutModel {
    type Diagnostics = FrozenReadoutDiagnostics;
    type Update = FrozenReadoutUpdate;

    fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: habitua::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        self.evaluate_readout(stream_id, stream_id, features, at)
    }

    fn evaluate_input_with_id(
        &self,
        input_id: &str,
        stream_id: &str,
        input: ModelInput<'_>,
        at: habitua::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        match input {
            ModelInput::Features(features) => {
                self.evaluate_readout(input_id, stream_id, features, at)
            }
            ModelInput::Failure(_) => Err(ModelError::Incompatible(
                "frozen readout requires a feature-vector input".to_owned(),
            )),
        }
    }

    fn commit(&mut self, update: Self::Update, weight: f32) -> Result<(), ModelError> {
        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            return Err(ModelError::InvalidLearningWeight);
        }
        let effective_strength = update.strength * weight;
        if effective_strength > 0.0 && update.human_feedback_applied {
            if let Some(event_id) = update.human_feedback_event_id {
                let mut cases = self.cases.borrow_mut();
                cases.retain(|case| case.event_id != event_id);
                cases.push(FrozenReadoutCase {
                    features: update.augmented_features.clone(),
                    input_id: update.input_id.clone(),
                    kind: update.feedback_kind,
                    strength: effective_strength,
                    timestamp_s: update.feedback_timestamp_s.unwrap_or(update.timestamp_s),
                    received_at_s: update.feedback_received_at_s,
                    event_id,
                    revision: update.human_feedback_revision.unwrap_or(0),
                });
                let excess = cases
                    .len()
                    .saturating_sub(self.config.case_memory_max_cases);
                if excess > 0 {
                    cases.drain(..excess);
                }
            }
        } else if effective_strength > 0.0 && self.config.training_mode != TrainingMode::Frozen {
            let event_target = match update.feedback_kind {
                FeedbackKind::Reward => 1.0,
                FeedbackKind::Punish => 0.0,
            };
            let mut state = self.state.borrow_mut();
            let logit = state.bias
                + state
                    .weights
                    .iter()
                    .zip(&update.augmented_features)
                    .map(|(weight, feature)| weight * feature)
                    .sum::<f32>();
            let probability = sigmoid_f32(logit);
            let error = effective_strength * (probability - event_target);
            for (parameter, feature) in state.weights.iter_mut().zip(&update.augmented_features) {
                *parameter -= self.config.learning_rate
                    * (error * *feature + self.config.l2_regularization * *parameter);
            }
            state.bias -= self.config.learning_rate * error;
            state.update_count = state.update_count.saturating_add(1);
        }
        let mut histories = self.histories.borrow_mut();
        let history = histories.entry(update.stream_id).or_default();
        history.push(FrozenReadoutHistoryEntry {
            features: update.base_features,
            timestamp_s: update.timestamp_s,
        });
        while history.len() > self.config.recent_observations {
            history.remove(0);
        }
        Ok(())
    }

    fn identity(&self) -> ModelIdentity {
        ModelIdentity {
            model_version: FROZEN_READOUT_MODEL_VERSION,
            feature_schema_version: FROZEN_READOUT_SCHEMA_VERSION,
        }
    }

    fn fingerprint(&self) -> u64 {
        let mut words = vec![
            FROZEN_READOUT_MODEL_VERSION as u64,
            FROZEN_READOUT_SCHEMA_VERSION as u64,
            self.feature_dimension as u64,
            self.role as u64,
            self.config.recent_observations as u64,
            u64::from(self.config.distance_threshold.to_bits()),
            u64::from(self.config.previous_absolute_difference_scale.to_bits()),
            u64::from(self.config.history_elapsed_time_scale_seconds.to_bits()),
            u64::from(self.config.history_max_age_seconds.to_bits()),
            u64::from(self.config.l2_regularization.to_bits()),
            u64::from(self.config.learning_rate.to_bits()),
            u64::from(self.config.reaction_threshold.to_bits()),
            u64::from(self.config.case_memory_distance_threshold.to_bits()),
            u64::from(self.config.case_memory_time_constant_seconds.to_bits()),
            u64::from(self.config.case_memory_logit_scale.to_bits()),
            self.config.case_memory_max_cases as u64,
            self.config.training_mode as u64,
            self.seed,
        ];
        if let Some(standardization) = &self.standardization {
            words.push(1);
            words.extend(
                standardization
                    .mean
                    .iter()
                    .map(|value| u64::from(value.to_bits())),
            );
            words.extend(
                standardization
                    .scale
                    .iter()
                    .map(|value| u64::from(value.to_bits())),
            );
        } else {
            words.push(0);
        }
        if let Ok(events) = self.human_feedback.events.lock() {
            for value in events.keys() {
                words.push(habitua_pattern_hash(
                    value
                        .as_bytes()
                        .chunks(8)
                        .map(|chunk| {
                            let mut bytes = [0_u8; 8];
                            bytes[..chunk.len()].copy_from_slice(chunk);
                            u64::from_le_bytes(bytes)
                        })
                        .collect(),
                ));
            }
        }
        habitua_pattern_hash(words)
    }
}

impl PersistentModel for FrozenReadoutModel {
    fn state_format_version(&self) -> u16 {
        FROZEN_READOUT_STATE_VERSION
    }

    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()> {
        let histories = self.histories.borrow();
        Self::validate_histories(
            &histories,
            self.feature_dimension,
            self.config.recent_observations,
        )?;
        let snapshot = FrozenReadoutSnapshot {
            version: FROZEN_READOUT_STATE_VERSION,
            feature_dimension: self.feature_dimension,
            role: self.role,
            config: self.config.clone(),
            state: self.state.borrow().clone(),
            histories: histories.clone(),
            standardization: self.standardization.clone(),
            cases: self.cases.borrow().clone(),
        };
        serde_json::to_writer(writer, &snapshot)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn load_state(&mut self, reader: &mut dyn Read) -> io::Result<()> {
        let snapshot: FrozenReadoutSnapshot = serde_json::from_reader(reader)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if snapshot.version != FROZEN_READOUT_STATE_VERSION
            || snapshot.feature_dimension != self.feature_dimension
            || snapshot.role != self.role
            || snapshot.config != self.config
            || snapshot.standardization != self.standardization
        {
            return Err(invalid_rate_brain_state(
                "frozen readout state configuration does not match",
            ));
        }
        let expected_dimension = self
            .feature_dimension
            .checked_add(FROZEN_READOUT_HISTORY_FEATURES)
            .ok_or_else(|| invalid_rate_brain_state("frozen readout dimension overflowed"))?;
        if snapshot.state.weights.len() != expected_dimension
            || snapshot
                .state
                .weights
                .iter()
                .any(|value| !value.is_finite())
            || !snapshot.state.bias.is_finite()
        {
            return Err(invalid_rate_brain_state(
                "frozen readout state has an invalid dimension or value",
            ));
        }
        Self::validate_histories(
            &snapshot.histories,
            self.feature_dimension,
            self.config.recent_observations,
        )?;
        if snapshot.cases.len() > self.config.case_memory_max_cases
            || snapshot.cases.iter().any(|case| {
                case.features.len() != expected_dimension
                    || case.features.iter().any(|value| !value.is_finite())
                    || !case.strength.is_finite()
                    || !(0.0..=1.0).contains(&case.strength)
                    || !case.timestamp_s.is_finite()
                    || case.received_at_s.is_some_and(|value| !value.is_finite())
                    || case.event_id.is_empty()
            })
        {
            return Err(invalid_rate_brain_state(
                "frozen readout case memory has an invalid shape or value",
            ));
        }
        *self.state.borrow_mut() = snapshot.state;
        *self.histories.borrow_mut() = snapshot.histories;
        *self.cases.borrow_mut() = snapshot.cases;
        Ok(())
    }

    fn reserve_load_budget(&self, load_budget: &mut LoadBudget) -> io::Result<()> {
        let state = self.state.borrow();
        let mut count = state.weights.len().saturating_add(2);
        if let Some(standardization) = &self.standardization {
            count = count
                .saturating_add(standardization.mean.len())
                .saturating_add(standardization.scale.len());
        }
        for (stream, history) in self.histories.borrow().iter() {
            count = count
                .checked_add(stream.len())
                .and_then(|value| value.checked_add(history.len()))
                .and_then(|value| {
                    history.iter().try_fold(value, |total, entry| {
                        total
                            .checked_add(entry.features.len())
                            .and_then(|next| next.checked_add(1))
                    })
                })
                .ok_or_else(|| invalid_rate_brain_state("frozen readout budget overflowed"))?;
        }
        let cases = self.cases.borrow();
        count = count
            .checked_add(
                cases
                    .iter()
                    .try_fold(0_usize, |total, case| {
                        total
                            .checked_add(case.event_id.len())
                            .and_then(|next| next.checked_add(case.features.len()))
                            .and_then(|next| next.checked_add(2))
                    })
                    .ok_or_else(|| {
                        invalid_rate_brain_state("frozen readout case budget overflowed")
                    })?,
            )
            .ok_or_else(|| invalid_rate_brain_state("frozen readout case budget overflowed"))?;
        load_budget.reserve(count)
    }

    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()> {
        self.reserve_load_budget(load_budget)?;
        self.load_state(reader)
    }
}

/// Registers the frozen-response readout model for `BrainRunner`.
pub fn register_frozen_readout_model(
    registry: &mut habitua::ModelRegistry,
    feature_dimension: usize,
    seed_base: u64,
    human_feedback: FrozenReadoutFeedback,
) -> Result<(), habitua::RegistryError> {
    registry.register::<FrozenReadoutModel, _, _>(
        "frozen_readout",
        move |brain| {
            let settings = match &brain.model.settings {
                habitua::ModelSettings::Custom(settings) => settings,
                _ => {
                    return Err(habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: "frozen_readout requires custom settings".to_owned(),
                    });
                }
            };
            let role = parse_rate_brain_role(settings, brain)?;
            let recent_observations = parse_setting(settings, "recent_observations", brain)?;
            let distance_threshold = parse_setting_f32(settings, "distance_threshold", brain)?;
            let previous_absolute_difference_scale =
                parse_setting_f32(settings, "previous_absolute_difference_scale", brain)?;
            let history_elapsed_time_scale_seconds =
                parse_setting_f32(settings, "history_elapsed_time_scale_seconds", brain)?;
            let l2_regularization = parse_setting_f32(settings, "l2_regularization", brain)?;
            let learning_rate = parse_setting_f32(settings, "learning_rate", brain)?;
            let reaction_threshold = parse_setting_f32(settings, "reaction_threshold", brain)?;
            let training_mode = parse_training_mode(settings, brain)?;
            let case_memory_distance_threshold = settings
                .get("case_memory_distance_threshold")
                .map(|_| parse_setting_f32(settings, "case_memory_distance_threshold", brain))
                .transpose()?
                .unwrap_or_else(default_case_memory_distance_threshold);
            let case_memory_time_constant_seconds = settings
                .get("case_memory_time_constant_seconds")
                .map(|_| parse_setting_f32(settings, "case_memory_time_constant_seconds", brain))
                .transpose()?
                .unwrap_or_else(default_case_memory_time_constant_seconds);
            let case_memory_logit_scale = settings
                .get("case_memory_logit_scale")
                .map(|_| parse_setting_f32(settings, "case_memory_logit_scale", brain))
                .transpose()?
                .unwrap_or_else(default_case_memory_logit_scale);
            let case_memory_max_cases = settings
                .get("case_memory_max_cases")
                .map(|_| parse_setting(settings, "case_memory_max_cases", brain))
                .transpose()?
                .unwrap_or_else(default_case_memory_max_cases);
            let configured_dimension = parse_setting(settings, "feature_dimension", brain)?;
            if configured_dimension != feature_dimension {
                return Err(habitua::RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "frozen_readout feature dimension does not match runner".to_owned(),
                });
            }
            let seed = settings
                .get("seed")
                .ok_or_else(|| habitua::RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "frozen_readout seed is missing".to_owned(),
                })?
                .parse::<u64>()
                .map_err(|_| habitua::RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "frozen_readout seed is invalid".to_owned(),
                })?
                .wrapping_add(seed_base);
            let mut model = FrozenReadoutModel::with_feedback_handle(
                feature_dimension,
                seed,
                role,
                FrozenReadoutConfig {
                    recent_observations,
                    distance_threshold,
                    previous_absolute_difference_scale,
                    history_elapsed_time_scale_seconds,
                    history_max_age_seconds: parse_setting_f32(
                        settings,
                        "history_max_age_seconds",
                        brain,
                    )?,
                    l2_regularization,
                    learning_rate,
                    reaction_threshold,
                    training_mode,
                    case_memory_distance_threshold,
                    case_memory_time_constant_seconds,
                    case_memory_logit_scale,
                    case_memory_max_cases,
                },
                human_feedback.clone(),
            )
            .map_err(|error| habitua::RegistryError::Model {
                brain: brain.id.clone(),
                reason: error.to_string(),
            })?;
            if let Some(mean) = settings.get("standardization_mean") {
                let mean = parse_setting_vector(mean, "standardization_mean", brain)?;
                let scale = parse_setting_vector(
                    settings.get("standardization_scale").ok_or_else(|| {
                        habitua::RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: "readout setting standardization_scale is missing".to_owned(),
                        }
                    })?,
                    "standardization_scale",
                    brain,
                )?;
                model.set_standardization(mean, scale).map_err(|error| {
                    habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: error.to_string(),
                    }
                })?;
            }
            if let Some(weights) = settings.get("initial_weights") {
                let weights = parse_setting_vector(weights, "initial_weights", brain)?;
                let bias = settings
                    .get("initial_bias")
                    .ok_or_else(|| habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: "readout setting initial_bias is missing".to_owned(),
                    })?
                    .parse::<f32>()
                    .map_err(|_| habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: "readout setting initial_bias is invalid".to_owned(),
                    })?;
                model
                    .set_fitted_parameters(weights, bias)
                    .map_err(|error| habitua::RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: error.to_string(),
                    })?;
            }
            Ok(model)
        },
        frozen_readout_report,
    )
}

fn frozen_readout_report(
    evaluation: &Evaluation<FrozenReadoutDiagnostics, FrozenReadoutUpdate>,
    config: &habitua::BrainConfig,
) -> ModelReport {
    let diagnostics = &evaluation.diagnostics;
    let mut metrics = vec![
        Metric::new(
            "target_probability",
            diagnostics.target_probability,
            "probability",
            MetricDirection::HigherIsMore,
        ),
        Metric::new(
            "automatic_change",
            f32::from(u8::from(diagnostics.automatic_change.unwrap_or(false))),
            "boolean",
            MetricDirection::HigherIsMore,
        ),
        Metric::new(
            "repeat_count",
            diagnostics.repeat_count as f32,
            "count",
            MetricDirection::Informational,
        ),
        Metric::new(
            "human_feedback_applied",
            f32::from(u8::from(diagnostics.human_feedback_applied)),
            "boolean",
            MetricDirection::Informational,
        ),
        Metric::new(
            "feedback_reversed",
            f32::from(u8::from(
                evaluation.readiness == Readiness::Evaluated
                    && diagnostics.human_feedback_applied
                    && diagnostics.selected_feedback
                        != automatic_feedback_for_role(
                            config.role.as_str(),
                            diagnostics.automatic_change,
                        ),
            )),
            "boolean",
            MetricDirection::Informational,
        ),
        Metric::new(
            "readout_update_count",
            diagnostics.update_count as f32,
            "count",
            MetricDirection::Informational,
        ),
        Metric::new(
            "base_probability",
            diagnostics.base_probability,
            "probability",
            MetricDirection::Informational,
        ),
        Metric::new(
            "case_memory_adjustment",
            diagnostics.case_memory_adjustment,
            "logit",
            MetricDirection::Informational,
        ),
        Metric::new(
            "case_memory_match_count",
            diagnostics.case_memory_match_count as f32,
            "count",
            MetricDirection::Informational,
        ),
    ];
    if let Some(distance) = diagnostics.feature_distance {
        metrics.push(Metric::new(
            "feature_distance",
            distance,
            "normalized-rms",
            MetricDirection::HigherIsMore,
        ));
    }
    if let Some(previous_absolute_difference) = diagnostics.previous_absolute_difference {
        metrics.push(Metric::new(
            "previous_absolute_difference",
            previous_absolute_difference,
            "mean-absolute-difference",
            MetricDirection::HigherIsMore,
        ));
    }
    if let Some(elapsed_seconds) = diagnostics.elapsed_seconds {
        metrics.push(Metric::new(
            "elapsed_seconds",
            elapsed_seconds,
            "seconds",
            MetricDirection::HigherIsMore,
        ));
    }
    if let Some(distance) = diagnostics.case_memory_nearest_distance {
        metrics.push(Metric::new(
            "case_memory_nearest_distance",
            distance,
            "l2",
            MetricDirection::LowerIsMore,
        ));
    }
    let evidence = vec![Evidence {
        name: "frozen-response-history".to_owned(),
        current: diagnostics.feature_distance,
        baseline: None,
        sample_count: Some(diagnostics.repeat_count),
        window: None,
        reference_ids: Vec::new(),
        detail: format!(
            "{} role; human overlay={}; reaction uses learned probability",
            config.role, diagnostics.human_feedback_applied
        ),
    }];
    let positive = diagnostics.target_probability >= config.reactions.threshold;
    let readiness = evaluation.readiness.clone();
    let mut report = ModelReport {
        novelty_short: evaluation.novelty_short,
        novelty_long: None,
        familiarity_short: evaluation.familiarity_short,
        familiarity_long: None,
        recency: None,
        pattern_id: None,
        readiness: readiness.clone(),
        learning_signal: if readiness == Readiness::Evaluated {
            if positive {
                LearningSignal::Abnormal
            } else {
                LearningSignal::Normal
            }
        } else {
            LearningSignal::Unknown
        },
        metrics,
        evidence,
        reactions: Vec::new(),
    };
    let reaction_kind = match config.role.as_str() {
        "change" => ReactionKind::Novel,
        "no_change" => ReactionKind::Repeat,
        _ => ReactionKind::Deviation,
    };
    if readiness == Readiness::Evaluated
        && diagnostics.target_probability >= config.reactions.threshold
        && config.reactions.kinds.contains(&reaction_kind)
    {
        report.reactions.push(Reaction {
            brain: config.id.clone(),
            target: config.input.target.clone(),
            kind: reaction_kind,
            strength: Some(diagnostics.target_probability),
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            readiness,
        });
    }
    report
}

fn automatic_feedback_for_role(role: &str, automatic_change: Option<bool>) -> FeedbackKind {
    let changed = automatic_change.unwrap_or(false);
    match role {
        "no_change" => {
            if changed {
                FeedbackKind::Punish
            } else {
                FeedbackKind::Reward
            }
        }
        _ => {
            if changed {
                FeedbackKind::Reward
            } else {
                FeedbackKind::Punish
            }
        }
    }
}

fn parse_rate_brain_role(
    settings: &BTreeMap<String, String>,
    brain: &habitua::BrainConfig,
) -> Result<RateBrainRole, habitua::RegistryError> {
    match settings.get("role").map(String::as_str) {
        Some("change") => Ok(RateBrainRole::Change),
        Some("no_change") => Ok(RateBrainRole::NoChange),
        _ => Err(habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: "readout role must be change or no_change".to_owned(),
        }),
    }
}

fn parse_training_mode(
    settings: &BTreeMap<String, String>,
    brain: &habitua::BrainConfig,
) -> Result<TrainingMode, habitua::RegistryError> {
    match settings.get("training_mode").map(String::as_str) {
        Some("frozen") => Ok(TrainingMode::Frozen),
        Some("readout_only") => Ok(TrainingMode::ReadoutOnly),
        Some("full") => Ok(TrainingMode::Full),
        _ => Err(habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: "readout training_mode is invalid".to_owned(),
        }),
    }
}

fn parse_setting_f32(
    settings: &BTreeMap<String, String>,
    name: &str,
    brain: &habitua::BrainConfig,
) -> Result<f32, habitua::RegistryError> {
    settings
        .get(name)
        .ok_or_else(|| habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("readout setting {name} is missing"),
        })?
        .parse::<f32>()
        .map_err(|_| habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("readout setting {name} is invalid"),
        })
}

fn parse_setting_vector(
    value: &str,
    name: &str,
    brain: &habitua::BrainConfig,
) -> Result<Vec<f32>, habitua::RegistryError> {
    if value.is_empty() {
        return Err(habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("readout setting {name} is empty"),
        });
    }
    value
        .split(',')
        .map(|item| item.parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("readout setting {name} is invalid"),
        })
}

fn normalized_rms_distance_f32(left: &[f32], right: &[f32]) -> f32 {
    let squared = left
        .iter()
        .zip(right)
        .map(|(left, right)| {
            let difference = *left - *right;
            difference * difference
        })
        .sum::<f32>();
    (squared / left.len().max(1) as f32).sqrt()
}

fn l2_distance_f32(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return f32::INFINITY;
    }
    left.iter()
        .zip(right)
        .map(|(left, right)| {
            let difference = *left - *right;
            difference * difference
        })
        .sum::<f32>()
        .sqrt()
}

fn logit_from_probability(probability: f32) -> f32 {
    let probability = probability.clamp(f32::EPSILON, 1.0 - f32::EPSILON);
    (probability / (1.0 - probability)).ln()
}

fn sigmoid_f32(value: f32) -> f32 {
    if value >= 0.0 {
        let exponent = (-value).exp();
        1.0 / (1.0 + exponent)
    } else {
        let exponent = value.exp();
        exponent / (1.0 + exponent)
    }
}

fn mean_absolute_difference(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (*left - *right).abs())
        .sum::<f32>()
        / left.len().max(1) as f32
}

fn parse_setting(
    settings: &BTreeMap<String, String>,
    name: &str,
    brain: &habitua::BrainConfig,
) -> Result<usize, habitua::RegistryError> {
    settings
        .get(name)
        .ok_or_else(|| habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("rate_brain setting {name} is missing"),
        })?
        .parse::<usize>()
        .map_err(|_| habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("rate_brain setting {name} is invalid"),
        })
}

fn parse_setting_f64(
    settings: &BTreeMap<String, String>,
    name: &str,
    brain: &habitua::BrainConfig,
) -> Result<f64, habitua::RegistryError> {
    settings
        .get(name)
        .ok_or_else(|| habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("rate_brain setting {name} is missing"),
        })?
        .parse::<f64>()
        .map_err(|_| habitua::RegistryError::Model {
            brain: brain.id.clone(),
            reason: format!("rate_brain setting {name} is invalid"),
        })
}

fn normalized_rms_distance(left: &[f32], right: &[f32]) -> f64 {
    let squared = left
        .iter()
        .zip(right)
        .map(|(left, right)| {
            let difference = f64::from(*left) - f64::from(*right);
            difference * difference
        })
        .sum::<f64>();
    (squared / left.len().max(1) as f64).sqrt()
}

fn readout_structure_matches(left: &RateReadout, right: &RateReadout) -> bool {
    left.neuron_indices == right.neuron_indices
        && left.mean == right.mean
        && left.std == right.std
        && left.std_floor == right.std_floor
        && left.clip == right.clip
        && left.projection.len() == right.projection.len()
        && left.output_weights.len() == right.output_weights.len()
}

fn rate_model_error(error: RateModelError) -> ModelError {
    ModelError::Incompatible(error.to_string())
}

fn rate_model_io_error(error: RateModelError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn invalid_rate_brain_state(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn habitua_pattern_hash(words: Vec<u64>) -> u64 {
    words.into_iter().fold(0xcbf29ce484222325, |hash, word| {
        hash.wrapping_mul(0x100000001b3).wrapping_add(word)
    })
}

impl RateLearningState {
    /// Creates a state with nfly-compatible recurrent initial values.
    pub fn new(
        graph: &RateGraph,
        readout_indices: Vec<u32>,
        projection_dimension: usize,
        seed: u64,
        config: RateLearningConfig,
    ) -> Result<Self, RateModelError> {
        config.validate()?;
        let parameters = RateParameters::initial(graph);
        let readout = RateReadout::new(readout_indices, projection_dimension, seed)?;
        readout.validate(graph.neuron_count())?;
        Ok(Self {
            initial_parameters: parameters.clone(),
            optimizer: AdamState::new(&parameters, &readout),
            parameters,
            readout,
            config,
            graph_fingerprint: graph.fingerprint(),
            validated_graph_address: Some(std::ptr::from_ref(graph) as usize),
            rest_activity: None,
            rest_optimizer_step: None,
            rest_convergence_max_abs: None,
        })
    }

    /// Returns the current learning configuration.
    pub fn config(&self) -> &RateLearningConfig {
        &self.config
    }

    /// Returns the optimizer update count.
    pub fn optimizer_step(&self) -> u64 {
        self.optimizer.step
    }

    /// Predicts a value without changing learning or habituation state.
    pub fn predict(
        &mut self,
        graph: &RateGraph,
        input: &[f64],
    ) -> Result<(RateForwardResult, ReadoutForward), RateModelError> {
        self.validate_graph(graph)?;
        let rest = self.rest_state(graph)?.0;
        let forward = forward_with_parameters(
            graph,
            &self.parameters,
            input,
            self.config.steps_per_observation,
            Some(&rest),
        )?;
        let readout = self.readout.forward(forward.final_activity())?;
        Ok((forward, readout))
    }

    /// Predicts an observation without allocating a reverse-propagation
    /// history. The full [`Self::predict`] path remains available for learning.
    pub fn predict_final(
        &mut self,
        graph: &RateGraph,
        input: &[f64],
    ) -> Result<(Vec<f64>, ReadoutForward), RateModelError> {
        self.validate_graph(graph)?;
        let _ = self.rest_state(graph)?;
        let rest = self
            .rest_activity
            .as_deref()
            .ok_or_else(|| RateModelError::Invalid("rate rest state was not built".to_owned()))?;
        let activity = final_activity_with_parameters(
            graph,
            &self.parameters,
            input,
            self.config.steps_per_observation,
            Some(rest),
        )?;
        let readout = self.readout.forward(&activity)?;
        Ok((activity, readout))
    }

    /// Returns the no-input equilibrium used as the initial state of an observation.
    pub fn rest_activity(&mut self, graph: &RateGraph) -> Result<Vec<f64>, RateModelError> {
        Ok(self.rest_state(graph)?.0)
    }

    /// Returns the rest state and its maximum absolute change on the last update.
    pub fn rest_state(&mut self, graph: &RateGraph) -> Result<(Vec<f64>, f64), RateModelError> {
        self.validate_graph(graph)?;
        if self.rest_optimizer_step == Some(self.optimizer.step)
            && let Some(rest) = &self.rest_activity
        {
            return Ok((
                rest.clone(),
                self.rest_convergence_max_abs.unwrap_or(f64::NAN),
            ));
        }
        let zero_input = vec![0.0; graph.neuron_count()];
        let rest_forward =
            forward_with_parameters(graph, &self.parameters, &zero_input, REST_STEPS, None)?;
        let rest = rest_forward.final_activity().to_vec();
        let convergence_max_abs = rest_forward
            .activity_history
            .get(rest_forward.activity_history.len().saturating_sub(2))
            .map(|previous| {
                previous
                    .iter()
                    .zip(&rest)
                    .map(|(before, after)| (after - before).abs())
                    .fold(0.0, f64::max)
            })
            .unwrap_or(f64::NAN);
        self.rest_activity = Some(rest.clone());
        self.rest_optimizer_step = Some(self.optimizer.step);
        self.rest_convergence_max_abs = Some(convergence_max_abs);
        Ok((rest, convergence_max_abs))
    }

    /// Returns the fixed number of no-input updates used for the rest state.
    pub const fn rest_steps() -> usize {
        REST_STEPS
    }

    /// Applies one reward or punish label according to the selected training mode.
    pub fn train_observation(
        &mut self,
        graph: &RateGraph,
        input: &[f64],
        event: &FeedbackEvent,
        mode: TrainingMode,
    ) -> Result<TrainingReport, RateModelError> {
        self.validate_graph(graph)?;
        let (forward, readout_forward) = self.predict(graph, input)?;
        let (target, strength) = event.target();
        let probability = readout_forward.probability;
        let loss = strength * binary_cross_entropy_from_logit(readout_forward.logit, target);
        if strength == 0.0 || mode == TrainingMode::Frozen {
            return Ok(TrainingReport {
                input_id: event.input_id.clone(),
                event_id: event.event_id.clone(),
                target,
                strength,
                probability_before: probability,
                loss,
                gradient_norm_before_clip: 0.0,
                gradient_was_clipped: false,
                gradient_nonzero_log_gain_fraction: 0.0,
                updated: false,
                optimizer_step: self.optimizer.step,
            });
        }
        let logit_gradient = strength * (probability - target);
        let readout_gradients =
            self.readout
                .backward(forward.final_activity(), &readout_forward, logit_gradient)?;
        let circuit_gradients = RateBackward::backward_with_parameters(
            graph,
            &self.parameters,
            &forward,
            &readout_gradients.activity,
        )?;
        let mut circuit_gradients = circuit_gradients;
        let gradient_nonzero_log_gain_fraction = circuit_gradients
            .log_gain
            .iter()
            .filter(|value| value.abs() > 1.0e-12)
            .count() as f64
            / circuit_gradients.log_gain.len().max(1) as f64;
        add_prior_gradients(
            &mut circuit_gradients,
            &self.parameters,
            &self.initial_parameters,
            &self.config,
        );
        let norm = gradient_norm(&circuit_gradients, &readout_gradients);
        let scale = (self.config.gradient_norm_limit / norm).min(1.0);
        let clipped = scale < 1.0;
        self.optimizer.step = self.optimizer.step.saturating_add(1);
        if mode == TrainingMode::Full {
            self.apply_circuit_gradients(&circuit_gradients, scale);
        }
        self.apply_readout_gradients(&readout_gradients, scale);
        Ok(TrainingReport {
            input_id: event.input_id.clone(),
            event_id: event.event_id.clone(),
            target,
            strength,
            probability_before: probability,
            loss,
            gradient_norm_before_clip: norm,
            gradient_was_clipped: clipped,
            gradient_nonzero_log_gain_fraction,
            updated: true,
            optimizer_step: self.optimizer.step,
        })
    }

    /// Saves a checkpoint whose graph fingerprint is checked on restore.
    pub fn save_checkpoint(&self, path: impl AsRef<Path>) -> Result<(), RateModelError> {
        let checkpoint = RateCheckpoint {
            version: CHECKPOINT_VERSION,
            state: self.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&checkpoint)?;
        fs::write(path, bytes)?;
        Ok(())
    }

    /// Loads a checkpoint and rejects a different graph.
    pub fn load_checkpoint(
        path: impl AsRef<Path>,
        graph: &RateGraph,
    ) -> Result<Self, RateModelError> {
        let checkpoint: RateCheckpoint = serde_json::from_slice(&fs::read(path)?)?;
        if checkpoint.version != CHECKPOINT_VERSION {
            return Err(RateModelError::Invalid(format!(
                "unsupported rate checkpoint version {}",
                checkpoint.version
            )));
        }
        let mut state = checkpoint.state;
        state.validate_graph(graph)?;
        Ok(state)
    }

    fn validate_graph(&mut self, graph: &RateGraph) -> Result<(), RateModelError> {
        let graph_address = std::ptr::from_ref(graph) as usize;
        if self.validated_graph_address != Some(graph_address)
            && self.graph_fingerprint != graph.fingerprint()
        {
            return Err(RateModelError::Invalid(
                "rate checkpoint graph fingerprint does not match".to_owned(),
            ));
        }
        self.validated_graph_address = Some(graph_address);
        self.config.validate()?;
        self.readout.validate(graph.neuron_count())?;
        self.parameters.validate(graph)?;
        Ok(())
    }

    fn apply_circuit_gradients(&mut self, gradients: &RateGradients, scale: f64) {
        let step = self.optimizer.step;
        for index in 0..self.parameters.log_gain.len() {
            adam_update_indexed(
                &mut self.parameters.log_gain[index],
                gradients.log_gain[index] * scale,
                &mut self.optimizer.log_gain,
                index,
                self.config.circuit_learning_rate,
                &self.config,
                step,
            );
        }
        for index in 0..self.parameters.bias.len() {
            adam_update_indexed(
                &mut self.parameters.bias[index],
                gradients.bias[index] * scale,
                &mut self.optimizer.bias,
                index,
                self.config.circuit_learning_rate,
                &self.config,
                step,
            );
            adam_update_indexed(
                &mut self.parameters.alpha_logits[index],
                gradients.alpha_logits[index] * scale,
                &mut self.optimizer.alpha_logits,
                index,
                self.config.circuit_learning_rate,
                &self.config,
                step,
            );
        }
        for parameter in &mut self.parameters.log_gain {
            *parameter = parameter.clamp(self.config.gain_min.ln(), self.config.gain_max.ln());
        }
        let alpha_min = logit(self.config.alpha_min);
        let alpha_max = logit(self.config.alpha_max);
        for parameter in &mut self.parameters.alpha_logits {
            *parameter = parameter.clamp(alpha_min, alpha_max);
        }
        self.rest_activity = None;
        self.rest_optimizer_step = None;
        self.rest_convergence_max_abs = None;
    }

    fn apply_readout_gradients(&mut self, gradients: &ReadoutGradients, scale: f64) {
        let step = self.optimizer.step;
        for index in 0..self.readout.projection.len() {
            adam_update_indexed(
                &mut self.readout.projection[index],
                gradients.projection[index] * scale,
                &mut self.optimizer.projection,
                index,
                self.config.readout_learning_rate,
                &self.config,
                step,
            );
        }
        for index in 0..self.readout.output_weights.len() {
            adam_update_indexed(
                &mut self.readout.output_weights[index],
                gradients.output_weights[index] * scale,
                &mut self.optimizer.output_weights,
                index,
                self.config.readout_learning_rate,
                &self.config,
                step,
            );
        }
        adam_scalar_update(
            &mut self.readout.output_bias,
            gradients.output_bias * scale,
            &mut self.optimizer.output_bias_first,
            &mut self.optimizer.output_bias_second,
            self.config.readout_learning_rate,
            &self.config,
            step,
        );
    }
}

fn adam_update_indexed(
    parameter: &mut f64,
    gradient: f64,
    state: &mut AdamVector,
    index: usize,
    learning_rate: f64,
    config: &RateLearningConfig,
    step: u64,
) {
    state.first[index] = config.beta1 * state.first[index] + (1.0 - config.beta1) * gradient;
    state.second[index] =
        config.beta2 * state.second[index] + (1.0 - config.beta2) * gradient * gradient;
    let corrected_first = state.first[index] / (1.0 - config.beta1.powf(step as f64));
    let corrected_second = state.second[index] / (1.0 - config.beta2.powf(step as f64));
    *parameter -= learning_rate * corrected_first / (corrected_second.sqrt() + config.epsilon);
}

fn adam_scalar_update(
    parameter: &mut f64,
    gradient: f64,
    first: &mut f64,
    second: &mut f64,
    learning_rate: f64,
    config: &RateLearningConfig,
    step: u64,
) {
    *first = config.beta1 * *first + (1.0 - config.beta1) * gradient;
    *second = config.beta2 * *second + (1.0 - config.beta2) * gradient * gradient;
    let corrected_first = *first / (1.0 - config.beta1.powf(step as f64));
    let corrected_second = *second / (1.0 - config.beta2.powf(step as f64));
    *parameter -= learning_rate * corrected_first / (corrected_second.sqrt() + config.epsilon);
}

fn add_prior_gradients(
    gradients: &mut RateGradients,
    parameters: &RateParameters,
    initial: &RateParameters,
    config: &RateLearningConfig,
) {
    let edge_scale = config.prior_gain / parameters.log_gain.len().max(1) as f64;
    for ((gradient, parameter), initial) in gradients
        .log_gain
        .iter_mut()
        .zip(&parameters.log_gain)
        .zip(&initial.log_gain)
    {
        *gradient += edge_scale * (parameter - initial);
    }
    let neuron_scale = 1.0 / parameters.bias.len().max(1) as f64;
    for index in 0..parameters.bias.len() {
        gradients.bias[index] +=
            config.prior_bias * neuron_scale * (parameters.bias[index] - initial.bias[index]);
        gradients.alpha_logits[index] += config.prior_alpha
            * neuron_scale
            * (parameters.alpha_logits[index] - initial.alpha_logits[index]);
    }
}

fn gradient_norm(circuit: &RateGradients, readout: &ReadoutGradients) -> f64 {
    let sum = circuit
        .log_gain
        .iter()
        .chain(&circuit.bias)
        .chain(&circuit.alpha_logits)
        .chain(&circuit.input)
        .chain(&circuit.initial_activity)
        .chain(&readout.projection)
        .chain(&readout.output_weights)
        .chain(std::iter::once(&readout.output_bias))
        .map(|value| value * value)
        .sum::<f64>();
    sum.sqrt()
}

fn binary_cross_entropy_from_logit(logit_value: f64, target: f64) -> f64 {
    (0.0_f64).max(logit_value) - logit_value * target + (1.0 + (-logit_value.abs()).exp()).ln()
}

fn sigmoid(value: f64) -> f64 {
    if value >= 0.0 {
        let e = (-value).exp();
        1.0 / (1.0 + e)
    } else {
        let e = value.exp();
        e / (1.0 + e)
    }
}

fn logit(value: f64) -> f64 {
    (value / (1.0 - value)).ln()
}

fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}

fn next_unit(state: &mut u64) -> f64 {
    (next_random(state) as f64) / (u64::MAX as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate::{RateIncomingEdge, RateNeuronMetadata, RateOutgoingEdge};

    fn graph() -> RateGraph {
        let metadata = (0..3)
            .map(|body_id| RateNeuronMetadata {
                body_id,
                type_name: Some(format!("T{body_id}")),
                class: None,
                superclass: None,
                subclass: None,
                soma_side: None,
                root_side: None,
                assigned_ol_hex1: None,
                assigned_ol_hex2: None,
                selected_neurotransmitter: None,
                neurotransmitter_source: "test".to_owned(),
                source_sign: if body_id == 1 { -1 } else { 1 },
            })
            .collect();
        RateGraph::new(
            vec![0, 1, 2],
            vec![0, 2, 3, 4],
            vec![
                RateOutgoingEdge {
                    target: 1,
                    contact_count: 2,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 1,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 3,
                },
                RateOutgoingEdge {
                    target: 0,
                    contact_count: 1,
                },
            ],
            vec![0, 1, 2, 4],
            vec![
                RateIncomingEdge {
                    source: 2,
                    edge_id: 3,
                },
                RateIncomingEdge {
                    source: 0,
                    edge_id: 0,
                },
                RateIncomingEdge {
                    source: 0,
                    edge_id: 1,
                },
                RateIncomingEdge {
                    source: 1,
                    edge_id: 2,
                },
            ],
            vec![1, -1, 1],
            metadata,
            BTreeMap::new(),
        )
        .expect("test graph should be valid")
    }

    #[test]
    fn reward_and_punish_are_bce_targets_and_duplicate_delivery_is_ignored() {
        let mut store = FeedbackStore::default();
        assert!(store.reward("e1", "a", 1.0, 0.0).expect("reward"));
        assert!(!store.reward("e1", "a", 1.0, 0.0).expect("duplicate"));
        let correction = FeedbackEvent::new("e1", "a", FeedbackKind::Punish, 0.5, 2.0)
            .expect("correction")
            .with_revision(1);
        assert!(store.insert(correction).expect("correction insert"));
        let same_revision = FeedbackEvent::new("e1", "a", FeedbackKind::Reward, 1.0, 3.0)
            .expect("same revision resubmission")
            .with_revision(1);
        assert!(!store.insert(same_revision).expect("same revision"));
        assert_eq!(
            store.get("e1").expect("corrected event").kind,
            FeedbackKind::Punish
        );
        let event = store.get("e1").expect("event");
        assert_eq!(event.target(), (0.0, 0.5));
        let punish =
            FeedbackEvent::new("e2", "b", FeedbackKind::Punish, 0.25, 1.0).expect("punish");
        assert_eq!(punish.target(), (0.0, 0.25));
    }

    #[test]
    fn readout_backward_matches_a_finite_difference() {
        let readout = RateReadout::new(vec![0, 2], 2, 7).expect("readout");
        let activity = vec![0.4, 0.2, 0.6];
        let forward = readout.forward(&activity).expect("forward");
        let gradients = readout
            .backward(&activity, &forward, 1.0)
            .expect("backward");
        let epsilon = 1.0e-6;
        let mut plus = readout.clone();
        plus.output_bias += epsilon;
        let mut minus = readout;
        minus.output_bias -= epsilon;
        let finite_difference = (plus.forward(&activity).expect("plus").logit
            - minus.forward(&activity).expect("minus").logit)
            / (2.0 * epsilon);
        assert!((gradients.output_bias - finite_difference).abs() < 1.0e-8);
    }

    #[test]
    fn frozen_does_not_update_and_full_learning_can_checkpoint_and_resume() {
        let graph = graph();
        let indices = vec![0, 2];
        let mut state = RateLearningState::new(
            &graph,
            indices,
            2,
            9,
            RateLearningConfig {
                steps_per_observation: 2,
                ..RateLearningConfig::default()
            },
        )
        .expect("state");
        let event =
            FeedbackEvent::new("e", "input", FeedbackKind::Reward, 1.0, 0.0).expect("event");
        let before = state.clone();
        let frozen = state
            .train_observation(&graph, &[0.2, 0.0, 0.1], &event, TrainingMode::Frozen)
            .expect("frozen");
        assert!(!frozen.updated);
        assert_eq!(state.parameters, before.parameters);
        assert_eq!(state.readout, before.readout);
        assert_eq!(state.optimizer_step(), before.optimizer_step());
        let trained = state
            .train_observation(&graph, &[0.2, 0.0, 0.1], &event, TrainingMode::Full)
            .expect("train");
        assert!(trained.updated);
        assert_eq!(state.optimizer_step(), 1);
        let path = std::env::current_dir()
            .expect("current directory")
            .join("target")
            .join(format!(
                "habitua-rate-checkpoint-{}-{}.json",
                std::process::id(),
                state.optimizer_step()
            ));
        fs::create_dir_all(path.parent().expect("checkpoint parent")).expect("checkpoint dir");
        state.save_checkpoint(&path).expect("save");
        let restored = RateLearningState::load_checkpoint(&path, &graph).expect("load");
        assert_eq!(restored.optimizer_step(), state.optimizer_step());
        assert_eq!(
            restored.parameters.log_gain.len(),
            state.parameters.log_gain.len()
        );
        for (actual, expected) in restored
            .parameters
            .log_gain
            .iter()
            .zip(&state.parameters.log_gain)
        {
            assert!((actual - expected).abs() < 1.0e-14);
        }
        fs::remove_file(path).expect("remove");
    }

    #[test]
    fn rate_brain_automatic_food_uses_recent_history_and_human_overlay() {
        let graph = Arc::new(graph());
        let mut human_feedback = BTreeMap::new();
        human_feedback.insert(
            "change:human".to_owned(),
            FeedbackEvent::new("human-event", "human", FeedbackKind::Reward, 0.5, 0.0)
                .expect("human event"),
        );
        let mut model = RateBrainModel::new(
            Arc::clone(&graph),
            vec![0, 2],
            2,
            11,
            RateBrainRole::Change,
            RateBrainConfig {
                recent_observations: 2,
                distance_threshold: 0.01,
                training_mode: TrainingMode::Frozen,
            },
            RateLearningConfig {
                steps_per_observation: 2,
                ..RateLearningConfig::default()
            },
            human_feedback,
        )
        .expect("rate brain");
        let first = model
            .evaluate_input_with_id(
                "first",
                "stream",
                ModelInput::Features(&[0.2, 0.0, 0.1]),
                habitua::Timestamp::ZERO,
            )
            .expect("first evaluation");
        assert_eq!(first.readiness, Readiness::BaselineInsufficient);
        model.commit(first.update, 0.0).expect("first commit");
        let repeated = model
            .evaluate_input_with_id(
                "repeated",
                "stream",
                ModelInput::Features(&[0.2, 0.0, 0.1]),
                habitua::Timestamp::from_secs(1),
            )
            .expect("repeated evaluation");
        assert_eq!(repeated.readiness, Readiness::Evaluated);
        assert_eq!(repeated.diagnostics.automatic_change, Some(false));
        model.commit(repeated.update, 0.0).expect("repeated commit");
        let changed = model
            .evaluate_input_with_id(
                "changed",
                "stream",
                ModelInput::Features(&[0.4, 0.0, 0.1]),
                habitua::Timestamp::from_secs(2),
            )
            .expect("changed evaluation");
        assert_eq!(changed.diagnostics.automatic_change, Some(true));
        let human = model
            .evaluate_input_with_id(
                "human",
                "stream",
                ModelInput::Features(&[0.4, 0.0, 0.1]),
                habitua::Timestamp::from_secs(3),
            )
            .expect("human evaluation");
        assert!(human.diagnostics.human_feedback_applied);
        assert_eq!(human.diagnostics.selected_feedback, FeedbackKind::Reward);
        assert!((human.diagnostics.selected_strength - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn frozen_readout_reaction_uses_learned_probability_after_automatic_label_changes() {
        let mut model = FrozenReadoutModel::new(
            1,
            17,
            RateBrainRole::Change,
            FrozenReadoutConfig {
                recent_observations: 4,
                distance_threshold: 0.01,
                previous_absolute_difference_scale: 1.0,
                history_elapsed_time_scale_seconds: 1.0,
                history_max_age_seconds: 300.0,
                l2_regularization: 0.0,
                learning_rate: 10.0,
                reaction_threshold: 0.5,
                training_mode: TrainingMode::ReadoutOnly,
                case_memory_distance_threshold: 4.0,
                case_memory_time_constant_seconds: 600.0,
                case_memory_logit_scale: 4.0,
                case_memory_max_cases: 256,
            },
            BTreeMap::new(),
        )
        .expect("frozen readout");
        let first = model
            .evaluate_input_with_id(
                "first",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::ZERO,
            )
            .expect("first evaluation");
        model.commit(first.update, 0.0).expect("first commit");
        let changed = model
            .evaluate_input_with_id(
                "changed",
                "stream",
                ModelInput::Features(&[1.0]),
                habitua::Timestamp::from_secs(1),
            )
            .expect("changed evaluation");
        assert_eq!(changed.diagnostics.automatic_change, Some(true));
        model.commit(changed.update, 1.0).expect("learned commit");
        let repeated = model
            .evaluate_input_with_id(
                "repeated",
                "stream",
                ModelInput::Features(&[1.0]),
                habitua::Timestamp::from_secs(2),
            )
            .expect("repeated evaluation");
        assert_eq!(repeated.diagnostics.automatic_change, Some(false));
        assert!(repeated.diagnostics.target_probability > 0.5);

        let config = habitua::BrainConfig {
            id: "brain-change".to_owned(),
            role: "change".to_owned(),
            enabled: true,
            mode: habitua::BrainMode::Active,
            input: habitua::BrainInputSpec {
                schema_id: "test".to_owned(),
                schema_version: 1,
                target: "screen".to_owned(),
                context: "test".to_owned(),
            },
            model: habitua::ModelSpec {
                kind: "frozen_readout".to_owned(),
                version: 1,
                seed: Some(17),
                settings: habitua::ModelSettings::Custom(BTreeMap::new()),
            },
            learning: habitua::BrainLearningPolicy {
                mode: habitua::LearningMode::Manual,
                weight: 1.0,
                require_evaluated: false,
                bootstrap: false,
                learn_on_reaction: false,
                frequency_limit: None,
            },
            reactions: habitua::ReactionPolicy {
                threshold: 0.5,
                kinds: vec![ReactionKind::Novel],
                calibration: None,
            },
            resources: habitua::ResourcePolicy {
                max_streams: 1,
                max_samples: 8,
                max_memory_bytes: 1_024,
                evaluation_deadline: None,
            },
            state: habitua::StatePolicy {
                format_version: 1,
                configuration_identity: "test".to_owned(),
                deletion: habitua::DeletionPolicy::Delete,
            },
        };
        let mut threshold_evaluation = repeated;
        let report = frozen_readout_report(&threshold_evaluation, &config);
        assert_eq!(report.reactions.len(), 1);
        assert_eq!(report.reactions[0].kind, ReactionKind::Novel);

        let mut high_threshold_config = config.clone();
        high_threshold_config.reactions.threshold = 1.0;
        threshold_evaluation.diagnostics.target_probability = 0.75;
        let high_threshold_report =
            frozen_readout_report(&threshold_evaluation, &high_threshold_config);
        assert!(high_threshold_report.reactions.is_empty());
    }

    #[test]
    fn frozen_readout_human_feedback_is_case_memory_not_weight_update() {
        let feedback = FrozenReadoutFeedback::default();
        feedback.insert(
            "change:screen",
            FeedbackEvent::new("food-1", "screen", FeedbackKind::Reward, 1.0, 1.0)
                .expect("feedback event"),
        );
        let mut model = FrozenReadoutModel::with_feedback_handle(
            1,
            23,
            RateBrainRole::Change,
            FrozenReadoutConfig {
                recent_observations: 4,
                distance_threshold: 0.01,
                previous_absolute_difference_scale: 1.0,
                history_elapsed_time_scale_seconds: 1.0,
                history_max_age_seconds: 300.0,
                l2_regularization: 0.0,
                learning_rate: 0.1,
                reaction_threshold: 0.5,
                training_mode: TrainingMode::ReadoutOnly,
                case_memory_distance_threshold: 4.0,
                case_memory_time_constant_seconds: 10.0,
                case_memory_logit_scale: 4.0,
                case_memory_max_cases: 8,
            },
            feedback.clone(),
        )
        .expect("frozen readout");
        let weights_before = model.weights();
        let first = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(1),
            )
            .expect("feedback evaluation");
        model.commit(first.update, 1.0).expect("feedback commit");
        assert_eq!(model.weights(), weights_before);
        let nearby = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(2),
            )
            .expect("nearby evaluation");
        assert_eq!(nearby.diagnostics.case_memory_match_count, 1);
        assert!(nearby.diagnostics.case_memory_adjustment > 2.0);
        let adjustment_near = nearby.diagnostics.case_memory_adjustment;
        model.commit(nearby.update, 0.0).expect("nearby commit");
        feedback.clear();
        let far = model
            .evaluate_input_with_id(
                "other",
                "other-stream",
                ModelInput::Features(&[100.0]),
                habitua::Timestamp::from_secs(101),
            )
            .expect("far evaluation");
        assert_eq!(far.diagnostics.case_memory_match_count, 0);
        let decayed = model
            .evaluate_input_with_id(
                "screen",
                "decay-stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(101),
            )
            .expect("decayed evaluation");
        assert_eq!(decayed.diagnostics.case_memory_match_count, 1);
        assert!(decayed.diagnostics.case_memory_adjustment < adjustment_near);
    }

    #[test]
    fn frozen_readout_case_memory_requires_exact_input_hash_before_distance_match() {
        let feedback = FrozenReadoutFeedback::default();
        feedback.insert(
            "change:image-a",
            FeedbackEvent::new("food-a", "image-a", FeedbackKind::Reward, 1.0, 1.0)
                .expect("feedback event"),
        );
        let mut model = FrozenReadoutModel::with_feedback_handle(
            1,
            31,
            RateBrainRole::Change,
            FrozenReadoutConfig {
                recent_observations: 4,
                distance_threshold: 0.01,
                previous_absolute_difference_scale: 1.0,
                history_elapsed_time_scale_seconds: 1.0,
                history_max_age_seconds: 300.0,
                l2_regularization: 0.0,
                learning_rate: 0.1,
                reaction_threshold: 0.5,
                training_mode: TrainingMode::ReadoutOnly,
                case_memory_distance_threshold: 4.0,
                case_memory_time_constant_seconds: 600.0,
                case_memory_logit_scale: 4.0,
                case_memory_max_cases: 8,
            },
            feedback,
        )
        .expect("frozen readout");
        let evaluation = model
            .evaluate_input_with_id(
                "image-a",
                "stream-a",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(1),
            )
            .expect("image-a evaluation");
        model
            .commit(evaluation.update, 1.0)
            .expect("image-a commit");
        let different_hash = model
            .evaluate_input_with_id(
                "image-b",
                "stream-b",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(2),
            )
            .expect("image-b evaluation");
        assert_eq!(different_hash.diagnostics.case_memory_match_count, 0);
    }

    #[test]
    fn delayed_feedback_decay_starts_at_event_time_not_observation_time() {
        let feedback = FrozenReadoutFeedback::default();
        let delayed =
            FeedbackEvent::new("delayed-food", "image-a", FeedbackKind::Reward, 1.0, 10.0)
                .expect("feedback event")
                .with_received_at_s(100.0)
                .expect("received timestamp");
        feedback.insert("change:image-a", delayed);
        let mut model = FrozenReadoutModel::with_feedback_handle(
            1,
            37,
            RateBrainRole::Change,
            FrozenReadoutConfig {
                recent_observations: 4,
                distance_threshold: 0.01,
                previous_absolute_difference_scale: 1.0,
                history_elapsed_time_scale_seconds: 1.0,
                history_max_age_seconds: 300.0,
                l2_regularization: 0.0,
                learning_rate: 0.1,
                reaction_threshold: 0.5,
                training_mode: TrainingMode::ReadoutOnly,
                case_memory_distance_threshold: 4.0,
                case_memory_time_constant_seconds: 10.0,
                case_memory_logit_scale: 4.0,
                case_memory_max_cases: 8,
            },
            feedback,
        )
        .expect("frozen readout");
        let evaluation = model
            .evaluate_input_with_id(
                "image-a",
                "stream-a",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(100),
            )
            .expect("delayed evaluation");
        model
            .commit(evaluation.update, 1.0)
            .expect("delayed commit");
        let after_commit = model
            .evaluate_input_with_id(
                "image-a",
                "stream-a",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(100),
            )
            .expect("post-commit evaluation");
        assert_eq!(after_commit.diagnostics.case_memory_match_count, 1);
        assert!(after_commit.diagnostics.case_memory_adjustment < 0.1);
    }

    #[test]
    fn delayed_feedback_case_survives_state_round_trip_with_event_timestamp() {
        let config = FrozenReadoutConfig {
            recent_observations: 4,
            distance_threshold: 0.01,
            previous_absolute_difference_scale: 1.0,
            history_elapsed_time_scale_seconds: 1.0,
            history_max_age_seconds: 300.0,
            l2_regularization: 0.0,
            learning_rate: 0.1,
            reaction_threshold: 0.5,
            training_mode: TrainingMode::ReadoutOnly,
            case_memory_distance_threshold: 4.0,
            case_memory_time_constant_seconds: 10.0,
            case_memory_logit_scale: 4.0,
            case_memory_max_cases: 8,
        };
        let feedback = FrozenReadoutFeedback::default();
        feedback.insert(
            "change:image-a",
            FeedbackEvent::new("delayed-food", "image-a", FeedbackKind::Reward, 1.0, 10.0)
                .expect("feedback event")
                .with_received_at_s(100.0)
                .expect("received timestamp"),
        );
        let mut model = FrozenReadoutModel::with_feedback_handle(
            1,
            41,
            RateBrainRole::Change,
            config.clone(),
            feedback,
        )
        .expect("frozen readout");
        let evaluation = model
            .evaluate_input_with_id(
                "image-a",
                "stream-a",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(100),
            )
            .expect("delayed evaluation");
        model
            .commit(evaluation.update, 1.0)
            .expect("delayed commit");
        let mut payload = Vec::new();
        model.save_state(&mut payload).expect("save state");
        let mut restored = FrozenReadoutModel::with_feedback_handle(
            1,
            41,
            RateBrainRole::Change,
            config,
            FrozenReadoutFeedback::default(),
        )
        .expect("restored readout");
        restored
            .load_state(&mut std::io::Cursor::new(payload))
            .expect("load state");
        let after_restore = restored
            .evaluate_input_with_id(
                "image-a",
                "stream-a",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(100),
            )
            .expect("restored evaluation");
        assert_eq!(after_restore.diagnostics.case_memory_match_count, 1);
        assert!(after_restore.diagnostics.case_memory_adjustment < 0.1);
    }

    #[test]
    fn frozen_readout_feedback_correction_and_cancellation_update_case_memory() {
        let feedback = FrozenReadoutFeedback::default();
        feedback.insert(
            "change:screen",
            FeedbackEvent::new("food-1", "screen", FeedbackKind::Reward, 1.0, 1.0)
                .expect("feedback event"),
        );
        let mut model = FrozenReadoutModel::with_feedback_handle(
            1,
            29,
            RateBrainRole::Change,
            FrozenReadoutConfig {
                recent_observations: 4,
                distance_threshold: 0.01,
                previous_absolute_difference_scale: 1.0,
                history_elapsed_time_scale_seconds: 1.0,
                history_max_age_seconds: 300.0,
                l2_regularization: 0.0,
                learning_rate: 0.1,
                reaction_threshold: 0.5,
                training_mode: TrainingMode::ReadoutOnly,
                case_memory_distance_threshold: 4.0,
                case_memory_time_constant_seconds: 600.0,
                case_memory_logit_scale: 4.0,
                case_memory_max_cases: 8,
            },
            feedback.clone(),
        )
        .expect("frozen readout");
        let first = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(1),
            )
            .expect("initial evaluation");
        model.commit(first.update, 1.0).expect("initial commit");
        let initial = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(2),
            )
            .expect("case evaluation");
        assert_eq!(initial.diagnostics.selected_feedback, FeedbackKind::Reward);
        assert_eq!(initial.diagnostics.case_memory_match_count, 1);
        model.commit(initial.update, 0.0).expect("case observation");

        let correction = FeedbackEvent::new("food-1", "screen", FeedbackKind::Punish, 1.0, 3.0)
            .expect("correction")
            .with_revision(1);
        feedback.insert("change:screen", correction);
        let corrected = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(3),
            )
            .expect("corrected evaluation");
        assert_eq!(corrected.diagnostics.case_memory_match_count, 0);
        assert_eq!(
            corrected.diagnostics.selected_feedback,
            FeedbackKind::Punish
        );
        model
            .commit(corrected.update, 1.0)
            .expect("corrected commit");
        let punished = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(4),
            )
            .expect("punished evaluation");
        assert_eq!(punished.diagnostics.case_memory_match_count, 1);
        assert!(punished.diagnostics.case_memory_adjustment < 0.0);
        model
            .commit(punished.update, 0.0)
            .expect("punished observation");

        feedback.insert(
            "change:screen",
            FeedbackEvent::cancellation("food-1", "screen", 5.0, 2).expect("cancellation"),
        );
        let cancelled = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(5),
            )
            .expect("cancelled evaluation");
        assert_eq!(cancelled.diagnostics.case_memory_match_count, 0);
        assert!(!cancelled.diagnostics.human_feedback_applied);
        assert_eq!(
            cancelled.diagnostics.selected_feedback,
            FeedbackKind::Punish
        );

        feedback.insert(
            "change:screen",
            FeedbackEvent::new("food-1", "screen", FeedbackKind::Reward, 1.0, 6.0)
                .expect("stale event"),
        );
        let stale = model
            .evaluate_input_with_id(
                "screen",
                "stream",
                ModelInput::Features(&[0.0]),
                habitua::Timestamp::from_secs(6),
            )
            .expect("stale evaluation");
        assert!(!stale.diagnostics.human_feedback_applied);
        assert_eq!(stale.diagnostics.case_memory_match_count, 0);
    }

    #[test]
    fn shared_history_features_keep_elapsed_time_after_history_is_bounded() {
        let history = (0..5)
            .map(|timestamp_s| FrozenReadoutHistoryEntry {
                features: vec![timestamp_s as f32],
                timestamp_s: timestamp_s as f64,
            })
            .collect::<Vec<_>>();
        let (diagnostics, augmented) =
            frozen_readout_augmented_features(&[5.0], &history, 5.0, 4, 0.01, 1.0, 3_600.0);
        assert_eq!(diagnostics.elapsed_seconds, Some(1.0));
        assert_eq!(diagnostics.repeat_count, 0);
        assert_eq!(augmented.len(), 5);
        assert_eq!(augmented[4], 1.0 / 3_600.0);
    }
}
