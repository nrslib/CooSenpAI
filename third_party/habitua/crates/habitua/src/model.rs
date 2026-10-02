use std::fmt;
use std::io::{Read, Write};
use std::time::Duration;

use crate::encoder::EncodeError;
use crate::memory::{MemoryError, MemoryObservationError};
use crate::pattern::PatternId;
use crate::reaction::Readiness;

/// Structured result information accepted by models that do not consume a
/// feature vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailureObservation {
    /// Whether the operation completed successfully.
    pub success: bool,
    /// Optional stable result code supplied by the adapter.
    pub result_code: Option<String>,
    /// Optional error category supplied by the adapter.
    pub error_type: Option<String>,
    /// Optional state name at the time of the result.
    pub state: Option<String>,
}

/// Input alternatives passed through the orchestration boundary.
pub enum ModelInput<'a> {
    /// An adapter-produced numeric feature vector.
    Features(&'a [f32]),
    /// A structured operation result for a failure model.
    Failure(&'a FailureObservation),
}

/// Identifies the model implementation and the meaning of its input features.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelIdentity {
    /// Version of the model algorithm and its behavior.
    pub model_version: u32,
    /// Version of the adapter-defined feature schema.
    pub feature_schema_version: u32,
}

/// The result of evaluating one feature vector.
///
/// `update` is deliberately returned with the result instead of being applied
/// by [`Model::evaluate`]. A caller can inspect the result, then commit the
/// update with a learning weight of zero to stop learned-state reinforcement
/// or a smaller value to reduce reinforcement.
pub struct Evaluation<D, U> {
    /// Short-term novelty in the range `0..=1`, when this model has that
    /// concept.
    pub novelty_short: Option<f32>,
    /// Long-term novelty in the range `0..=1`, when this model has that
    /// concept.
    pub novelty_long: Option<f32>,
    /// Short-term familiarity in the range `0..=1`, when this model has that
    /// concept.
    pub familiarity_short: Option<f32>,
    /// Long-term familiarity in the range `0..=1`, when this model has that
    /// concept.
    pub familiarity_long: Option<f32>,
    /// Time since the model's exact pattern was last seen, when available.
    pub recency: Option<Duration>,
    /// Stable pattern fingerprint, when this model has a pattern concept.
    pub pattern_id: Option<PatternId>,
    /// Whether the model had enough information to evaluate its result.
    pub readiness: Readiness,
    /// Information specific to the model implementation.
    pub diagnostics: D,
    /// Learning update that has not yet been committed.
    pub update: U,
}

/// Compatibility name for a model evaluation returned by `observe`.
pub type Observation<D, U> = Evaluation<D, U>;

/// A replaceable evaluation-and-learning model.
pub trait Model {
    /// Model-specific diagnostic information returned with an evaluation.
    type Diagnostics;
    /// Model-specific update returned with an evaluation.
    type Update;

    /// Evaluates input without changing model state.
    fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: crate::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError>;

    /// Evaluates one of the input forms supported by the orchestration
    /// boundary. Feature models use the default implementation; a structured
    /// model can override this method without encoding its input as floats.
    fn evaluate_input(
        &self,
        stream_id: &str,
        input: ModelInput<'_>,
        at: crate::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        match input {
            ModelInput::Features(features) => self.evaluate(stream_id, features, at),
            ModelInput::Failure(_) => Err(ModelError::Incompatible(
                "this model requires a feature-vector input".to_owned(),
            )),
        }
    }

    /// Evaluates an input while also exposing the adapter's stable input ID.
    ///
    /// The ordinary [`Model::evaluate_input`] contract remains unchanged. A
    /// model that needs to associate an update with a particular input, such
    /// as a feedback event, may override this method. The default delegates
    /// to [`Model::evaluate_input`] so existing model implementations retain
    /// their behavior.
    fn evaluate_input_with_id(
        &self,
        _input_id: &str,
        stream_id: &str,
        input: ModelInput<'_>,
        at: crate::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        self.evaluate_input(stream_id, input, at)
    }

    /// Evaluates input using the public observation spelling.
    ///
    /// This default is an alias for [`Model::evaluate`]. It is also a
    /// `Result` API, so malformed external input is reported rather than
    /// converted into a panic.
    fn observe(
        &self,
        stream_id: &str,
        features: &[f32],
        at: crate::Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        self.evaluate(stream_id, features, at)
    }

    /// Commits an update with a learning weight in `0.0..=1.0`.
    ///
    /// A weight of zero does not reinforce learned state, but records the
    /// arrival metadata carried by the update (such as the latest timestamp or
    /// exact-pattern recency). A weight between zero and one reduces the
    /// learned contribution proportionally. The model may reject an update
    /// that was evaluated before a newer update was committed.
    fn commit(&mut self, update: Self::Update, weight: f32) -> Result<(), ModelError>;

    /// Returns the algorithm and input-schema identity.
    fn identity(&self) -> ModelIdentity;

