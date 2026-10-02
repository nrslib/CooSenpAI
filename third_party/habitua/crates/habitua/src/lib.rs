#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! A replaceable habituation engine for arbitrary feature vectors.
//!
//! The core default alias is the sparse random-projection [`FlyModel`]. The
//! CPU connectome simulator is exposed explicitly for the separate plugin
//! boundary and comparison work.

mod config;
mod decaying_memory;
mod encoder;
mod failure;
mod habituation;
mod memory;
mod model;
mod nearest_neighbor;
mod numeric;
mod orchestration;
mod pattern;
mod persistence;
mod profile;
mod reaction;
mod timestamp;

pub use config::{Config, ConfigError, LongLearningPolicy};
pub use decaying_memory::{DecayingMemory, MemoryConfigError};
pub use encoder::{EncodeError, Encoder, EncoderConfigError, FlyHashEncoder};
pub use failure::{FailureDiagnostics, FailureModel, FailureModelConfig, FailureUpdate};
pub use habituation::{
    EngineConfigError, FlyDiagnostics, FlyUpdate, Habituation, HabituationError, StreamStats,
};
pub use memory::{
    Memory, MemoryError, MemoryObservation, MemoryObservationError, MemoryStats, MemoryTransaction,
    PersistentMemory,
};
pub use model::{
    Evaluation, FailureObservation, Model, ModelError, ModelIdentity, ModelInput, Observation,
    ObserveError, PersistentModel,
};
pub use nearest_neighbor::{
    NearestNeighborConfig, NearestNeighborDiagnostics, NearestNeighborError, NearestNeighborModel,
    NearestNeighborStreamStats, NearestNeighborUpdate,
};
pub use numeric::{
    NamedDeviation, NumericDeviationConfig, NumericDeviationDiagnostics, NumericDeviationError,
    NumericDeviationModel, NumericDirection, NumericStreamStats, NumericUpdate,
};
pub use orchestration::{
    BrainConfig, BrainEvaluation, BrainInput, BrainInputSpec, BrainLearningPolicy, BrainRunner,
    BrainUpdate, CommitRecord, ConsensusAction, ConsensusPolicy, DeletionPolicy, FrequencyLimit,
    LearningMode, ModelRegistry, ModelReport, ModelSettings, ModelSpec, OrchestrationEvaluation,
    ReactionPolicy, RegistryError, ResourcePolicy, RunnerError, StatePolicy,
};
pub use pattern::{Pattern, PatternError, PatternId};
pub use persistence::{LoadBudget, PersistenceError};
pub use reaction::{
    BrainEvaluationState, BrainMode, Evidence, LearningSignal, Metric, MetricDirection, Reaction,
    ReactionKind, Readiness,
};
pub use timestamp::Timestamp;

/// The earlier sparse random-projection habituation model.
pub type FlyModel = Habituation<FlyHashEncoder, DecayingMemory>;

/// The core default habituation model.
pub type Habitua = FlyModel;

#[cfg(test)]
mod tests;
