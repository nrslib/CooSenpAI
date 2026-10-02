use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io::{self, Cursor, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::config::Config;
use crate::decaying_memory::DecayingMemory;
use crate::encoder::FlyHashEncoder;
use crate::failure::{FailureDiagnostics, FailureModel, FailureModelConfig, FailureUpdate};
use crate::habituation::{FlyDiagnostics, Habituation};
use crate::model::{
    Evaluation, FailureObservation, Model, ModelError, ModelInput, PersistentModel,
};
use crate::nearest_neighbor::{
    NearestNeighborConfig, NearestNeighborDiagnostics, NearestNeighborModel,
};
use crate::numeric::{
    NamedDeviation, NumericDeviationConfig, NumericDeviationDiagnostics, NumericDeviationModel,
};
use crate::persistence::{
    LoadBudget, MAX_LOAD_ELEMENTS, read_count, read_f32, read_string, read_timestamp, read_u8,
    read_u16, read_u32, read_u64, write_f32, write_string, write_timestamp, write_u8, write_u16,
    write_u32, write_u64,
};
use crate::reaction::{
    BrainEvaluationState, BrainMode, Evidence, LearningSignal, Metric, MetricDirection, Reaction,
    ReactionKind, Readiness,
};
use crate::timestamp::Timestamp;

const ORCHESTRATION_MAGIC: &[u8; 8] = b"BRAINS\0\0";
const ORCHESTRATION_LEGACY_STATE_VERSION: u16 = 1;
const ORCHESTRATION_PREVIOUS_STATE_VERSION: u16 = 2;
const ORCHESTRATION_THIRD_STATE_VERSION: u16 = 3;
const ORCHESTRATION_STATE_VERSION: u16 = 4;
const MAX_BRAIN_COUNT: usize = 4_096;
const MAX_BRAIN_ID_LENGTH: usize = 4 * 1024;
const MAX_MODEL_KIND_LENGTH: usize = 256;
static NEXT_RUNNER_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Selects a built-in model or carries settings for a registered custom model.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelSettings {
    /// Configuration for the Fly encoder and decaying memory.
    Fly(Config),
    /// Names and configuration for the numeric deviation model.
    Numeric {
        /// Names of the numeric values in the input vector.
        names: Vec<String>,
        /// Numeric model configuration.
        config: NumericDeviationConfig,
    },
    /// Configuration for the bounded nearest-neighbor model.
    NearestNeighbor(NearestNeighborConfig),
    /// Configuration for the structured, non-learning failure model.
    Failure(FailureModelConfig),
    /// Caller-defined settings passed to a custom registry factory.
    Custom(BTreeMap<String, String>),
}

/// Identifies a model implementation in a brain configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelSpec {
    /// Registry name of the model implementation.
    pub kind: String,
    /// Version of the model implementation and its behavior.
    pub version: u32,
    /// Optional experiment seed recorded as part of the configuration.
    pub seed: Option<u64>,
    /// Model-specific settings.
    pub settings: ModelSettings,
}

/// Input selection and schema information for one brain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrainInputSpec {
    /// Adapter-defined feature schema identifier.
    pub schema_id: String,
    /// Adapter-defined feature schema version.
    pub schema_version: u32,
    /// Target selected by the adapter, such as `screen` or `latency`.
    pub target: String,
    /// Stable context partitioning description.
    pub context: String,
}

/// A limit on automatic learning frequency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrequencyLimit {
    /// Maximum number of positive-weight commits in the window.
    pub maximum_commits: u32,
    /// Window in which the maximum applies.
    pub window: Duration,
}

/// Automatic learning mode for one brain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LearningMode {
    /// Learn every evaluated input that satisfies the readiness condition.
    EveryEvaluation,
    /// Learn after a run of normal evaluations, then continue stabilizing it.
    RepeatedNormal {
        /// Number of consecutive reaction-free evaluations needed first.
        consecutive_observations: u32,
    },
    /// Learn only when the same stream is revisited after the interval.
    Revisit {
        /// Minimum interval between positive-weight commits for a stream.
        interval: Duration,
    },
    /// Do not automatically learn; the caller may commit an update manually.
    Manual,
}

/// Learning policy applied after an evaluation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrainLearningPolicy {
    /// Mode used to decide the recommended weight.
    pub mode: LearningMode,
    /// Weight used when the mode permits learning.
    pub weight: f32,
    /// Whether a model must report `Evaluated` before learning.
    pub require_evaluated: bool,
    /// Whether a baseline-insufficient evaluation may bootstrap the model.
    ///
    /// This is explicit because a detector must not be treated as normal
    /// merely because it has not collected enough baseline data yet.
    pub bootstrap: bool,
    /// Whether a repeated-normal policy may count an evaluation that emitted
    /// a configured reaction as normal for learning purposes.
    pub learn_on_reaction: bool,
    /// Optional frequency limit for positive-weight commits.
    pub frequency_limit: Option<FrequencyLimit>,
}

impl Default for BrainLearningPolicy {
    fn default() -> Self {
        Self {
            mode: LearningMode::EveryEvaluation,
            weight: 1.0,
            // A baseline-insufficient model may still need to learn its
            // initial baseline. Its report remains non-evaluable until the
            // model-specific readiness condition is met.
            require_evaluated: false,
            bootstrap: false,
            learn_on_reaction: false,
            frequency_limit: None,
        }
    }
}

/// Reaction threshold and calibration declaration for one brain.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactionPolicy {
    /// Raw model-specific threshold used by the built-in reporter.
    pub threshold: f32,
    /// Reaction kinds allowed from this brain.
    pub kinds: Vec<ReactionKind>,
    /// Name of an external calibration procedure, if one was applied.
    pub calibration: Option<String>,
}

impl Default for ReactionPolicy {
    fn default() -> Self {
        Self {
            threshold: 0.8,
            kinds: vec![ReactionKind::Novel, ReactionKind::Deviation],
            calibration: None,
        }
    }
}

/// Resource limits and evaluation deadline for one brain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourcePolicy {
    /// Maximum number of independent streams held by the model.
    pub max_streams: usize,
    /// Maximum number of feature samples or representatives, when applicable.
    pub max_samples: usize,
    /// Maximum serialized model-state bytes held by this runner.
    pub max_memory_bytes: usize,
    /// Maximum allowed delay from input availability to evaluation.
    pub evaluation_deadline: Option<Duration>,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            max_streams: 4_096,
            max_samples: 4_096,
            max_memory_bytes: MAX_LOAD_ELEMENTS,
            evaluation_deadline: None,
        }
    }
}

/// State format and identity policy for one brain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePolicy {
    /// Model-specific state format version expected by this configuration.
    /// Zero means the current version reported by the registered model.
    pub format_version: u16,
    /// Caller-defined value identifying the complete compatible configuration.
    pub configuration_identity: String,
    /// Policy used when a brain is removed from a runner.
    pub deletion: DeletionPolicy,
}

/// Determines what [`BrainRunner::remove_brain`] does with a brain state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeletionPolicy {
    /// Remove the brain and its state from the runner.
    Delete,
    /// Keep the state by disabling the brain in place.
    Retain,
}

impl Default for StatePolicy {
    fn default() -> Self {
        Self {
            format_version: 0,
            configuration_identity: "default".to_owned(),
            deletion: DeletionPolicy::Delete,
        }
    }
}

/// Complete configuration of one independently evaluated brain.
#[derive(Clone, Debug, PartialEq)]
pub struct BrainConfig {
    /// Unique brain instance identifier.
    pub id: String,
    /// Human-readable role, such as `screen-text` or `latency`.
    pub role: String,
    /// Whether this brain is evaluated and learns.
    pub enabled: bool,
    /// Whether its reactions may participate in the caller's activation policy.
    pub mode: BrainMode,
    /// Input selection and schema contract.
    pub input: BrainInputSpec,
    /// Model registry specification.
    pub model: ModelSpec,
    /// Automatic learning policy.
    pub learning: BrainLearningPolicy,
    /// Reaction reporting policy.
    pub reactions: ReactionPolicy,
    /// Resource and deadline policy.
    pub resources: ResourcePolicy,
    /// Persistence compatibility policy.
    pub state: StatePolicy,
}

impl BrainConfig {
    /// Validates fields that are independent of the selected registry model.
    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.id.is_empty() || self.id.len() > MAX_BRAIN_ID_LENGTH {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "brain id must be non-empty and within its size limit".to_owned(),
            });
        }
        if self.role.is_empty()
            || self.input.schema_id.is_empty()
            || self.input.target.is_empty()
            || self.input.context.is_empty()
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "role, schema_id, target, and context must be non-empty".to_owned(),
            });
        }
        if self.input.schema_version == 0
            || self.model.kind.is_empty()
            || self.model.kind.len() > MAX_MODEL_KIND_LENGTH
            || self.model.version == 0
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "schema and model versions must be non-zero".to_owned(),
            });
        }
        if !self.learning.weight.is_finite() || !(0.0..=1.0).contains(&self.learning.weight) {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "learning weight must be finite and in 0..=1".to_owned(),
            });
        }
        match self.learning.mode {
            LearningMode::RepeatedNormal {
                consecutive_observations: 0,
            } => {
                return Err(RegistryError::InvalidConfiguration {
                    brain: self.id.clone(),
                    reason: "consecutive_observations must be greater than zero".to_owned(),
                });
            }
            LearningMode::Revisit { interval } if interval.is_zero() => {
                return Err(RegistryError::InvalidConfiguration {
                    brain: self.id.clone(),
                    reason: "revisit interval must be non-zero".to_owned(),
                });
            }
            _ => {}
        }
        if let Some(limit) = self.learning.frequency_limit
            && (limit.maximum_commits == 0 || limit.window.is_zero())
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "frequency limit must have a positive count and window".to_owned(),
            });
        }
        if !self.reactions.threshold.is_finite() || self.reactions.threshold < 0.0 {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "reaction threshold must be finite and non-negative".to_owned(),
            });
        }
        if self.resources.max_streams == 0
            || self.resources.max_samples == 0
            || self.resources.max_memory_bytes == 0
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "resource limits must be greater than zero".to_owned(),
            });
        }
        if self.resources.max_streams > MAX_LOAD_ELEMENTS
            || self.resources.max_samples > MAX_LOAD_ELEMENTS
            || self.resources.max_memory_bytes > MAX_LOAD_ELEMENTS
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "resource limits exceed the runner persistence budget".to_owned(),
            });
        }
        if self.state.configuration_identity.is_empty()
            || self.state.configuration_identity.len() > MAX_BRAIN_ID_LENGTH
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: self.id.clone(),
                reason: "state configuration identity must be non-empty and within its size limit"
                    .to_owned(),
            });
        }
        Ok(())
    }

    /// Returns a stable fingerprint of settings that affect model state.
    pub fn configuration_fingerprint(&self) -> u64 {
        self.configuration_fingerprint_for_format(ORCHESTRATION_STATE_VERSION)
    }

    fn configuration_fingerprint_for_format(&self, format_version: u16) -> u64 {
        let mut words = Vec::new();
        words.push(u64::from(format_version >= ORCHESTRATION_THIRD_STATE_VERSION) + 1);
        push_string(&mut words, &self.id);
        push_string(&mut words, &self.role);
        push_string(&mut words, &self.input.schema_id);
        push_string(&mut words, &self.input.target);
        push_string(&mut words, &self.input.context);
        words.extend([
            u64::from(u8::from(self.enabled)),
            u64::from(self.mode as u8),
            u64::from(self.input.schema_version),
            u64::from(self.model.version),
            self.model.seed.unwrap_or(0),
            u64::from(self.state.format_version),
            u64::from(self.state.deletion as u8),
            u64::from(u8::from(self.learning.require_evaluated)),
            u64::from(self.learning.weight.to_bits()),
            u64::from(self.reactions.threshold.to_bits()),
            u64::from(self.resources.max_streams as u32),
            u64::from(self.resources.max_samples as u32),
            self.resources.max_memory_bytes as u64,
        ]);
        if format_version >= ORCHESTRATION_STATE_VERSION {
            words.extend([
                u64::from(u8::from(self.learning.bootstrap)),
                u64::from(u8::from(self.learning.learn_on_reaction)),
            ]);
        }
        if let Some(deadline) = self.resources.evaluation_deadline {
            words.extend([1, deadline.as_secs(), u64::from(deadline.subsec_nanos())]);
        } else {
            words.push(0);
        }
        match self.learning.mode {
            LearningMode::EveryEvaluation => words.push(0),
            LearningMode::RepeatedNormal {
                consecutive_observations,
            } => words.extend([1, u64::from(consecutive_observations)]),
            LearningMode::Revisit { interval } => {
                words.extend([2, interval.as_secs(), u64::from(interval.subsec_nanos())])
            }
            LearningMode::Manual => words.push(3),
        }
        if let Some(limit) = self.learning.frequency_limit {
            words.extend([
                1,
                u64::from(limit.maximum_commits),
                limit.window.as_secs(),
                u64::from(limit.window.subsec_nanos()),
            ]);
        } else {
            words.push(0);
        }
        push_string(&mut words, &self.model.kind);
        push_string(&mut words, &self.state.configuration_identity);
        push_string(
            &mut words,
            self.reactions.calibration.as_deref().unwrap_or(""),
        );
        for kind in &self.reactions.kinds {
            words.push(reaction_kind_tag(kind));
        }
        push_model_settings_for_format(&mut words, &self.model.settings, format_version);
        crate::pattern::stable_hash(words)
    }

    /// Returns the compatibility identity used when copying state between an
    /// active brain and a shadow brain. The brain ID, role, enabled flag, and
    /// active/shadow mode are intentionally excluded; input meaning, model
    /// settings, state identity, learning policy, and resource limits remain
    /// part of the check.
    pub fn state_compatibility_fingerprint(&self) -> u64 {
        let mut words = vec![2];
        push_string(&mut words, &self.input.schema_id);
        push_string(&mut words, &self.input.target);
        push_string(&mut words, &self.input.context);
        words.extend([
            u64::from(self.input.schema_version),
            u64::from(self.model.version),
            u64::from(u8::from(self.model.seed.is_some())),
            self.model.seed.unwrap_or(0),
            u64::from(self.state.format_version),
            u64::from(u8::from(self.learning.require_evaluated)),
            u64::from(self.learning.weight.to_bits()),
            u64::from(u8::from(self.learning.bootstrap)),
            u64::from(u8::from(self.learning.learn_on_reaction)),
            u64::from(self.reactions.threshold.to_bits()),
            self.resources.max_streams as u64,
            self.resources.max_samples as u64,
            self.resources.max_memory_bytes as u64,
        ]);
        if let Some(deadline) = self.resources.evaluation_deadline {
            words.extend([1, deadline.as_secs(), u64::from(deadline.subsec_nanos())]);
        } else {
            words.push(0);
        }
        words.push(u64::from(u8::from(self.reactions.calibration.is_some())));
        push_string(
            &mut words,
            self.reactions.calibration.as_deref().unwrap_or(""),
        );
        for kind in &self.reactions.kinds {
            words.push(reaction_kind_tag(kind));
        }
        push_learning_mode(&mut words, self.learning.mode);
        if let Some(limit) = self.learning.frequency_limit {
            words.extend([
                1,
                u64::from(limit.maximum_commits),
                limit.window.as_secs(),
                u64::from(limit.window.subsec_nanos()),
            ]);
        } else {
            words.push(0);
        }
        push_string(&mut words, &self.model.kind);
        push_string(&mut words, &self.state.configuration_identity);
        push_model_settings(&mut words, &self.model.settings);
        crate::pattern::stable_hash(words)
    }
}