    /// Returns a stable fingerprint for the complete model configuration.
    fn fingerprint(&self) -> u64;
}

/// Persistence contract for a complete model state.
///
/// Implementations must validate the model identity and configuration before
/// replacing state. A failed load must leave the receiver unchanged. The
/// orchestration registry uses this contract to keep each brain's state
/// separate.
pub trait PersistentModel: Model {
    /// Version of the model-specific state payload.
    fn state_format_version(&self) -> u16;

    /// Saves the model-specific state.
    fn save_state(&self, writer: &mut dyn Write) -> std::io::Result<()>;

    /// Loads the model-specific state transactionally.
    fn load_state(&mut self, reader: &mut dyn Read) -> std::io::Result<()>;

    /// Reserves the model-specific allocation units used by its loader.
    ///
    /// The runner calls this during save preflight with the same cumulative
    /// budget it gives to [`Self::load_state_with_budget`]. Implementations
    /// must count every collection element or scalar slot their loader may
    /// allocate.
    fn reserve_load_budget(&self, load_budget: &mut crate::LoadBudget) -> std::io::Result<()>;

    /// Saves state under the supplied cumulative persistence budget.
    ///
    /// The default reserves the same model-state elements as the loader and
    /// then delegates to [`Self::save_state`]. An implementation that creates
    /// additional temporary collections must reserve those elements here too;
    /// it must not turn the budget into an advisory value.
    fn save_state_with_budget(
        &self,
        writer: &mut dyn Write,
        load_budget: &mut crate::LoadBudget,
    ) -> std::io::Result<()> {
        self.reserve_load_budget(load_budget)?;
        self.save_state(writer)
    }

    /// Loads state while sharing the runner's cumulative allocation budget.
    ///
    /// Implementations must reserve allocations before creating them and must
    /// leave the receiver unchanged if loading fails.
    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        load_budget: &mut crate::LoadBudget,
    ) -> std::io::Result<()>;
}

/// Errors returned by a [`Model`].
#[derive(Debug)]
pub enum ModelError {
    /// The encoder rejected the feature vector.
    Encode(EncodeError),
    /// The memory rejected the observation.
    Memory(MemoryError),
    /// A memory returned a result outside the core contract.
    InvalidMemoryObservation(MemoryObservationError),
    /// The feature profile cannot represent another committed sample.
    ProfileSampleCountOverflow,
    /// A commit weight was not finite or was outside `0.0..=1.0`.
    InvalidLearningWeight,
    /// The update was evaluated against an older model state.
    StaleUpdate,
    /// The update belongs to another model instance or configuration.
    ForeignUpdate,
    /// The model's update generation counter overflowed.
    UpdateSequenceOverflow,
    /// A numeric deviation model rejected its input or configuration.
    Numeric(String),
    /// A configured resource limit would be exceeded.
    ResourceLimit(String),
    /// A model or registry component is not compatible with the requested
    /// input or saved state.
    Incompatible(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(error) => error.fmt(formatter),
            Self::Memory(error) => error.fmt(formatter),
            Self::InvalidMemoryObservation(error) => error.fmt(formatter),
            Self::ProfileSampleCountOverflow => {
                write!(formatter, "feature profile sample count overflowed")
            }
            Self::InvalidLearningWeight => {
                write!(formatter, "learning weight must be finite and in 0..=1")
            }
            Self::StaleUpdate => write!(formatter, "model update was evaluated from stale state"),
            Self::ForeignUpdate => {
                write!(formatter, "model update belongs to another model instance")
            }
            Self::UpdateSequenceOverflow => write!(formatter, "model update sequence overflowed"),
            Self::Numeric(error) => write!(formatter, "numeric deviation error: {error}"),
            Self::ResourceLimit(error) => write!(formatter, "model resource limit: {error}"),
            Self::Incompatible(error) => write!(formatter, "incompatible model state: {error}"),
        }
    }
}

impl std::error::Error for ModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::InvalidMemoryObservation(error) => Some(error),
            Self::ProfileSampleCountOverflow
            | Self::InvalidLearningWeight
            | Self::StaleUpdate
            | Self::ForeignUpdate
            | Self::UpdateSequenceOverflow
            | Self::Numeric(_)
            | Self::ResourceLimit(_)
            | Self::Incompatible(_) => None,
        }
    }
}

impl From<EncodeError> for ModelError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

impl From<MemoryError> for ModelError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

impl From<MemoryObservationError> for ModelError {
    fn from(error: MemoryObservationError) -> Self {
        Self::InvalidMemoryObservation(error)
    }
}

/// Compatibility name for model evaluation errors.
pub type ObserveError = ModelError;

pub(crate) fn validate_learning_weight(weight: f32) -> Result<(), ModelError> {
    if weight.is_finite() && (0.0..=1.0).contains(&weight) {
        Ok(())
    } else {
        Err(ModelError::InvalidLearningWeight)
    }
}
