#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! CPU primitives for running a MaleCNS firing-rate network.

mod rate;
mod rate_artifact;
mod rate_controls;
mod rate_habituation;
mod rate_learning;
mod retina;

pub use rate::{
    RateActivitySamples, RateBackward, RateEngine, RateForwardResult, RateForwardTiming,
    RateGradients, RateGraph, RateIncomingEdge, RateModelError, RateNeuronMetadata,
    RateOutgoingEdge, RatePackManifest, RateParameters, SourceFile,
    final_activity_with_parameters_profile, final_activity_with_parameters_sampled_profile,
    reference_forward,
};
pub use rate_artifact::{
    RATE_HELPER_OBSERVATION_SCHEMA, RateHelperArtifact, RateHelperBrain, RateHelperCaseMemory,
    RateHelperResponseGroup,
};
pub use rate_controls::{
    RateControlResult, RateControlStats, shuffle_feedback, shuffle_weights, shuffle_wiring,
};
pub use rate_habituation::{HabituationConfig, HabituationResult, HabituationState};
pub use rate_learning::{
    FeedbackEvent, FeedbackKind, FeedbackShuffleResult, FeedbackStore, FrozenReadoutConfig,
    FrozenReadoutDiagnostics, FrozenReadoutFeedback, FrozenReadoutHistoryEntry,
    FrozenReadoutHistoryFeatures, FrozenReadoutModel, FrozenReadoutUpdate, RateBrainConfig,
    RateBrainDiagnostics, RateBrainModel, RateBrainRole, RateBrainUpdate, RateLearningConfig,
    RateLearningState, RateReadout, ReadoutForward, ReadoutGradients, TrainingMode, TrainingReport,
    frozen_readout_augmented_features, frozen_readout_augmented_features_with_expiry,
    frozen_readout_history_features, frozen_readout_history_features_with_expiry,
    register_frozen_readout_model, register_rate_brain_model,
};
pub use retina::{
    RetinaAudit, RetinaChannel, RetinaInput, RetinaMap, RetinaMapConfig, RetinaMapEntry, RgbImage,
    ScreenFixture, ScreenFixtureKind, image_sha256,
};