/// A feature vector finalized by an adapter before model evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct BrainInput {
    /// Adapter or source record identifier.
    pub id: String,
    /// Stable stream/context identifier.
    pub stream_id: String,
    /// Target represented by this input.
    pub target: String,
    /// Context partition selected by the adapter.
    pub context: String,
    /// Input schema identifier.
    pub schema_id: String,
    /// Input schema version.
    pub schema_version: u32,
    /// Features supplied to each matching brain.
    pub features: Vec<f32>,
    /// Optional structured result for a failure brain. Feature-vector brains
    /// ignore this field and continue to use `features`.
    pub failure: Option<FailureObservation>,
    /// Time at which the input became available to the caller.
    pub available_at: Timestamp,
    /// Optional time at which this runner processes the input. When absent,
    /// the availability time is used and no processing delay is reported.
    pub processed_at: Option<Timestamp>,
    /// Monotonic adapter processing position.
    pub position: u64,
}

/// Common report produced by a type-erased model wrapper.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelReport {
    /// Short-term novelty, when the model defines it.
    pub novelty_short: Option<f32>,
    /// Long-term novelty, when the model defines it.
    pub novelty_long: Option<f32>,
    /// Short-term familiarity, when the model defines it.
    pub familiarity_short: Option<f32>,
    /// Long-term familiarity, when the model defines it.
    pub familiarity_long: Option<f32>,
    /// Exact-pattern recency, when the model defines it.
    pub recency: Option<Duration>,
    /// Pattern fingerprint, when the model defines it.
    pub pattern_id: Option<u64>,
    /// Readiness of the model-specific result.
    pub readiness: Readiness,
    /// Learning signal independent of the configured reaction kinds.
    pub learning_signal: LearningSignal,
    /// Model-specific metrics with units and directions.
    pub metrics: Vec<Metric>,
    /// Evidence supporting the metrics or a readiness state.
    pub evidence: Vec<Evidence>,
    /// Reactions selected by the configured reporter.
    pub reactions: Vec<Reaction>,
}

/// Evaluation of one brain before any brain commits its update.
pub struct BrainEvaluation {
    /// Brain instance identifier.
    pub brain: String,
    /// Brain role.
    pub role: String,
    /// Active or shadow mode at evaluation time.
    pub mode: BrainMode,
    /// Target from the brain configuration.
    pub target: String,
    /// Common and model-specific report.
    pub report: ModelReport,
    /// Recommended weight from the brain's learning policy.
    pub recommended_learning_weight: f32,
    /// Evaluation processing time measured by the runner.
    pub processing_time: Duration,
    /// Opaque update bound to this brain, configuration, stream, and generation.
    pub update: Option<BrainUpdate>,
}

/// Evaluation of all enabled brains for one finalized input.
pub struct OrchestrationEvaluation {
    /// Input identifier.
    pub input_id: String,
    /// Timestamp used for every model evaluation.
    pub available_at: Timestamp,
    /// One result for each configured brain.
    pub brains: Vec<BrainEvaluation>,
    /// Active-brain reactions suitable for the caller's activation policy.
    ///
    /// These are individual brain reactions for diagnostics. When a
    /// [`ConsensusPolicy`] is configured, external callers should use
    /// [`Self::external_reactions`] instead.
    pub active_reactions: Vec<Reaction>,
    /// Action selected by the configured consensus policy.
    pub consensus_action: ConsensusAction,
    /// Reactions approved for an external caller after consensus.
    pub external_reactions: Vec<Reaction>,
}

/// Final action selected from the configured change and no-change brains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsensusAction {
    /// The change brain had sufficient evidence.
    NotifyChange,
    /// The no-change brain had sufficient evidence.
    SuppressNoChange,
    /// Evidence was insufficient or contradictory.
    Hold,
}

/// Selects external reactions from two role-specific probability metrics.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsensusPolicy {
    /// Role whose probability means that a change was detected.
    pub change_role: String,
    /// Role whose probability means that no change was detected.
    pub no_change_role: String,
    /// Metric name containing each role's probability.
    pub probability_metric: String,
    /// Probability threshold applied to both roles.
    pub threshold: f32,
    /// Minimum margin above or below the threshold for both roles.
    pub margin: f32,
    /// Reaction emitted when the change side wins.
    pub change_reaction: ReactionKind,
    /// Reaction emitted when the no-change side wins.
    pub no_change_reaction: ReactionKind,
}

impl ConsensusPolicy {
    /// Creates a policy from probability values.
    pub fn new(
        change_role: impl Into<String>,
        no_change_role: impl Into<String>,
        probability_metric: impl Into<String>,
        threshold: f32,
        margin: f32,
        change_reaction: ReactionKind,
        no_change_reaction: ReactionKind,
    ) -> Result<Self, RegistryError> {
        if !threshold.is_finite()
            || !(0.0..=1.0).contains(&threshold)
            || !margin.is_finite()
            || !(0.0..0.5).contains(&margin)
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: "consensus".to_owned(),
                reason: "consensus threshold and margin are invalid".to_owned(),
            });
        }
        let policy = Self {
            change_role: change_role.into(),
            no_change_role: no_change_role.into(),
            probability_metric: probability_metric.into(),
            threshold,
            margin,
            change_reaction,
            no_change_reaction,
        };
        if policy.change_role.is_empty()
            || policy.no_change_role.is_empty()
            || policy.probability_metric.is_empty()
            || policy.change_role == policy.no_change_role
        {
            return Err(RegistryError::InvalidConfiguration {
                brain: "consensus".to_owned(),
                reason: "consensus roles and metric must be distinct and non-empty".to_owned(),
            });
        }
        Ok(policy)
    }

    /// Returns the configured probability threshold.
    pub fn threshold_value(&self) -> f32 {
        self.threshold
    }

    /// Returns the configured probability margin.
    pub fn margin_value(&self) -> f32 {
        self.margin
    }
}

/// Result of a successful or zero-weight commit.
#[derive(Clone, Debug, PartialEq)]
pub struct CommitRecord {
    /// Brain identifier.
    pub brain: String,
    /// Weight passed to the model.
    pub weight: f32,
    /// Whether positive learning was applied.
    pub learned: bool,
}

/// An update whose owner and generation are checked by [`BrainRunner`].
pub struct BrainUpdate {
    brain: String,
    runner_token: u64,
    configuration_fingerprint: u64,
    stream_id: String,
    at: Timestamp,
    generation: u64,
    normal_streak_action: NormalStreakAction,
    update: Box<dyn ErasedUpdate>,
}

/// Registry-level errors, including unknown model and incompatible state errors.
#[derive(Debug)]
pub enum RegistryError {
    /// A brain configuration is invalid.
    InvalidConfiguration {
        /// Brain identifier being validated.
        brain: String,
        /// Validation explanation.
        reason: String,
    },
    /// A model kind is not registered.
    UnknownModel {
        /// Unknown registry kind.
        kind: String,
    },
    /// A model factory or reporter rejected its configuration.
    Model {
        /// Brain identifier.
        brain: String,
        /// Factory or model error.
        reason: String,
    },
    /// A brain ID is duplicated.
    DuplicateBrain {
        /// Duplicated brain identifier.
        brain: String,
    },
    /// A registry kind is already registered.
    DuplicateModel {
        /// Duplicated registry kind.
        kind: String,
    },
    /// A saved state does not match the current brain configuration.
    IncompatibleState {
        /// Brain identifier.
        brain: String,
        /// Compatibility failure explanation.
        reason: String,
    },
    /// A serialized state is malformed or exceeds a bound.
    InvalidState(String),
    /// An underlying I/O operation failed.
    Io(io::Error),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration { brain, reason } => {
                write!(formatter, "invalid brain configuration {brain}: {reason}")
            }
            Self::UnknownModel { kind } => write!(formatter, "unknown model kind {kind}"),
            Self::Model { brain, reason } => write!(formatter, "model {brain}: {reason}"),
            Self::DuplicateBrain { brain } => write!(formatter, "duplicate brain id {brain}"),
            Self::DuplicateModel { kind } => write!(formatter, "duplicate model kind {kind}"),
            Self::IncompatibleState { brain, reason } => {
                write!(formatter, "incompatible state for brain {brain}: {reason}")
            }
            Self::InvalidState(reason) => {
                write!(formatter, "invalid orchestration state: {reason}")
            }
            Self::Io(error) => write!(formatter, "orchestration I/O error: {error}"),
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidConfiguration { .. }
            | Self::UnknownModel { .. }
            | Self::Model { .. }
            | Self::DuplicateBrain { .. }
            | Self::DuplicateModel { .. }
            | Self::IncompatibleState { .. }
            | Self::InvalidState(_) => None,
        }
    }
}

impl From<io::Error> for RegistryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Errors produced while evaluating or committing a brain.
#[derive(Debug)]
pub enum RunnerError {
    /// The registry or brain configuration is invalid.
    Registry(RegistryError),
    /// A brain update belongs to another brain or configuration.
    ForeignUpdate,
    /// An update was evaluated from an older runner generation.
    StaleUpdate,
    /// A model rejected a commit or a persisted payload.
    Model {
        /// Brain identifier.
        brain: String,
        /// Underlying model error.
        source: ModelError,
    },
    /// The input schema does not match a brain.
    SchemaMismatch {
        /// Brain identifier.
        brain: String,
    },
    /// A built-in model's configured input dimension differs from the input.
    DimensionMismatch {
        /// Brain identifier.
        brain: String,
        /// Expected dimension.
        expected: usize,
        /// Actual dimension.
        actual: usize,
    },
    /// The runner's stream or state resource limit would be exceeded.
    ResourceLimit {
        /// Brain identifier.
        brain: String,
        /// Resource limit explanation.
        reason: String,
    },
    /// The adapter supplied an internally inconsistent or out-of-order input.
    InvalidInput {
        /// Explanation of the invalid input.
        reason: String,
    },
}

