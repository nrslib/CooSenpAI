use std::time::Duration;

use crate::timestamp::Timestamp;

/// The kind of change to which a brain reacts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReactionKind {
    /// A value or pattern is new to the brain.
    Novel,
    /// A numeric or distance-based value differs from its baseline.
    Deviation,
    /// An expected input or event did not arrive.
    Absence,
    /// An input or condition is repeating.
    Repeat,
    /// An operation explicitly failed.
    Failure,
}

/// Whether a metric becomes more abnormal as it increases or decreases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricDirection {
    /// A larger value is a stronger reaction.
    HigherIsMore,
    /// A smaller value is a stronger reaction.
    LowerIsMore,
    /// Distance from a baseline in either direction is stronger.
    TwoSided,
    /// The value is descriptive and has no ordering contract.
    Informational,
}

/// The state in which a brain produced an evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Readiness {
    /// The model had enough input and baseline state to evaluate the result.
    Evaluated,
    /// The model is valid, but it does not have enough baseline observations.
    BaselineInsufficient,
    /// The brain did not receive an input for its configured target.
    InputMissing,
    /// The input or configuration is invalid.
    Invalid,
    /// Evaluation completed after the configured deadline.
    Expired,
    /// The model could not evaluate the input.
    Error,
}

/// 学習方針が現在の入力を正常・異常のどちらとして扱えるかを示す信号。
///
/// これは反応の種類や閾値の有効化とは別の値である。反応を外部通知用に
/// 無効化しても、異常な入力を正常な学習標本として扱わないために使う。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LearningSignal {
    /// 基準があり、現在の入力を正常な経験として扱える。
    ///
    /// Novelty の反応が同時に出ていても、安定した新しい通常状態を
    /// 学習する方針ではこの値を使える。
    Normal,
    /// 基準があり、現在の入力を異常な経験として扱う。
    Abnormal,
    /// 基準不足または入力不足で学習方針を決められない。
    Unknown,
}

/// A model-specific named metric in a common report.
#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    /// Metric name stable within the model kind.
    pub name: String,
    /// Metric value.
    pub value: f32,
    /// Unit name, such as `z`, `ms`, or `distance`.
    pub unit: String,
    /// Direction in which the metric is more abnormal.
    pub direction: MetricDirection,
}

impl Metric {
    /// Creates a metric from owned or borrowed string-like values.
    pub fn new(
        name: impl Into<String>,
        value: f32,
        unit: impl Into<String>,
        direction: MetricDirection,
    ) -> Self {
        Self {
            name: name.into(),
            value,
            unit: unit.into(),
            direction,
        }
    }
}

/// Evidence supporting a reaction.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    /// Human-readable evidence name.
    pub name: String,
    /// Current value when the evidence is numeric.
    pub current: Option<f32>,
    /// Baseline value when one exists.
    pub baseline: Option<f32>,
    /// Number of observations used by the baseline.
    pub sample_count: Option<usize>,
    /// Time window represented by the baseline.
    pub window: Option<Duration>,
    /// Input IDs used as references, when applicable.
    pub reference_ids: Vec<String>,
    /// Additional model-specific explanation.
    pub detail: String,
}

impl Evidence {
    /// Creates descriptive evidence without a numeric baseline.
    pub fn text(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            current: None,
            baseline: None,
            sample_count: None,
            window: None,
            reference_ids: Vec::new(),
            detail: detail.into(),
        }
    }
}

/// A common reaction report emitted by one brain.
#[derive(Clone, Debug, PartialEq)]
pub struct Reaction {
    /// Brain instance that produced this reaction.
    pub brain: String,
    /// Configured target of the brain.
    pub target: String,
    /// Reaction category.
    pub kind: ReactionKind,
    /// Calibrated strength. It is `None` when the value is not calibrated.
    pub strength: Option<f32>,
    /// Model-specific metrics supporting the reaction.
    pub metrics: Vec<Metric>,
    /// Evidence supporting the reaction.
    pub evidence: Vec<Evidence>,
    /// Whether the reaction is ready for an external policy to use.
    pub readiness: Readiness,
}

/// Whether a brain participates in an external activation policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrainMode {
    /// Reactions may be used by the caller's activation policy.
    Active,
    /// Reactions are recorded for comparison but do not activate the policy.
    Shadow,
}

/// Runtime state recorded for one brain.
#[derive(Clone, Debug, PartialEq)]
pub struct BrainEvaluationState {
    /// Brain instance ID.
    pub brain: String,
    /// Role assigned by the caller.
    pub role: String,
    /// Whether this brain is enabled.
    pub enabled: bool,
    /// Active or shadow mode.
    pub mode: BrainMode,
    /// Readiness of the latest evaluation.
    pub readiness: Readiness,
    /// Number of inputs evaluated, including inputs committed with weight zero.
    pub evaluation_count: u64,
    /// Latest input availability time.
    pub last_arrival: Option<Timestamp>,
    /// Latest input processing time.
    pub last_processed: Option<Timestamp>,
    /// Latest adapter processing position.
    pub last_position: Option<u64>,
    /// Whether learning is currently enabled by the brain policy.
    pub learning_enabled: bool,
    /// Learning weight used by the latest commit, if committed.
    pub last_learning_weight: Option<f32>,
    /// Number of successful commits.
    pub learning_count: u64,
    /// Latest error text, if an evaluation or commit failed.
    pub last_error: Option<String>,
}