impl fmt::Display for RunnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => error.fmt(formatter),
            Self::ForeignUpdate => write!(formatter, "brain update belongs to another brain"),
            Self::StaleUpdate => write!(formatter, "brain update is stale"),
            Self::Model { brain, source } => write!(formatter, "model {brain}: {source}"),
            Self::SchemaMismatch { brain } => {
                write!(formatter, "input schema does not match brain {brain}")
            }
            Self::DimensionMismatch {
                brain,
                expected,
                actual,
            } => write!(
                formatter,
                "brain {brain} expects {expected} features, got {actual}"
            ),
            Self::ResourceLimit { brain, reason } => {
                write!(formatter, "brain {brain} resource limit: {reason}")
            }
            Self::InvalidInput { reason } => write!(formatter, "invalid runner input: {reason}"),
        }
    }
}

impl std::error::Error for RunnerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Registry(error) => Some(error),
            Self::Model { source, .. } => Some(source),
            Self::ForeignUpdate
            | Self::StaleUpdate
            | Self::SchemaMismatch { .. }
            | Self::DimensionMismatch { .. }
            | Self::ResourceLimit { .. }
            | Self::InvalidInput { .. } => None,
        }
    }
}

impl From<RegistryError> for RunnerError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

fn push_string(words: &mut Vec<u64>, value: &str) {
    words.push(value.len() as u64);
    words.extend(value.bytes().map(u64::from));
}

fn push_learning_mode(words: &mut Vec<u64>, mode: LearningMode) {
    match mode {
        LearningMode::EveryEvaluation => words.push(0),
        LearningMode::RepeatedNormal {
            consecutive_observations,
        } => words.extend([1, u64::from(consecutive_observations)]),
        LearningMode::Revisit { interval } => {
            words.extend([2, interval.as_secs(), u64::from(interval.subsec_nanos())])
        }
        LearningMode::Manual => words.push(3),
    }
}

fn push_model_settings(words: &mut Vec<u64>, settings: &ModelSettings) {
    push_model_settings_for_format(words, settings, ORCHESTRATION_STATE_VERSION);
}

fn push_model_settings_for_format(
    words: &mut Vec<u64>,
    settings: &ModelSettings,
    format_version: u16,
) {
    match settings {
        ModelSettings::Fly(config) => {
            words.push(1);
            words.extend([
                config.dimensions as u64,
                config.encoder_units as u64,
                config.k as u64,
                config.seed,
                u64::from(config.projection_density.to_bits()),
                config.short_time_constant.as_secs(),
                u64::from(config.short_time_constant.subsec_nanos()),
                config.long_time_constant.as_secs(),
                u64::from(config.long_time_constant.subsec_nanos()),
                u64::from(config.short_reinforcement.to_bits()),
                u64::from(config.long_reinforcement.to_bits()),
                u64::from(config.short_familiarity_weight.to_bits()),
                u64::from(config.long_familiarity_weight.to_bits()),
                u64::from(config.input_strength_scale.to_bits()),
                u64::from(config.temporal_shift_scale.to_bits()),
                u64::from(config.variance_floor.to_bits()),
                config.max_patterns as u64,
                config.max_streams as u64,
                config.max_memory_elements as u64,
                u64::from(config.model_version),
                u64::from(config.feature_schema_version),
            ]);
            if format_version >= ORCHESTRATION_THIRD_STATE_VERSION {
                push_long_learning_policy(words, &config.long_learning_policy);
            }
        }
        ModelSettings::Numeric { names, config } => {
            words.push(2);
            words.extend([
                u64::from(config.mad_floor.to_bits()),
                u64::from(config.learning_rate.to_bits()),
                config.max_samples as u64,
                config.max_streams as u64,
                config.sample_window.as_secs(),
                u64::from(config.sample_window.subsec_nanos()),
                config.minimum_samples as u64,
                config.max_patterns as u64,
                u64::from(config.model_version),
                u64::from(config.feature_schema_version),
                u64::from(config.direction as u8),
            ]);
            for name in names {
                push_string(words, name);
            }
        }
        ModelSettings::NearestNeighbor(config) => {
            words.push(3);
            words.extend([
                config.dimensions as u64,
                config.max_representatives as u64,
                u64::from(config.merge_distance.to_bits()),
                config.max_patterns as u64,
                config.max_streams as u64,
                u64::from(config.model_version),
                u64::from(config.feature_schema_version),
            ]);
        }
        ModelSettings::Failure(config) => {
            words.push(5);
            words.extend([
                u64::from(config.model_version),
                u64::from(config.feature_schema_version),
            ]);
        }
        ModelSettings::Custom(settings) => {
            words.push(4);
            for (key, value) in settings {
                push_string(words, key);
                push_string(words, value);
            }
        }
    }
}

fn push_long_learning_policy(words: &mut Vec<u64>, policy: &crate::LongLearningPolicy) {
    match policy {
        crate::LongLearningPolicy::Revisit { interval } => {
            words.extend([0, interval.as_secs(), u64::from(interval.subsec_nanos())]);
        }
        crate::LongLearningPolicy::RepeatedNormal {
            consecutive_observations,
        } => words.extend([1, u64::from(*consecutive_observations)]),
        crate::LongLearningPolicy::LegacyEveryCommit => words.push(2),
    }
}

trait ErasedUpdate {
    fn commit(
        self: Box<Self>,
        model: &mut dyn ErasedBrainModel,
        weight: f32,
    ) -> Result<(), ModelError>;
}

trait ErasedBrainModel {
    fn evaluate(
        &self,
        config: &BrainConfig,
        input: &BrainInput,
    ) -> Result<(ModelReport, Box<dyn ErasedUpdate>), ModelError>;
    fn commit(&mut self, update: Box<dyn ErasedUpdate>, weight: f32) -> Result<(), ModelError>;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    fn identity(&self) -> crate::ModelIdentity;
    fn fingerprint(&self) -> u64;
    fn state_format_version(&self) -> u16;
    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()>;
    fn save_state_with_budget(
        &self,
        writer: &mut dyn Write,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()>;
    fn load_state(&mut self, reader: &mut dyn Read) -> io::Result<()>;
    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()>;
}

struct TypedUpdate<M: Model + 'static> {
    update: M::Update,
}

impl<M> ErasedUpdate for TypedUpdate<M>
where
    M: Model + 'static,
    M::Update: 'static,
{
    fn commit(
        self: Box<Self>,
        model: &mut dyn ErasedBrainModel,
        weight: f32,
    ) -> Result<(), ModelError> {
        let typed_model = model
            .as_any_mut()
            .downcast_mut::<M>()
            .ok_or(ModelError::ForeignUpdate)?;
        typed_model.commit(self.update, weight)
    }
}

type TypedReporter<M> = Arc<
    dyn Fn(
        &Evaluation<<M as Model>::Diagnostics, <M as Model>::Update>,
        &BrainConfig,
    ) -> ModelReport,
>;

struct TypedBrainModel<M: Model> {
    model: M,
    reporter: TypedReporter<M>,
}

impl<M> ErasedBrainModel for TypedBrainModel<M>
where
    M: PersistentModel + 'static,
    M::Diagnostics: 'static,
    M::Update: 'static,
{
    fn evaluate(
        &self,
        config: &BrainConfig,
        input: &BrainInput,
    ) -> Result<(ModelReport, Box<dyn ErasedUpdate>), ModelError> {
        let model_input = if config.model.kind == "failure" {
            input
                .failure
                .as_ref()
                .map(ModelInput::Failure)
                .ok_or_else(|| {
                    ModelError::Incompatible(
                        "failure model requires a structured result".to_owned(),
                    )
                })?
        } else {
            ModelInput::Features(&input.features)
        };
        let evaluation = self.model.evaluate_input_with_id(
            &input.id,
            &input.stream_id,
            model_input,
            input.available_at,
        )?;
        validate_evaluation(&evaluation)?;
        let report = (self.reporter)(&evaluation, config);
        validate_report(&report, config)?;
        let update = Box::new(TypedUpdate::<M> {
            update: evaluation.update,
        });
        Ok((report, update))
    }

    fn commit(&mut self, update: Box<dyn ErasedUpdate>, weight: f32) -> Result<(), ModelError> {
        update.commit(self, weight)
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        &mut self.model
    }

    fn identity(&self) -> crate::ModelIdentity {
        self.model.identity()
    }

    fn fingerprint(&self) -> u64 {
        self.model.fingerprint()
    }

    fn state_format_version(&self) -> u16 {
        self.model.state_format_version()
    }

    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()> {
        self.model.save_state(writer)
    }

    fn save_state_with_budget(
        &self,
        writer: &mut dyn Write,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()> {
        self.model.save_state_with_budget(writer, load_budget)
    }

    fn load_state(&mut self, reader: &mut dyn Read) -> io::Result<()> {
        self.model.load_state(reader)
    }

    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()> {
        self.model.load_state_with_budget(reader, load_budget)
    }
}

type ErasedFactory = dyn Fn(&BrainConfig) -> Result<Box<dyn ErasedBrainModel>, RegistryError>;

/// Registry of model factories used by [`BrainRunner`].
pub struct ModelRegistry {
    factories: BTreeMap<String, Arc<ErasedFactory>>,
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl ModelRegistry {
    /// Creates a registry containing the core Fly, Numeric, nearest-neighbor,
    /// and structured failure models. External brain plugins register their
    /// own model and report converter explicitly.
    pub fn with_builtins() -> Self {
        let mut registry = Self {
            factories: BTreeMap::new(),
        };
        registry.register_builtins();
        registry
    }

    /// Creates an empty registry for custom models.
    pub fn empty() -> Self {
        Self {
            factories: BTreeMap::new(),
        }
    }

    /// Registers a persistent typed model and its report converter.
    pub fn register<M, F, R>(
        &mut self,
        kind: impl Into<String>,
        factory: F,
        reporter: R,
    ) -> Result<(), RegistryError>
    where
        M: PersistentModel + 'static,
        M::Diagnostics: 'static,
        M::Update: 'static,
        F: Fn(&BrainConfig) -> Result<M, RegistryError> + 'static,
        R: Fn(&Evaluation<M::Diagnostics, M::Update>, &BrainConfig) -> ModelReport + 'static,
    {
        let kind = kind.into();
        if kind.is_empty() || kind.len() > MAX_MODEL_KIND_LENGTH {
            return Err(RegistryError::InvalidConfiguration {
                brain: kind,
                reason: "model kind has an invalid length".to_owned(),
            });
        }
        if self.factories.contains_key(&kind) {
            return Err(RegistryError::DuplicateModel { kind });
        }
        let reporter: TypedReporter<M> = Arc::new(reporter);
        let factory = Arc::new(move |config: &BrainConfig| {
            let model = factory(config)?;
            Ok(Box::new(TypedBrainModel {
                model,
                reporter: Arc::clone(&reporter),
            }) as Box<dyn ErasedBrainModel>)
        });
        self.factories.insert(kind, factory);
        Ok(())
    }

    /// Returns whether a model kind is registered.
    pub fn contains(&self, kind: &str) -> bool {
        self.factories.contains_key(kind)
    }

    fn create(&self, config: &BrainConfig) -> Result<Box<dyn ErasedBrainModel>, RegistryError> {
        validate_model_resource_limits(config)?;
        let factory =
            self.factories
                .get(&config.model.kind)
                .ok_or_else(|| RegistryError::UnknownModel {
                    kind: config.model.kind.clone(),
                })?;
        factory(config)
    }

    fn register_builtins(&mut self) {
        let _ = self.register::<Habituation<FlyHashEncoder, DecayingMemory>, _, _>(
            "fly",
            |brain| match &brain.model.settings {
                ModelSettings::Fly(config) => {
                    if brain.model.version != config.model_version
                        || brain.input.schema_version != config.feature_schema_version
                    {
                        return Err(RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: "Fly model and brain versions do not match".to_owned(),
                        });
                    }
                    if brain.model.seed.is_some_and(|seed| seed != config.seed) {
                        return Err(RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: "Fly model seed does not match its configuration".to_owned(),
                        });
                    }
                    Habituation::<FlyHashEncoder, DecayingMemory>::with_config(config.clone())
                        .map_err(|error| RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: error.to_string(),
                        })
                }
                _ => Err(RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "fly requires ModelSettings::Fly".to_owned(),
                }),
            },
            fly_report,
        );
        let _ = self.register::<NumericDeviationModel, _, _>(
            "numeric",
            |brain| match &brain.model.settings {
                ModelSettings::Numeric { names, config } => {
                    if brain.model.version != config.model_version
                        || brain.input.schema_version != config.feature_schema_version
                    {
                        return Err(RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: "numeric model and brain versions do not match".to_owned(),
                        });
                    }
                    NumericDeviationModel::with_config(names.clone(), config.clone()).map_err(
                        |error| RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: error.to_string(),
                        },
                    )
                }
                _ => Err(RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "numeric requires ModelSettings::Numeric".to_owned(),
                }),
            },
            numeric_report,
        );
        let _ = self.register::<NearestNeighborModel, _, _>(
            "nearest_neighbor",
            |brain| match &brain.model.settings {
                ModelSettings::NearestNeighbor(config) => {
                    if brain.model.version != config.model_version
                        || brain.input.schema_version != config.feature_schema_version
                    {
                        return Err(RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: "nearest-neighbor model and brain versions do not match"
                                .to_owned(),
                        });
                    }
                    NearestNeighborModel::new(config.clone()).map_err(|error| {
                        RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: error.to_string(),
                        }
                    })
                }
                _ => Err(RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "nearest_neighbor requires ModelSettings::NearestNeighbor".to_owned(),
                }),
            },
            nearest_report,
        );
        let _ = self.register::<FailureModel, _, _>(
            "failure",
            |brain| match &brain.model.settings {
                ModelSettings::Failure(config) => {
                    if brain.model.version != config.model_version
                        || brain.input.schema_version != config.feature_schema_version
                    {
                        return Err(RegistryError::Model {
                            brain: brain.id.clone(),
                            reason: "failure model and brain versions do not match".to_owned(),
                        });
                    }
                    FailureModel::new(*config).map_err(|error| RegistryError::Model {
                        brain: brain.id.clone(),
                        reason: error.to_string(),
                    })
                }
                _ => Err(RegistryError::Model {
                    brain: brain.id.clone(),
                    reason: "failure requires ModelSettings::Failure".to_owned(),
                }),
            },
            failure_report,
        );
    }
}

fn fly_report<U>(evaluation: &Evaluation<FlyDiagnostics, U>, config: &BrainConfig) -> ModelReport {
    let metrics = vec![
        Metric::new(
            "novelty_short",
            evaluation.novelty_short.unwrap_or(0.0),
            "ratio",
            MetricDirection::HigherIsMore,
        ),
        Metric::new(
            "novelty_long",
            evaluation.novelty_long.unwrap_or(0.0),
            "ratio",
            MetricDirection::HigherIsMore,
        ),
        Metric::new(
            "input_strength",
            evaluation.diagnostics.input_strength,
            "normalized-rms",
            MetricDirection::Informational,
        ),
        Metric::new(
            "profile_deviation",
            evaluation.diagnostics.profile_deviation,
            "ratio",
            MetricDirection::HigherIsMore,
        ),
    ];
    let evidence = vec![Evidence {
        name: "fly-memory".to_owned(),
        current: None,
        baseline: None,
        sample_count: None,
        window: None,
        reference_ids: Vec::new(),
        detail: "短期・長期の疎なユニット記憶と特徴量 profile を参照".to_owned(),
    }];
    let mut report = ModelReport {
        novelty_short: evaluation.novelty_short,
        novelty_long: evaluation.novelty_long,
        familiarity_short: evaluation.familiarity_short,
        familiarity_long: evaluation.familiarity_long,
        recency: evaluation.recency,
        pattern_id: evaluation.pattern_id,
        readiness: evaluation.readiness.clone(),
        // Novelty describes the model's familiarity with a pattern. It is not
        // an assertion that the input must be excluded from a normal baseline:
        // a repeated-normal learning policy may deliberately learn a new
        // stable pattern while still reporting Novel to the caller.
        learning_signal: match evaluation.readiness {
            Readiness::Evaluated => LearningSignal::Normal,
            _ => LearningSignal::Unknown,
        },
        metrics,
        evidence,
        reactions: Vec::new(),
    };
    if let Some(score) = evaluation.novelty_short
        && score >= config.reactions.threshold
        && allows_kind(&config.reactions, &ReactionKind::Novel)
    {
        report.reactions.push(Reaction {
            brain: config.id.clone(),
            target: config.input.target.clone(),
            kind: ReactionKind::Novel,
            strength: None,
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            readiness: report.readiness.clone(),
        });
    }
    report
}

fn numeric_report<U>(
    evaluation: &Evaluation<NumericDeviationDiagnostics, U>,
    config: &BrainConfig,
) -> ModelReport {
    let metrics = evaluation
        .diagnostics
        .deviations
        .iter()
        .map(numeric_metric)
        .collect::<Vec<_>>();
    let evidence = evaluation
        .diagnostics
        .deviations
        .iter()
        .map(numeric_evidence)
        .collect::<Vec<_>>();
    let mut report = ModelReport {
        novelty_short: None,
        novelty_long: None,
        familiarity_short: None,
        familiarity_long: None,
        recency: evaluation.recency,
        pattern_id: evaluation.pattern_id,
        readiness: evaluation.readiness.clone(),
        learning_signal: if evaluation.readiness == Readiness::Evaluated {
            if evaluation
                .diagnostics
                .deviations
                .iter()
                .any(|deviation| deviation.z_plus >= config.reactions.threshold)
            {
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
    let has_deviation = evaluation
        .diagnostics
        .deviations
        .iter()
        .any(|deviation| deviation.z_plus >= config.reactions.threshold);
    if has_deviation && allows_kind(&config.reactions, &ReactionKind::Deviation) {
        report.reactions.push(Reaction {
            brain: config.id.clone(),
            target: config.input.target.clone(),
            kind: ReactionKind::Deviation,
            strength: None,
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            readiness: report.readiness.clone(),
        });
    }
    report
}

fn nearest_report<U>(
    evaluation: &Evaluation<NearestNeighborDiagnostics, U>,
    config: &BrainConfig,
) -> ModelReport {
    let distance = evaluation.diagnostics.nearest_distance;
    let mut metrics = Vec::new();
    if let Some(value) = distance {
        metrics.push(Metric::new(
            "cosine_distance",
            value,
            "distance",
            MetricDirection::HigherIsMore,
        ));
    }
    for (name, value) in [
        ("cosine_distance_mean", evaluation.diagnostics.distance_mean),
        (
            "cosine_distance_median",
            evaluation.diagnostics.distance_median,
        ),
        ("cosine_distance_max", evaluation.diagnostics.distance_max),
    ] {
        if let Some(value) = value {
            metrics.push(Metric::new(
                name,
                value,
                "distance",
                MetricDirection::Informational,
            ));
        }
    }
    let evidence = vec![Evidence {
        name: "nearest-representative".to_owned(),
        current: distance,
        baseline: Some(0.0),
        sample_count: Some(evaluation.diagnostics.representative_count),
        window: None,
        reference_ids: Vec::new(),
        detail: format!(
            "保持代表例 {}/{} 件との最近傍コサイン距離",
            evaluation.diagnostics.representative_count,
            evaluation.diagnostics.maximum_representatives
        ),
    }];
    let mut report = ModelReport {
        novelty_short: None,
        novelty_long: None,
        familiarity_short: None,
        familiarity_long: None,
        recency: evaluation.recency,
        pattern_id: evaluation.pattern_id,
        readiness: evaluation.readiness.clone(),
        // A nearest-neighbor distance is a novelty diagnostic, not a learning
        // veto. The learning policy decides whether a newly seen example is
        // stable enough to retain.
        learning_signal: match evaluation.readiness {
            Readiness::Evaluated => LearningSignal::Normal,
            _ => LearningSignal::Unknown,
        },
        metrics,
        evidence,
        reactions: Vec::new(),
    };
    if let Some(distance) = distance
        && distance >= config.reactions.threshold
        && allows_kind(&config.reactions, &ReactionKind::Novel)
    {
        report.reactions.push(Reaction {
            brain: config.id.clone(),
            target: config.input.target.clone(),
            kind: ReactionKind::Novel,
            strength: None,
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            readiness: report.readiness.clone(),
        });
    }
    report
}

fn failure_report(
    evaluation: &Evaluation<FailureDiagnostics, FailureUpdate>,
    config: &BrainConfig,
) -> ModelReport {
    let result = &evaluation.diagnostics.result;
    let failed = if result.success { 0.0 } else { 1.0 };
    let metrics = vec![Metric::new(
        "failed",
        failed,
        "boolean",
        MetricDirection::HigherIsMore,
    )];
    let mut detail = result
        .result_code
        .as_deref()
        .unwrap_or("no-result-code")
        .to_owned();
    if let Some(error_type) = &result.error_type {
        detail.push_str(" error_type=");
        detail.push_str(error_type);
    }
    let mut evidence = vec![Evidence::text("operation-result", detail)];
    if let Some(state) = &result.state {
        evidence.push(Evidence::text("operation-state", state.clone()));
    }
    let mut report = ModelReport {
        novelty_short: None,
        novelty_long: None,
        familiarity_short: None,
        familiarity_long: None,
        recency: None,
        pattern_id: None,
        readiness: evaluation.readiness.clone(),
        learning_signal: if evaluation.readiness == Readiness::Evaluated {
            if result.success {
                LearningSignal::Normal
            } else {
                LearningSignal::Abnormal
            }
        } else {
            LearningSignal::Unknown
        },
        metrics,
        evidence,
        reactions: Vec::new(),
    };
    if !result.success && allows_kind(&config.reactions, &ReactionKind::Failure) {
        report.reactions.push(Reaction {
            brain: config.id.clone(),
            target: config.input.target.clone(),
            kind: ReactionKind::Failure,
            strength: None,
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            readiness: report.readiness.clone(),
        });
    }
    report
}

fn numeric_metric(deviation: &NamedDeviation) -> Metric {
    Metric::new(
        format!("{}.z_plus", deviation.name),
        deviation.z_plus,
        "z",
        // z_plus is a standardized distance, so a larger z is always more
        // deviant. The configured side is retained in the evidence below.
        MetricDirection::HigherIsMore,
    )
}

fn numeric_evidence(deviation: &NamedDeviation) -> Evidence {
    Evidence {
        name: deviation.name.clone(),
        current: Some(deviation.current),
        baseline: deviation.baseline,
        sample_count: Some(deviation.sample_count),
        window: Some(deviation.window),
        reference_ids: Vec::new(),
        detail: format!(
            "中央値と MAD による時間窓内の基準（方向: {:?}）",
            deviation.direction
        ),
    }
}

fn allows_kind(policy: &ReactionPolicy, kind: &ReactionKind) -> bool {
    policy.kinds.iter().any(|allowed| allowed == kind)
}

fn metric_value(evaluation: &BrainEvaluation, name: &str) -> Option<f32> {
    evaluation
        .report
        .metrics
        .iter()
        .find(|metric| metric.name == name)
        .map(|metric| metric.value)
}

fn validate_evaluation<D, U>(evaluation: &Evaluation<D, U>) -> Result<(), ModelError> {
    for value in [
        evaluation.novelty_short,
        evaluation.novelty_long,
        evaluation.familiarity_short,
        evaluation.familiarity_long,
    ]
    .into_iter()
    .flatten()
    {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ModelError::Incompatible(
                "model evaluation score is outside 0..=1 or not finite".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_report(report: &ModelReport, config: &BrainConfig) -> Result<(), ModelError> {
    if report.readiness != Readiness::Evaluated && report.learning_signal != LearningSignal::Unknown
    {
        return Err(ModelError::Incompatible(
            "non-evaluated model report must have an unknown learning signal".to_owned(),
        ));
    }
    for value in [
        report.novelty_short,
        report.novelty_long,
        report.familiarity_short,
        report.familiarity_long,
    ]
    .into_iter()
    .flatten()
    {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ModelError::Incompatible(
                "model report score is outside 0..=1 or not finite".to_owned(),
            ));
        }
    }
    for metric in &report.metrics {
        if !metric.value.is_finite() {
            return Err(ModelError::Incompatible(
                "model report contains a non-finite metric".to_owned(),
            ));
        }
    }
    for evidence in &report.evidence {
        for value in [evidence.current, evidence.baseline].into_iter().flatten() {
            if !value.is_finite() {
                return Err(ModelError::Incompatible(
                    "model report contains non-finite evidence".to_owned(),
                ));
            }
        }
    }
    for reaction in &report.reactions {
        if reaction.brain != config.id || reaction.target != config.input.target {
            return Err(ModelError::Incompatible(
                "reaction owner does not match the brain configuration".to_owned(),
            ));
        }
        if let Some(strength) = reaction.strength
            && !strength.is_finite()
        {
            return Err(ModelError::Incompatible(
                "reaction strength is not finite".to_owned(),
            ));
        }
        if reaction.readiness != report.readiness {
            return Err(ModelError::Incompatible(
                "reaction readiness does not match the model report".to_owned(),
            ));
        }
        for metric in &reaction.metrics {
            if !metric.value.is_finite() {
                return Err(ModelError::Incompatible(
                    "reaction contains a non-finite metric".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn reaction_kind_tag(kind: &ReactionKind) -> u64 {
    match kind {
        ReactionKind::Novel => 0,
        ReactionKind::Deviation => 1,
        ReactionKind::Absence => 2,
        ReactionKind::Repeat => 3,
        ReactionKind::Failure => 4,
    }
}

struct BrainInstance {
    config: BrainConfig,
    model: Box<dyn ErasedBrainModel>,
    state: BrainEvaluationState,
    stream_generations: BTreeMap<String, u64>,
    normal_streaks: BTreeMap<String, u32>,
    known_streams: BTreeMap<String, ()>,
    last_positive_commit: BTreeMap<String, Timestamp>,
    positive_commit_history: VecDeque<Timestamp>,
}

#[derive(Clone, Copy)]
enum NormalStreakAction {
    None,
    Advance,
    Reset,
}

/// Runs heterogeneous registered models against the same finalized inputs.
pub struct BrainRunner {
    brains: BTreeMap<String, BrainInstance>,
    runner_token: u64,
    consensus_policy: Option<ConsensusPolicy>,
}

impl BrainRunner {
    /// Constructs a runner from brain configurations and a model registry.
    pub fn new<I>(configs: I, registry: &ModelRegistry) -> Result<Self, RegistryError>
    where
        I: IntoIterator<Item = BrainConfig>,
    {
        let mut brains = BTreeMap::new();
        for config in configs {
            config.validate()?;
            if brains.len() >= MAX_BRAIN_COUNT {
                return Err(RegistryError::InvalidConfiguration {
                    brain: config.id.clone(),
                    reason: "brain count exceeds the runner limit".to_owned(),
                });
            }
            if brains.contains_key(&config.id) {
                return Err(RegistryError::DuplicateBrain {
                    brain: config.id.clone(),
                });
            }
            let model = registry.create(&config)?;
            let identity = model.identity();
            if identity.model_version != config.model.version
                || identity.feature_schema_version != config.input.schema_version
            {
                return Err(RegistryError::Model {
                    brain: config.id.clone(),
                    reason: "model identity does not match brain input or model version".to_owned(),
                });
            }
            if config.state.format_version != 0
                && model.state_format_version() != config.state.format_version
            {
                return Err(RegistryError::IncompatibleState {
                    brain: config.id.clone(),
                    reason: "configured state format does not match model".to_owned(),
                });
            }
            let state = BrainEvaluationState {
                brain: config.id.clone(),
                role: config.role.clone(),
                enabled: config.enabled,
                mode: config.mode,
                readiness: if config.enabled {
                    Readiness::BaselineInsufficient
                } else {
                    Readiness::InputMissing
                },
                evaluation_count: 0,
                last_arrival: None,
                last_processed: None,
                last_position: None,
                learning_enabled: config.learning.mode != LearningMode::Manual,
                last_learning_weight: None,
                learning_count: 0,
                last_error: None,
            };
            brains.insert(
                config.id.clone(),
                BrainInstance {
                    config,
                    model,
                    state,
                    stream_generations: BTreeMap::new(),
                    normal_streaks: BTreeMap::new(),
                    known_streams: BTreeMap::new(),
                    last_positive_commit: BTreeMap::new(),
                    positive_commit_history: VecDeque::new(),
                },
            );
        }
        Ok(Self {
            brains,
            runner_token: NEXT_RUNNER_TOKEN.fetch_add(1, Ordering::Relaxed),
            consensus_policy: None,
        })
    }

    /// Constructs a runner whose external reactions are selected by consensus.
    ///
    /// Individual model reactions remain in
    /// [`OrchestrationEvaluation::active_reactions`] for diagnostics. Only
    /// [`OrchestrationEvaluation::external_reactions`] can be used as the
    /// caller-facing reaction list.
    pub fn new_with_consensus_policy<I>(
        configs: I,
        registry: &ModelRegistry,
        consensus_policy: ConsensusPolicy,
    ) -> Result<Self, RegistryError>
    where
        I: IntoIterator<Item = BrainConfig>,
    {
        let mut runner = Self::new(configs, registry)?;
        runner.consensus_policy = Some(consensus_policy);
        Ok(runner)
    }

    /// Returns a runner containing the built-in models for `configs`.
    pub fn with_builtins<I>(configs: I) -> Result<Self, RegistryError>
    where
        I: IntoIterator<Item = BrainConfig>,
    {
        Self::new(configs, &ModelRegistry::with_builtins())
    }

    /// Returns the current state of every configured brain.
    pub fn states(&self) -> Vec<BrainEvaluationState> {
        self.brains
            .values()
            .map(|brain| brain.state.clone())
            .collect()
    }

    /// Returns the state of one brain.
    pub fn state(&self, brain: &str) -> Option<&BrainEvaluationState> {
        self.brains.get(brain).map(|instance| &instance.state)
    }

    /// Returns a brain configuration.
    pub fn brain_config(&self, brain: &str) -> Option<&BrainConfig> {
        self.brains.get(brain).map(|instance| &instance.config)
    }

    /// Evaluates all matching enabled brains before any update is committed.
    ///
    /// Model state is not changed by this method. The returned updates can be
    /// committed separately, allowing the caller to hold or reduce learning.
    pub fn evaluate_all(
        &mut self,
        input: &BrainInput,
    ) -> Result<OrchestrationEvaluation, RunnerError> {
        let processed_at = input.processed_at.unwrap_or(input.available_at);
        if processed_at < input.available_at {
            return Err(RunnerError::InvalidInput {
                reason: "processed_at must not precede available_at".to_owned(),
            });
        }
        if self
            .brains
            .values()
            .filter_map(|instance| instance.state.last_position)
            .any(|position| input.position < position)
        {
            return Err(RunnerError::InvalidInput {
                reason: "adapter processing position moved backwards".to_owned(),
            });
        }
        if input.stream_id.is_empty() || input.context.is_empty() {
            return Err(RunnerError::InvalidInput {
                reason: "stream_id and context must be non-empty".to_owned(),
            });
        }
        for instance in self.brains.values() {
            let config = &instance.config;
            if !config.enabled
                || input.target != config.input.target
                || input.context != config.input.context
            {
                continue;
            }
            if input.schema_id != config.input.schema_id
                || input.schema_version != config.input.schema_version
            {
                return Err(RunnerError::SchemaMismatch {
                    brain: config.id.clone(),
                });
            }
            if let Some(expected) = configured_dimensions(&config.model.settings)
                && expected != input.features.len()
            {
                return Err(RunnerError::DimensionMismatch {
                    brain: config.id.clone(),
                    expected,
                    actual: input.features.len(),
                });
            }
        }
        let mut evaluations = Vec::with_capacity(self.brains.len());

        for instance in self.brains.values_mut() {
            let config = instance.config.clone();
            if !config.enabled {
                instance.state.readiness = Readiness::InputMissing;
                evaluations.push(BrainEvaluation {
                    brain: config.id.clone(),
                    role: config.role.clone(),
                    mode: config.mode,
                    target: config.input.target.clone(),
                    report: empty_report(Readiness::InputMissing, "brain is disabled"),
                    recommended_learning_weight: 0.0,
                    processing_time: Duration::ZERO,
                    update: None,
                });
                continue;
            }
            if input.target != config.input.target || input.context != config.input.context {
                instance.state.readiness = Readiness::InputMissing;
                instance.state.evaluation_count = instance.state.evaluation_count.saturating_add(1);
                instance.state.last_arrival = Some(input.available_at);
                instance.state.last_processed = Some(processed_at);
                instance.state.last_position = Some(input.position);
                evaluations.push(BrainEvaluation {
                    brain: config.id.clone(),
                    role: config.role.clone(),
                    mode: config.mode,
                    target: config.input.target.clone(),
                    report: empty_report(Readiness::InputMissing, "target is not present in input"),
                    recommended_learning_weight: 0.0,
                    processing_time: Duration::ZERO,
                    update: None,
                });
                continue;
            }
            if input.schema_id != config.input.schema_id
                || input.schema_version != config.input.schema_version
            {
                instance.state.readiness = Readiness::Invalid;
                instance.state.last_error = Some("input schema does not match".to_owned());
                instance.normal_streaks.remove(&input.stream_id);
                return Err(RunnerError::SchemaMismatch {
                    brain: config.id.clone(),
                });
            }
            if let Some(deadline) = config.resources.evaluation_deadline
                && processed_at
                    .checked_duration_since(input.available_at)
                    .is_some_and(|elapsed| elapsed > deadline)
            {
                instance.state.readiness = Readiness::Expired;
                instance.normal_streaks.remove(&input.stream_id);
                instance.state.evaluation_count = instance.state.evaluation_count.saturating_add(1);
                instance.state.last_arrival = Some(input.available_at);
                instance.state.last_processed = Some(processed_at);
                instance.state.last_position = Some(input.position);
                evaluations.push(BrainEvaluation {
                    brain: config.id.clone(),
                    role: config.role.clone(),
                    mode: config.mode,
                    target: config.input.target.clone(),
                    report: empty_report(Readiness::Expired, "evaluation deadline exceeded"),
                    recommended_learning_weight: 0.0,
                    processing_time: Duration::ZERO,
                    update: None,
                });
                continue;
            }
            let is_failure_model = config.model.kind == "failure";
            if is_failure_model && input.failure.is_none() {
                instance.state.readiness = Readiness::InputMissing;
                instance.normal_streaks.remove(&input.stream_id);
                instance.state.last_error = Some("structured result is missing".to_owned());
                instance.state.evaluation_count = instance.state.evaluation_count.saturating_add(1);
                instance.state.last_arrival = Some(input.available_at);
                instance.state.last_processed = Some(processed_at);
                instance.state.last_position = Some(input.position);
                evaluations.push(BrainEvaluation {
                    brain: config.id.clone(),
                    role: config.role.clone(),
                    mode: config.mode,
                    target: config.input.target.clone(),
                    report: empty_report(Readiness::InputMissing, "structured result is missing"),
                    recommended_learning_weight: 0.0,
                    processing_time: Duration::ZERO,
                    update: None,
                });
                continue;
            }
            if !is_failure_model
                && (input.features.is_empty()
                    || input.features.iter().any(|value| !value.is_finite()))
            {
                instance.state.readiness = Readiness::Invalid;
                instance.normal_streaks.remove(&input.stream_id);
                instance.state.last_error =
                    Some("features must be non-empty and finite".to_owned());
                instance.state.evaluation_count = instance.state.evaluation_count.saturating_add(1);
                instance.state.last_arrival = Some(input.available_at);
                instance.state.last_processed = Some(processed_at);
                instance.state.last_position = Some(input.position);
                evaluations.push(BrainEvaluation {
                    brain: config.id.clone(),
                    role: config.role.clone(),
                    mode: config.mode,
                    target: config.input.target.clone(),
                    report: empty_report(
                        Readiness::Invalid,
                        "features must be non-empty and finite",
                    ),
                    recommended_learning_weight: 0.0,
                    processing_time: Duration::ZERO,
                    update: None,
                });
                continue;
            }
            if !instance.known_streams.contains_key(&input.stream_id)
                && instance.known_streams.len() >= config.resources.max_streams
            {
                return Err(RunnerError::ResourceLimit {
                    brain: config.id.clone(),
                    reason: "max_streams was reached".to_owned(),
                });
            }
            let started = std::time::Instant::now();
            let result = instance.model.evaluate(&config, input);
            let processing_time = started.elapsed();
            instance.state.evaluation_count = instance.state.evaluation_count.saturating_add(1);
            instance.state.last_arrival = Some(input.available_at);
            instance.state.last_processed = Some(processed_at);
            instance.state.last_position = Some(input.position);
            match result {
                Ok((report, update)) => {
                    instance.state.readiness = report.readiness.clone();
                    instance.state.last_error = None;
                    let normal_streak_action = instance.normal_streak_action(&report);
                    let projected_normal_streak =
                        instance.projected_normal_streak(&input.stream_id, normal_streak_action);
                    let recommended_learning_weight = instance.recommended_weight(
                        &input.stream_id,
                        input.available_at,
                        &report.readiness,
                        projected_normal_streak,
                    );
                    evaluations.push(BrainEvaluation {
                        brain: config.id.clone(),
                        role: config.role.clone(),
                        mode: config.mode,
                        target: config.input.target.clone(),
                        report,
                        recommended_learning_weight,
                        processing_time,
                        update: Some(BrainUpdate {
                            brain: config.id.clone(),
                            runner_token: self.runner_token,
                            configuration_fingerprint: config.configuration_fingerprint(),
                            stream_id: input.stream_id.clone(),
                            at: input.available_at,
                            generation: instance
                                .stream_generations
                                .get(&input.stream_id)
                                .copied()
                                .unwrap_or(0),
                            normal_streak_action,
                            update,
                        }),
                    });
                }
                Err(error) => {
                    instance.state.readiness = Readiness::Error;
                    instance.state.last_error = Some(error.to_string());
                    instance.normal_streaks.remove(&input.stream_id);
                    evaluations.push(BrainEvaluation {
                        brain: config.id.clone(),
                        role: config.role.clone(),
                        mode: config.mode,
                        target: config.input.target.clone(),
                        report: empty_report(Readiness::Error, &error.to_string()),
                        recommended_learning_weight: 0.0,
                        processing_time,
                        update: None,
                    });
                }
            }
        }
        let active_reactions = evaluations
            .iter()
            .filter(|evaluation| {
                evaluation.mode == BrainMode::Active
                    && evaluation.report.readiness == Readiness::Evaluated
            })
            .flat_map(|evaluation| evaluation.report.reactions.iter())
            .filter(|reaction| reaction.readiness == Readiness::Evaluated)
            .cloned()
            .collect();
        let (consensus_action, external_reactions) = self.select_external_reactions(&evaluations);
        Ok(OrchestrationEvaluation {
            input_id: input.id.clone(),
            available_at: input.available_at,
            brains: evaluations,
            active_reactions,
            consensus_action,
            external_reactions,
        })
    }

    fn select_external_reactions(
        &self,
        evaluations: &[BrainEvaluation],
    ) -> (ConsensusAction, Vec<Reaction>) {
        let Some(policy) = &self.consensus_policy else {
            return (ConsensusAction::Hold, Vec::new());
        };
        let Some(change) = evaluations
            .iter()
            .find(|evaluation| evaluation.role == policy.change_role)
        else {
            return (ConsensusAction::Hold, Vec::new());
        };
        let Some(no_change) = evaluations
            .iter()
            .find(|evaluation| evaluation.role == policy.no_change_role)
        else {
            return (ConsensusAction::Hold, Vec::new());
        };
        if change.report.readiness != Readiness::Evaluated
            || no_change.report.readiness != Readiness::Evaluated
        {
            return (ConsensusAction::Hold, Vec::new());
        }
        let Some(change_probability) = metric_value(change, &policy.probability_metric) else {
            return (ConsensusAction::Hold, Vec::new());
        };
        let Some(no_change_probability) = metric_value(no_change, &policy.probability_metric)
        else {
            return (ConsensusAction::Hold, Vec::new());
        };
        if !change_probability.is_finite() || !no_change_probability.is_finite() {
            return (ConsensusAction::Hold, Vec::new());
        }
        let threshold = policy.threshold_value();
        let margin = policy.margin_value();
        let action = if change_probability >= threshold + margin
            && no_change_probability <= threshold - margin
        {
            ConsensusAction::NotifyChange
        } else if change_probability <= threshold - margin
            && no_change_probability >= threshold + margin
        {
            ConsensusAction::SuppressNoChange
        } else {
            ConsensusAction::Hold
        };
        let selected = match action {
            ConsensusAction::NotifyChange => (change, policy.change_reaction.clone()),
            ConsensusAction::SuppressNoChange => (no_change, policy.no_change_reaction.clone()),
            ConsensusAction::Hold => return (ConsensusAction::Hold, Vec::new()),
        };
        let reaction = Reaction {
            brain: format!("consensus:{}", selected.0.brain),
            target: selected.0.target.clone(),
            kind: selected.1,
            strength: Some(if action == ConsensusAction::NotifyChange {
                change_probability
            } else {
                no_change_probability
            }),
            metrics: selected.0.report.metrics.clone(),
            evidence: selected.0.report.evidence.clone(),
            readiness: Readiness::Evaluated,
        };
        (action, vec![reaction])
    }

    /// Commits an update after checking its brain, configuration, stream, and generation.
    pub fn commit(
        &mut self,
        update: BrainUpdate,
        weight: f32,
    ) -> Result<CommitRecord, RunnerError> {
        let BrainUpdate {
            brain,
            runner_token,
            configuration_fingerprint,
            stream_id,
            at,
            generation,
            normal_streak_action,
            update,
        } = update;
        let instance = self
            .brains
            .get_mut(&brain)
            .ok_or(RunnerError::ForeignUpdate)?;
        let actual_generation = instance
            .stream_generations
            .get(&stream_id)
            .copied()
            .unwrap_or(0);
        if runner_token != self.runner_token
            || instance.config.configuration_fingerprint() != configuration_fingerprint
            || actual_generation != generation
        {
            return Err(
                if runner_token != self.runner_token
                    || instance.config.configuration_fingerprint() != configuration_fingerprint
                {
                    RunnerError::ForeignUpdate
                } else {
                    RunnerError::StaleUpdate
                },
            );
        }
        crate::model::validate_learning_weight(weight).map_err(|source| RunnerError::Model {
            brain: brain.clone(),
            source,
        })?;
        if weight > 0.0 && !instance.learning_frequency_available(at) {
            return Err(RunnerError::ResourceLimit {
                brain,
                reason: "learning frequency limit was reached".to_owned(),
            });
        }
        let next_generation = generation
            .checked_add(1)
            .ok_or_else(|| RunnerError::Model {
                brain: brain.clone(),
                source: ModelError::UpdateSequenceOverflow,
            })?;
        if let Err(source) = instance.model.commit(update, weight) {
            instance.state.last_error = Some(source.to_string());
            if matches!(source, ModelError::ForeignUpdate) {
                return Err(RunnerError::ForeignUpdate);
            }
            return Err(RunnerError::Model {
                brain: brain.clone(),
                source,
            });
        }
        instance
            .stream_generations
            .insert(stream_id.clone(), next_generation);
        instance.known_streams.insert(stream_id.clone(), ());
        instance.apply_normal_streak(&stream_id, normal_streak_action);
        instance.state.learning_count = instance.state.learning_count.saturating_add(1);
        instance.state.last_learning_weight = Some(weight);
        instance.state.learning_enabled = instance.config.learning.mode != LearningMode::Manual;
        if weight > 0.0 {
            instance.last_positive_commit.insert(stream_id, at);
            if instance.config.learning.frequency_limit.is_some() {
                instance.positive_commit_history.push_back(at);
                instance.prune_frequency_history(at);
            }
        }
        Ok(CommitRecord {
            brain,
            weight,
            learned: weight > 0.0,
        })
    }

    /// Commits all updates using each brain's recommended weight.
    pub fn commit_all(
        &mut self,
        evaluation: OrchestrationEvaluation,
    ) -> Result<Vec<CommitRecord>, RunnerError> {
        let mut records = Vec::new();
        for brain_evaluation in evaluation.brains {
            if let Some(update) = brain_evaluation.update {
                records.push(self.commit(update, brain_evaluation.recommended_learning_weight)?);
            }
        }
        Ok(records)
    }

    /// Removes a brain and its in-memory state.
    pub fn remove_brain(&mut self, brain: &str) -> bool {
        if let Some(instance) = self.brains.get_mut(brain)
            && instance.config.state.deletion == DeletionPolicy::Retain
        {
            instance.config.enabled = false;
            instance.state.enabled = false;
            instance.state.readiness = Readiness::InputMissing;
            return true;
        }
        self.brains.remove(brain).is_some()
    }

    /// Changes enabled state while retaining the brain's model state.
    pub fn set_enabled(&mut self, brain: &str, enabled: bool) -> Result<(), RunnerError> {
        let instance = self
            .brains
            .get_mut(brain)
            .ok_or(RunnerError::ForeignUpdate)?;
        instance.config.enabled = enabled;
        instance.state.enabled = enabled;
        if !enabled {
            instance.state.readiness = Readiness::InputMissing;
        }
        Ok(())
    }

    /// Promotes a shadow brain by copying state from a compatible shadow brain.
    pub fn promote_shadow(&mut self, shadow: &str, active: &str) -> Result<(), RunnerError> {
        let source = self.brains.get(shadow).ok_or(RunnerError::ForeignUpdate)?;
        if source.config.mode != BrainMode::Shadow {
            return Err(RunnerError::ForeignUpdate);
        }
        let mut payload = Vec::new();
        source
            .model
            .save_state(&mut payload)
            .map_err(|error| RunnerError::Registry(RegistryError::Io(error)))?;
        let source_fingerprint = source.model.fingerprint();
        let source_state_format_version = source.model.state_format_version();
        let source_state_compatibility = source.config.state_compatibility_fingerprint();
        let source_generations = source.stream_generations.clone();
        let source_known_streams = source.known_streams.clone();
        let source_last_positive_commit = source.last_positive_commit.clone();
        let source_history = source.positive_commit_history.clone();
        let source_normal_streaks = source.normal_streaks.clone();
        let source_state = source.state.clone();
        let target = self
            .brains
            .get_mut(active)
            .ok_or(RunnerError::ForeignUpdate)?;
        if target.config.mode != BrainMode::Active
            || target.model.fingerprint() != source_fingerprint
            || target.model.state_format_version() != source_state_format_version
            || target.config.state_compatibility_fingerprint() != source_state_compatibility
        {
            return Err(RunnerError::ForeignUpdate);
        }
        target
            .model
            .load_state(&mut Cursor::new(payload))
            .map_err(|error| {
                RunnerError::Registry(RegistryError::InvalidState(error.to_string()))
            })?;
        target.stream_generations = source_generations;
        target.known_streams = source_known_streams;
        target.last_positive_commit = source_last_positive_commit;
        target.positive_commit_history = source_history;
        target.normal_streaks = source_normal_streaks;
        target.state = BrainEvaluationState {
            brain: target.config.id.clone(),
            role: target.config.role.clone(),
            enabled: target.config.enabled,
            mode: target.config.mode,
            readiness: source_state.readiness,
            evaluation_count: source_state.evaluation_count,
            last_arrival: source_state.last_arrival,
            last_processed: source_state.last_processed,
            last_position: source_state.last_position,
            learning_enabled: source_state.learning_enabled,
            last_learning_weight: source_state.last_learning_weight,
            learning_count: source_state.learning_count,
            last_error: source_state.last_error,
        };
        Ok(())
    }

    fn brain_count(&self) -> usize {
        self.brains.len()
    }
}

impl BrainInstance {
    fn normal_streak_action(&self, report: &ModelReport) -> NormalStreakAction {
        let LearningMode::RepeatedNormal { .. } = self.config.learning.mode else {
            return NormalStreakAction::None;
        };
        let policy = self.config.learning;
        let can_count = match report.readiness {
            Readiness::Evaluated => match report.learning_signal {
                LearningSignal::Normal => true,
                LearningSignal::Abnormal => policy.learn_on_reaction,
                LearningSignal::Unknown => false,
            },
            Readiness::BaselineInsufficient => policy.bootstrap,
            Readiness::InputMissing
            | Readiness::Invalid
            | Readiness::Expired
            | Readiness::Error => false,
        };
        if can_count {
            NormalStreakAction::Advance
        } else {
            NormalStreakAction::Reset
        }
    }

    fn projected_normal_streak(&self, stream_id: &str, action: NormalStreakAction) -> u32 {
        match action {
            NormalStreakAction::Advance => self
                .normal_streaks
                .get(stream_id)
                .copied()
                .unwrap_or(0)
                .saturating_add(1),
            NormalStreakAction::None | NormalStreakAction::Reset => 0,
        }
    }

    fn apply_normal_streak(&mut self, stream_id: &str, action: NormalStreakAction) {
        match action {
            NormalStreakAction::None => {}
            NormalStreakAction::Advance => {
                let streak = self.normal_streaks.entry(stream_id.to_owned()).or_default();
                *streak = streak.saturating_add(1);
            }
            NormalStreakAction::Reset => {
                self.normal_streaks.remove(stream_id);
            }
        }
    }

    fn recommended_weight(
        &self,
        stream_id: &str,
        at: Timestamp,
        readiness: &Readiness,
        projected_normal_streak: u32,
    ) -> f32 {
        let policy = self.config.learning;
        if policy.require_evaluated && *readiness != Readiness::Evaluated && !policy.bootstrap {
            return 0.0;
        }
        if *readiness == Readiness::BaselineInsufficient {
            let bootstrap_allowed = policy.bootstrap
                || (!policy.require_evaluated
                    && matches!(policy.mode, LearningMode::EveryEvaluation));
            if !bootstrap_allowed {
                return 0.0;
            }
        } else if *readiness != Readiness::Evaluated {
            return 0.0;
        }
        // Bootstrap is stream-local. It covers the first configured number of
        // observations, including a Fly model's initial novelty and a
        // Numeric/nearest-neighbor model's BaselineInsufficient result.
        if let LearningMode::RepeatedNormal {
            consecutive_observations,
        } = policy.mode
            && policy.bootstrap
            && projected_normal_streak <= consecutive_observations
            && projected_normal_streak > 0
            && self.learning_frequency_available(at)
        {
            return policy.weight;
        }
        let mode_allows = match policy.mode {
            LearningMode::EveryEvaluation => true,
            LearningMode::RepeatedNormal {
                consecutive_observations,
            } => projected_normal_streak >= consecutive_observations,
            LearningMode::Revisit { interval } => self
                .last_positive_commit
                .get(stream_id)
                .and_then(|last| at.checked_duration_since(*last))
                .is_none_or(|elapsed| elapsed >= interval),
            LearningMode::Manual => false,
        };
        if mode_allows && self.learning_frequency_available(at) {
            policy.weight
        } else {
            0.0
        }
    }

    fn learning_frequency_available(&self, at: Timestamp) -> bool {
        let Some(limit) = self.config.learning.frequency_limit else {
            return true;
        };
        let count = self
            .positive_commit_history
            .iter()
            .filter(|timestamp| {
                at.checked_duration_since(**timestamp)
                    .is_some_and(|elapsed| elapsed <= limit.window)
            })
            .count();
        count < limit.maximum_commits as usize
    }

    fn prune_frequency_history(&mut self, at: Timestamp) {
        if let Some(limit) = self.config.learning.frequency_limit {
            while self
                .positive_commit_history
                .front()
                .is_some_and(|timestamp| {
                    at.checked_duration_since(*timestamp)
                        .is_some_and(|elapsed| elapsed > limit.window)
                })
            {
                self.positive_commit_history.pop_front();
            }
        }
    }
}

fn empty_report(readiness: Readiness, detail: &str) -> ModelReport {
    ModelReport {
        novelty_short: None,
        novelty_long: None,
        familiarity_short: None,
        familiarity_long: None,
        recency: None,
        pattern_id: None,
        readiness,
        learning_signal: LearningSignal::Unknown,
        metrics: Vec::new(),
        evidence: vec![Evidence::text("status", detail)],
        reactions: Vec::new(),
    }
}

fn configured_dimensions(settings: &ModelSettings) -> Option<usize> {
    match settings {
        ModelSettings::Fly(config) => Some(config.dimensions),
        ModelSettings::Numeric { names, .. } => Some(names.len()),
        ModelSettings::NearestNeighbor(config) => Some(config.dimensions),
        ModelSettings::Failure(_) => None,
        ModelSettings::Custom(_) => None,
    }
}

fn validate_model_resource_limits(config: &BrainConfig) -> Result<(), RegistryError> {
    let required_samples = match &config.model.settings {
        ModelSettings::Fly(settings) => settings.max_patterns,
        ModelSettings::Numeric {
            config: settings, ..
        } => settings.max_samples,
        ModelSettings::NearestNeighbor(settings) => settings.max_representatives,
        ModelSettings::Failure(_) => 0,
        ModelSettings::Custom(_) => return Ok(()),
    };
    if required_samples > config.resources.max_samples {
        return Err(RegistryError::InvalidConfiguration {
            brain: config.id.clone(),
            reason: format!(
                "model retains {required_samples} samples or representatives, above resource limit {}",
                config.resources.max_samples
            ),
        });
    }
    Ok(())
}

impl BrainRunner {
    /// Saves all brain states and runner bookkeeping in a versioned format.
    ///
    /// The same global allocation budget used by loading is checked before the
    /// first byte is written. A successful save is therefore guaranteed to be
    /// acceptable to [`Self::load`] when the same configurations and registry
    /// are supplied.
    pub fn save<W: Write>(&self, writer: &mut W) -> Result<(), RegistryError> {
        let mut save_plans = BTreeMap::new();
        let mut budget = LoadBudget::new();
        budget.reserve(self.brains.len()).map_err(state_io_error)?;
        for instance in self.brains.values() {
            let plan = preflight_saved_brain(instance, &mut budget)?;
            save_plans.insert(instance.config.id.clone(), plan);
        }
        writer.write_all(ORCHESTRATION_MAGIC)?;
        write_u16(writer, ORCHESTRATION_STATE_VERSION)?;
        write_u64(writer, self.brains.len() as u64)?;
        for instance in self.brains.values() {
            let state = &instance.state;
            write_string(writer, &instance.config.id)?;
            write_u64(writer, instance.config.configuration_fingerprint())?;
            write_u32(writer, instance.config.model.version)?;
            write_u32(writer, instance.config.input.schema_version)?;
            write_u16(writer, instance.model.state_format_version())?;
            write_u64(writer, instance.model.fingerprint())?;
            write_u64(writer, state.evaluation_count)?;
            write_optional_timestamp(writer, state.last_arrival)?;
            write_optional_timestamp(writer, state.last_processed)?;
            write_u8(writer, u8::from(state.last_position.is_some()))?;
            if let Some(position) = state.last_position {
                write_u64(writer, position)?;
            }
            write_u8(writer, u8::from(state.learning_enabled))?;
            write_u8(writer, u8::from(state.last_learning_weight.is_some()))?;
            if let Some(weight) = state.last_learning_weight {
                write_f32(writer, weight)?;
            }
            write_u64(writer, state.learning_count)?;
            write_u64(writer, instance.normal_streaks.len() as u64)?;
            for (stream, streak) in &instance.normal_streaks {
                write_string(writer, stream)?;
                write_u32(writer, *streak)?;
            }
            write_u8(writer, readiness_tag(&state.readiness))?;
            write_optional_string(writer, state.last_error.as_deref())?;
            write_u64(writer, instance.known_streams.len() as u64)?;
            for stream in instance.known_streams.keys() {
                write_string(writer, stream)?;
            }
            write_u64(writer, instance.stream_generations.len() as u64)?;
            for (stream, generation) in &instance.stream_generations {
                write_string(writer, stream)?;
                write_u64(writer, *generation)?;
            }
            write_u64(writer, instance.last_positive_commit.len() as u64)?;
            for (stream, timestamp) in &instance.last_positive_commit {
                write_string(writer, stream)?;
                write_timestamp(writer, *timestamp)?;
            }
            write_u64(writer, instance.positive_commit_history.len() as u64)?;
            for timestamp in &instance.positive_commit_history {
                write_timestamp(writer, *timestamp)?;
            }
            let plan = save_plans
                .get(&instance.config.id)
                .ok_or_else(|| RegistryError::InvalidState("missing save plan".to_owned()))?;
            write_u64(writer, plan.payload_length as u64)?;
            let mut limited = LimitedWriter::new(writer, plan.payload_length);
            // The preflight reserves the complete cumulative budget before
            // writing the header. The actual write is still performed through
            // the budget-aware model API, with a per-payload budget so a
            // custom model cannot allocate outside its declared persistence
            // contract while several brains are saved sequentially.
            let mut model_budget = LoadBudget::with_limit(plan.model_budget_elements);
            instance
                .model
                .save_state_with_budget(&mut limited, &mut model_budget)?;
            if limited.remaining() != 0 {
                return Err(RegistryError::InvalidState(
                    "model state size changed during save".to_owned(),
                ));
            }
            if model_budget.remaining() != 0 {
                return Err(RegistryError::InvalidState(
                    "model allocation plan changed during save".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Loads all brain states for the supplied configurations and registry.
    ///
    /// All models are constructed and validated before the returned runner is
    /// exposed. A failure never leaves a partially restored runner available to
    /// the caller.
    pub fn load<R: Read>(
        reader: &mut R,
        configs: &[BrainConfig],
        registry: &ModelRegistry,
    ) -> Result<Self, RegistryError> {
        let mut runner = Self::new(configs.to_vec(), registry)?;
        let mut magic = [0_u8; ORCHESTRATION_MAGIC.len()];
        reader.read_exact(&mut magic).map_err(state_io_error)?;
        if &magic != ORCHESTRATION_MAGIC {
            return Err(RegistryError::InvalidState(
                "invalid runner state marker".to_owned(),
            ));
        }
        let version = read_u16(reader).map_err(state_io_error)?;
        if version != ORCHESTRATION_LEGACY_STATE_VERSION
            && version != ORCHESTRATION_PREVIOUS_STATE_VERSION
            && version != ORCHESTRATION_THIRD_STATE_VERSION
            && version != ORCHESTRATION_STATE_VERSION
        {
            return Err(RegistryError::InvalidState(format!(
                "unsupported runner state version {version}"
            )));
        }
        let count = read_count(reader).map_err(state_io_error)?;
        if count != runner.brain_count() || count > MAX_BRAIN_COUNT {
            return Err(RegistryError::InvalidState(
                "saved brain count does not match configurations".to_owned(),
            ));
        }
        let mut budget = LoadBudget::new();
        budget.reserve(count).map_err(state_io_error)?;
        let mut loaded_ids = BTreeMap::new();
        for _ in 0..count {
            let id = read_string(reader, &mut budget).map_err(state_io_error)?;
            if loaded_ids.insert(id.clone(), ()).is_some() {
                return Err(RegistryError::InvalidState(
                    "duplicate saved brain id".to_owned(),
                ));
            }
            let instance =
                runner
                    .brains
                    .get_mut(&id)
                    .ok_or_else(|| RegistryError::IncompatibleState {
                        brain: id.clone(),
                        reason: "saved brain is not present in configuration".to_owned(),
                    })?;
            let configuration_fingerprint = read_u64(reader).map_err(state_io_error)?;
            let expected_configuration_fingerprint = if version >= ORCHESTRATION_STATE_VERSION {
                instance.config.configuration_fingerprint()
            } else {
                instance
                    .config
                    .configuration_fingerprint_for_format(version)
            };
            if configuration_fingerprint != expected_configuration_fingerprint {
                return Err(RegistryError::IncompatibleState {
                    brain: id,
                    reason: "configuration fingerprint differs".to_owned(),
                });
            }
            let model_version = read_u32(reader).map_err(state_io_error)?;
            let feature_schema_version = read_u32(reader).map_err(state_io_error)?;
            let state_format_version = read_u16(reader).map_err(state_io_error)?;
            let model_fingerprint = read_u64(reader).map_err(state_io_error)?;
            if model_version != instance.config.model.version
                || feature_schema_version != instance.config.input.schema_version
                || (instance.config.state.format_version != 0
                    && state_format_version != instance.config.state.format_version)
                || model_fingerprint != instance.model.fingerprint()
            {
                return Err(RegistryError::IncompatibleState {
                    brain: instance.config.id.clone(),
                    reason: "model or state identity differs".to_owned(),
                });
            }
            let legacy_generation = (version == ORCHESTRATION_LEGACY_STATE_VERSION)
                .then(|| read_u64(reader))
                .transpose()
                .map_err(state_io_error)?;
            let evaluation_count = read_u64(reader).map_err(state_io_error)?;
            let last_arrival = read_optional_timestamp(reader).map_err(state_io_error)?;
            let last_processed = read_optional_timestamp(reader).map_err(state_io_error)?;
            let last_position = read_optional_u64(reader).map_err(state_io_error)?;
            let learning_enabled = read_flag(reader, "learning_enabled")?;
            let last_learning_weight = if read_flag(reader, "last_learning_weight")? {
                let weight = read_f32(reader).map_err(state_io_error)?;
                if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
                    return Err(RegistryError::InvalidState(
                        "saved learning weight is invalid".to_owned(),
                    ));
                }
                Some(weight)
            } else {
                None
            };
            let learning_count = read_u64(reader).map_err(state_io_error)?;
            let legacy_normal_streak = (version < ORCHESTRATION_STATE_VERSION)
                .then(|| read_u32(reader))
                .transpose()
                .map_err(state_io_error)?;
            let normal_streaks = if version >= ORCHESTRATION_STATE_VERSION {
                let count = read_count(reader).map_err(state_io_error)?;
                if count > instance.config.resources.max_streams {
                    return Err(RegistryError::InvalidState(
                        "saved normal-streak stream count exceeds brain resource limit".to_owned(),
                    ));
                }
                budget.reserve(count).map_err(state_io_error)?;
                let mut streaks = BTreeMap::new();
                for _ in 0..count {
                    let stream = read_string(reader, &mut budget).map_err(state_io_error)?;
                    let streak = read_u32(reader).map_err(state_io_error)?;
                    if streaks.insert(stream, streak).is_some() {
                        return Err(RegistryError::InvalidState(
                            "duplicate saved normal-streak stream".to_owned(),
                        ));
                    }
                }
                streaks
            } else {
                // v1-v3 stored one brain-wide streak. It cannot be mapped
                // faithfully to stream-local bookkeeping, so model state is
                // preserved and only this derived counter is reset.
                let _ = legacy_normal_streak;
                BTreeMap::new()
            };
            let readiness = read_readiness(reader).map_err(state_io_error)?;
            let last_error = read_optional_string(reader, &mut budget).map_err(state_io_error)?;
            let known_count = read_count(reader).map_err(state_io_error)?;
            if known_count > instance.config.resources.max_streams {
                return Err(RegistryError::InvalidState(
                    "saved stream count exceeds brain resource limit".to_owned(),
                ));
            }
            budget.reserve(known_count).map_err(state_io_error)?;
            let mut known_streams = BTreeMap::new();
            for _ in 0..known_count {
                let stream = read_string(reader, &mut budget).map_err(state_io_error)?;
                if known_streams.insert(stream, ()).is_some() {
                    return Err(RegistryError::InvalidState(
                        "duplicate saved stream id".to_owned(),
                    ));
                }
            }
            if normal_streaks
                .keys()
                .any(|stream| !known_streams.contains_key(stream))
            {
                return Err(RegistryError::InvalidState(
                    "normal-streak stream is not present in saved streams".to_owned(),
                ));
            }
            let stream_generations = if version >= ORCHESTRATION_PREVIOUS_STATE_VERSION {
                let generation_count = read_count(reader).map_err(state_io_error)?;
                if generation_count != known_count {
                    return Err(RegistryError::InvalidState(
                        "saved stream generation count does not match stream count".to_owned(),
                    ));
                }
                budget.reserve(generation_count).map_err(state_io_error)?;
                let mut generations = BTreeMap::new();
                for _ in 0..generation_count {
                    let stream = read_string(reader, &mut budget).map_err(state_io_error)?;
                    let generation = read_u64(reader).map_err(state_io_error)?;
                    if !known_streams.contains_key(&stream)
                        || generations.insert(stream, generation).is_some()
                    {
                        return Err(RegistryError::InvalidState(
                            "invalid saved stream generation state".to_owned(),
                        ));
                    }
                }
                generations
            } else {
                known_streams
                    .keys()
                    .cloned()
                    .map(|stream| (stream, legacy_generation.unwrap_or(0)))
                    .collect()
            };
            let positive_count = read_count(reader).map_err(state_io_error)?;
            if positive_count > known_count {
                return Err(RegistryError::InvalidState(
                    "saved positive-commit stream count is invalid".to_owned(),
                ));
            }
            budget.reserve(positive_count).map_err(state_io_error)?;
            let mut last_positive_commit = BTreeMap::new();
            for _ in 0..positive_count {
                let stream = read_string(reader, &mut budget).map_err(state_io_error)?;
                let timestamp = read_timestamp(reader).map_err(state_io_error)?;
                if !known_streams.contains_key(&stream)
                    || last_positive_commit.insert(stream, timestamp).is_some()
                {
                    return Err(RegistryError::InvalidState(
                        "invalid positive-commit stream state".to_owned(),
                    ));
                }
            }
            let history_count = read_count(reader).map_err(state_io_error)?;
            if history_count > MAX_LOAD_ELEMENTS {
                return Err(RegistryError::InvalidState(
                    "saved learning history is too large".to_owned(),
                ));
            }
            budget.reserve(history_count).map_err(state_io_error)?;
            let mut positive_commit_history = VecDeque::with_capacity(history_count);
            for _ in 0..history_count {
                positive_commit_history.push_back(read_timestamp(reader).map_err(state_io_error)?);
            }
            let payload_length = read_count(reader).map_err(state_io_error)?;
            if payload_length > instance.config.resources.max_memory_bytes {
                return Err(RegistryError::InvalidState(
                    "saved model payload exceeds brain memory limit".to_owned(),
                ));
            }
            budget.reserve(payload_length).map_err(state_io_error)?;
            let mut limited = LimitedReader::new(reader, payload_length);
            instance
                .model
                .load_state_with_budget(&mut limited, &mut budget)
                .map_err(|error| RegistryError::IncompatibleState {
                    brain: instance.config.id.clone(),
                    reason: error.to_string(),
                })?;
            if limited.remaining() != 0 {
                return Err(RegistryError::InvalidState(
                    "model state payload was not fully consumed".to_owned(),
                ));
            }
            instance.stream_generations = stream_generations;
            instance.known_streams = known_streams;
            instance.last_positive_commit = last_positive_commit;
            instance.positive_commit_history = positive_commit_history;
            instance.normal_streaks = normal_streaks;
            instance.state = BrainEvaluationState {
                brain: instance.config.id.clone(),
                role: instance.config.role.clone(),
                enabled: instance.config.enabled,
                mode: instance.config.mode,
                readiness,
                evaluation_count,
                last_arrival,
                last_processed,
                last_position,
                learning_enabled,
                last_learning_weight,
                learning_count,
                last_error,
            };
        }
        if loaded_ids.len() != runner.brain_count() {
            return Err(RegistryError::InvalidState(
                "saved state omitted a configured brain".to_owned(),
            ));
        }
        let mut trailing = [0_u8; 1];
        if reader.read(&mut trailing).map_err(state_io_error)? != 0 {
            return Err(RegistryError::InvalidState(
                "trailing bytes after runner state".to_owned(),
            ));
        }
        Ok(runner)
    }
}

struct BrainSavePlan {
    payload_length: usize,
    model_budget_elements: usize,
}

fn preflight_saved_brain(
    instance: &BrainInstance,
    load_budget: &mut LoadBudget,
) -> Result<BrainSavePlan, RegistryError> {
    if instance.config.id.len() > MAX_BRAIN_ID_LENGTH
        || instance.config.resources.max_memory_bytes == 0
    {
        return Err(RegistryError::InvalidConfiguration {
            brain: instance.config.id.clone(),
            reason: "brain persistence limits are invalid".to_owned(),
        });
    }
    if instance.known_streams.len() > instance.config.resources.max_streams {
        return Err(RegistryError::InvalidState(
            "known stream count exceeds brain resource limit".to_owned(),
        ));
    }
    if instance.stream_generations.len() != instance.known_streams.len()
        || instance
            .stream_generations
            .keys()
            .any(|stream| !instance.known_streams.contains_key(stream))
        || instance
            .last_positive_commit
            .keys()
            .any(|stream| !instance.known_streams.contains_key(stream))
        || instance
            .normal_streaks
            .keys()
            .any(|stream| !instance.known_streams.contains_key(stream))
    {
        return Err(RegistryError::InvalidState(
            "runner stream bookkeeping is inconsistent".to_owned(),
        ));
    }
    let model_budget_before = load_budget.remaining();
    let mut counter = CountingWriter::default();
    instance
        .model
        .save_state_with_budget(&mut counter, load_budget)
        .map_err(RegistryError::Io)?;
    let model_budget_elements = model_budget_before.saturating_sub(load_budget.remaining());
    let payload_length = counter.len();
    if payload_length > instance.config.resources.max_memory_bytes {
        return Err(RegistryError::InvalidState(
            "model payload exceeds brain memory limit".to_owned(),
        ));
    }
    load_budget
        .reserve(instance.config.id.len())
        .map_err(state_io_error)?;
    load_budget
        .reserve(instance.state.last_error.as_ref().map_or(0, String::len))
        .map_err(state_io_error)?;
    load_budget
        .reserve(instance.known_streams.len())
        .map_err(state_io_error)?;
    for stream in instance.known_streams.keys() {
        load_budget.reserve(stream.len()).map_err(state_io_error)?;
    }
    load_budget
        .reserve(instance.normal_streaks.len())
        .map_err(state_io_error)?;
    for stream in instance.normal_streaks.keys() {
        load_budget.reserve(stream.len()).map_err(state_io_error)?;
    }
    load_budget
        .reserve(instance.stream_generations.len())
        .map_err(state_io_error)?;
    for stream in instance.stream_generations.keys() {
        load_budget.reserve(stream.len()).map_err(state_io_error)?;
    }
    load_budget
        .reserve(instance.last_positive_commit.len())
        .map_err(state_io_error)?;
    for stream in instance.last_positive_commit.keys() {
        load_budget.reserve(stream.len()).map_err(state_io_error)?;
    }
    load_budget
        .reserve(instance.positive_commit_history.len())
        .map_err(state_io_error)?;
    load_budget
        .reserve(payload_length)
        .map_err(state_io_error)?;
    Ok(BrainSavePlan {
        payload_length,
        model_budget_elements,
    })
}

#[derive(Default)]
struct CountingWriter {
    length: usize,
}

impl CountingWriter {
    fn len(&self) -> usize {
        self.length
    }
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.length = self
            .length
            .checked_add(buffer.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "state size overflowed"))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct LimitedWriter<'a> {
    writer: &'a mut dyn Write,
    remaining: usize,
}

impl<'a> LimitedWriter<'a> {
    fn new(writer: &'a mut dyn Write, limit: usize) -> Self {
        Self {
            writer,
            remaining: limit,
        }
    }

    fn remaining(&self) -> usize {
        self.remaining
    }
}

impl Write for LimitedWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "model state size changed during save",
            ));
        }
        self.writer.write_all(buffer)?;
        self.remaining -= buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

struct LimitedReader<'a> {
    reader: &'a mut dyn Read,
    remaining: usize,
}

impl<'a> LimitedReader<'a> {
    fn new(reader: &'a mut dyn Read, limit: usize) -> Self {
        Self {
            reader,
            remaining: limit,
        }
    }

    fn remaining(&self) -> usize {
        self.remaining
    }
}

impl Read for LimitedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 || buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min(self.remaining);
        let read = self.reader.read(&mut buffer[..length])?;
        self.remaining -= read;
        Ok(read)
    }
}

fn state_io_error(error: io::Error) -> RegistryError {
    RegistryError::InvalidState(error.to_string())
}

fn write_optional_timestamp(writer: &mut dyn Write, value: Option<Timestamp>) -> io::Result<()> {
    write_u8(writer, u8::from(value.is_some()))?;
    if let Some(value) = value {
        write_timestamp(writer, value)?;
    }
    Ok(())
}

fn read_optional_timestamp(reader: &mut dyn Read) -> io::Result<Option<Timestamp>> {
    let present = read_flag_io(reader, "timestamp")?;
    present.then(|| read_timestamp(reader)).transpose()
}

fn read_optional_u64(reader: &mut dyn Read) -> io::Result<Option<u64>> {
    let present = read_flag_io(reader, "u64")?;
    present.then(|| read_u64(reader)).transpose()
}

fn write_optional_string(writer: &mut dyn Write, value: Option<&str>) -> io::Result<()> {
    write_u8(writer, u8::from(value.is_some()))?;
    if let Some(value) = value {
        write_string(writer, value)?;
    }
    Ok(())
}

fn read_optional_string(
    reader: &mut dyn Read,
    budget: &mut LoadBudget,
) -> io::Result<Option<String>> {
    let present = read_flag_io(reader, "string")?;
    present.then(|| read_string(reader, budget)).transpose()
}

fn read_flag(reader: &mut dyn Read, name: &'static str) -> Result<bool, RegistryError> {
    read_flag_io(reader, name).map_err(state_io_error)
}

fn read_flag_io(reader: &mut dyn Read, name: &'static str) -> io::Result<bool> {
    match read_u8(reader)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {name} flag"),
        )),
    }
}

fn readiness_tag(readiness: &Readiness) -> u8 {
    match readiness {
        Readiness::Evaluated => 0,
        Readiness::BaselineInsufficient => 1,
        Readiness::InputMissing => 2,
        Readiness::Invalid => 3,
        Readiness::Expired => 4,
        Readiness::Error => 5,
    }
}

fn read_readiness(reader: &mut dyn Read) -> io::Result<Readiness> {
    match read_u8(reader)? {
        0 => Ok(Readiness::Evaluated),
        1 => Ok(Readiness::BaselineInsufficient),
        2 => Ok(Readiness::InputMissing),
        3 => Ok(Readiness::Invalid),
        4 => Ok(Readiness::Expired),
        5 => Ok(Readiness::Error),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid readiness value",
        )),
    }
}
