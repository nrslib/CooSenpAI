use std::fmt;
use std::time::Duration;

pub(crate) const MAX_CONFIG_DIMENSIONS: usize = 1_000_000;
pub(crate) const MAX_CONFIG_ENCODER_UNITS: usize = 1_000_000;
pub(crate) const MAX_CONFIG_PATTERNS: usize = 4_000_000;
pub(crate) const MAX_CONFIG_STREAMS: usize = 4_000_000;
pub(crate) const MAX_CONFIG_MEMORY_ELEMENTS: usize = 4_000_000;

pub(crate) const LEGACY_PROJECTION_DENSITY: f32 = 0.75;
pub(crate) const LEGACY_SHORT_REINFORCEMENT: f32 = 0.35;
pub(crate) const LEGACY_LONG_REINFORCEMENT: f32 = 0.12;
pub(crate) const LEGACY_SHORT_FAMILIARITY_WEIGHT: f32 = 0.6;
pub(crate) const LEGACY_LONG_FAMILIARITY_WEIGHT: f32 = 0.4;
pub(crate) const LEGACY_INPUT_STRENGTH_SCALE: f32 = 1.0;
pub(crate) const LEGACY_TEMPORAL_SHIFT_SCALE: f32 = 3.0;
pub(crate) const LEGACY_VARIANCE_FLOOR: f32 = 1.0e-6;
pub(crate) const LEGACY_MAX_PATTERNS: usize = 4_096;
pub(crate) const LEGACY_MODEL_VERSION: u32 = 1;
pub(crate) const LEGACY_FEATURE_SCHEMA_VERSION: u32 = 1;

/// Selects how a committed observation contributes to long-term memory.
///
/// `Revisit` makes a long-term trace stronger only when the exact pattern is
/// seen again after the configured interval. `RepeatedNormal` is useful when
/// a caller has established that consecutive observations are normal; it
/// requires the configured number of consecutive experiences. The hidden
/// legacy variant exists only while reading old persistence formats and is
/// rejected by new configurations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LongLearningPolicy {
    /// Reinforce long-term memory after a time-separated exact-pattern revisit.
    Revisit {
        /// Minimum elapsed time between revisits.
        interval: Duration,
    },
    /// Reinforce long-term memory after enough consecutive experiences.
    RepeatedNormal {
        /// Number of consecutive committed experiences required.
        consecutive_observations: u32,
    },
    #[doc(hidden)]
    LegacyEveryCommit,
}

impl LongLearningPolicy {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        match self {
            Self::Revisit { interval } if interval.is_zero() => {
                Err(ConfigError::ZeroLongRevisitInterval)
            }
            Self::RepeatedNormal {
                consecutive_observations,
            } if *consecutive_observations < 2 => Err(ConfigError::InvalidRepeatedNormalCount),
            Self::LegacyEveryCommit => Err(ConfigError::LegacyLearningPolicy),
            _ => Ok(()),
        }
    }

    pub(crate) fn is_legacy(&self) -> bool {
        matches!(self, Self::LegacyEveryCommit)
    }
}

/// Parameters for the built-in Fly encoder and decaying memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Number of input features expected by the encoder.
    pub dimensions: usize,
    /// Number of units in the sparse projection.
    pub encoder_units: usize,
    /// Number of projection units retained in each pattern.
    pub k: usize,
    /// Seed used to create the deterministic projection.
    pub seed: u64,
    /// Fraction of projection coefficients that are non-zero.
    pub projection_density: f32,
    /// Time constant for short-term memory decay.
    pub short_time_constant: Duration,
    /// Time constant for long-term memory decay.
    pub long_time_constant: Duration,
    /// Maximum short-term reinforcement before learning weight is applied.
    pub short_reinforcement: f32,
    /// Maximum long-term reinforcement before learning weight is applied.
    pub long_reinforcement: f32,
    /// Relative weight of short-term familiarity in the combined value.
    pub short_familiarity_weight: f32,
    /// Relative weight of long-term familiarity in the combined value.
    pub long_familiarity_weight: f32,
    /// Scale used by the input-strength diagnostic.
    pub input_strength_scale: f32,
    /// Scale used by the feature-profile deviation diagnostic.
    pub temporal_shift_scale: f32,
    /// Minimum per-feature standard deviation used by the profile diagnostic.
    pub variance_floor: f32,
    /// Maximum number of exact patterns retained for recency reporting.
    pub max_patterns: usize,
    /// Policy used to reinforce long-term memory.
    pub long_learning_policy: LongLearningPolicy,
    /// Maximum number of independent streams retained by the model.
    pub max_streams: usize,
    /// Maximum total number of owned stream, profile, unit, and pattern
    /// elements retained by the model.
    pub max_memory_elements: usize,
    /// Version of the model algorithm supplied by the caller.
    pub model_version: u32,
    /// Version of the adapter-defined feature schema supplied by the caller.
    pub feature_schema_version: u32,
}

impl Config {
    /// Validates the invariants shared by the built-in components.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_for_format(false)
    }

    pub(crate) fn validate_for_format(&self, allow_legacy: bool) -> Result<(), ConfigError> {
        if self.dimensions == 0 {
            return Err(ConfigError::ZeroDimensions);
        }
        if self.dimensions > MAX_CONFIG_DIMENSIONS {
            return Err(ConfigError::DimensionsTooLarge {
                dimensions: self.dimensions,
                maximum: MAX_CONFIG_DIMENSIONS,
            });
        }
        if self.encoder_units == 0 {
            return Err(ConfigError::ZeroEncoderUnits);
        }
        if self.encoder_units > MAX_CONFIG_ENCODER_UNITS {
            return Err(ConfigError::EncoderUnitsTooLarge {
                encoder_units: self.encoder_units,
                maximum: MAX_CONFIG_ENCODER_UNITS,
            });
        }
        if self.k == 0 || self.k > self.encoder_units {
            return Err(ConfigError::InvalidK {
                k: self.k,
                encoder_units: self.encoder_units,
            });
        }
        if !self.projection_density.is_finite()
            || !(0.0..=1.0).contains(&self.projection_density)
            || self.projection_density == 0.0
        {
            return Err(ConfigError::InvalidProjectionDensity);
        }
        if self.short_time_constant.is_zero() {
            return Err(ConfigError::ZeroShortTimeConstant);
        }
        if self.long_time_constant.is_zero() {
            return Err(ConfigError::ZeroLongTimeConstant);
        }
        if self.long_time_constant <= self.short_time_constant {
            return Err(ConfigError::LongTimeConstantNotLonger);
        }
        if !self.short_reinforcement.is_finite() || !(0.0..=1.0).contains(&self.short_reinforcement)
        {
            return Err(ConfigError::InvalidShortReinforcement);
        }
        if !self.long_reinforcement.is_finite() || !(0.0..=1.0).contains(&self.long_reinforcement) {
            return Err(ConfigError::InvalidLongReinforcement);
        }
        let familiarity_weight_sum = self.short_familiarity_weight + self.long_familiarity_weight;
        if !self.short_familiarity_weight.is_finite()
            || self.short_familiarity_weight < 0.0
            || !self.long_familiarity_weight.is_finite()
            || self.long_familiarity_weight < 0.0
            || !familiarity_weight_sum.is_finite()
            || familiarity_weight_sum <= 0.0
        {
            return Err(ConfigError::InvalidFamiliarityWeights);
        }
        if !self.input_strength_scale.is_finite() || self.input_strength_scale <= 0.0 {
            return Err(ConfigError::InvalidInputStrengthScale);
        }
        if !self.temporal_shift_scale.is_finite() || self.temporal_shift_scale <= 0.0 {
            return Err(ConfigError::InvalidTemporalShiftScale);
        }
        if !self.variance_floor.is_finite() || self.variance_floor <= 0.0 {
            return Err(ConfigError::InvalidVarianceFloor);
        }
        if self.max_patterns == 0 {
            return Err(ConfigError::ZeroMaxPatterns);
        }
        if self.max_patterns > MAX_CONFIG_PATTERNS {
            return Err(ConfigError::MaxPatternsTooLarge {
                max_patterns: self.max_patterns,
                maximum: MAX_CONFIG_PATTERNS,
            });
        }
        if allow_legacy && self.long_learning_policy.is_legacy() {
            // Old state files used an every-commit mode. It is accepted only
            // by the compatibility loader and cannot be constructed through
            // the public configuration validator.
        } else {
            self.long_learning_policy.validate()?;
        }
        if self.max_streams == 0 {
            return Err(ConfigError::ZeroMaxStreams);
        }
        if self.max_streams > MAX_CONFIG_STREAMS {
            return Err(ConfigError::MaxStreamsTooLarge {
                max_streams: self.max_streams,
                maximum: MAX_CONFIG_STREAMS,
            });
        }
        if self.max_memory_elements == 0 {
            return Err(ConfigError::ZeroMaxMemoryElements);
        }
        if self.max_memory_elements > MAX_CONFIG_MEMORY_ELEMENTS {
            return Err(ConfigError::MaxMemoryElementsTooLarge {
                max_memory_elements: self.max_memory_elements,
                maximum: MAX_CONFIG_MEMORY_ELEMENTS,
            });
        }
        if self.model_version == 0 {
            return Err(ConfigError::ZeroModelVersion);
        }
        if self.feature_schema_version == 0 {
            return Err(ConfigError::ZeroFeatureSchemaVersion);
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dimensions: 128,
            encoder_units: 512,
            k: 16,
            seed: 0x4841_4249_5455_4131,
            projection_density: LEGACY_PROJECTION_DENSITY,
            short_time_constant: Duration::from_secs(60 * 60),
            long_time_constant: Duration::from_secs(14 * 24 * 60 * 60),
            short_reinforcement: LEGACY_SHORT_REINFORCEMENT,
            long_reinforcement: LEGACY_LONG_REINFORCEMENT,
            short_familiarity_weight: LEGACY_SHORT_FAMILIARITY_WEIGHT,
            long_familiarity_weight: LEGACY_LONG_FAMILIARITY_WEIGHT,
            input_strength_scale: LEGACY_INPUT_STRENGTH_SCALE,
            temporal_shift_scale: LEGACY_TEMPORAL_SHIFT_SCALE,
            variance_floor: LEGACY_VARIANCE_FLOOR,
            max_patterns: LEGACY_MAX_PATTERNS,
            long_learning_policy: LongLearningPolicy::Revisit {
                interval: Duration::from_secs(60 * 60),
            },
            max_streams: 4_096,
            max_memory_elements: MAX_CONFIG_MEMORY_ELEMENTS,
            model_version: LEGACY_MODEL_VERSION,
            feature_schema_version: LEGACY_FEATURE_SCHEMA_VERSION,
        }
    }
}

/// Errors found while validating [`Config`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// The configured feature dimension is zero.
    ZeroDimensions,
    /// The configured feature dimension exceeds the supported allocation bound.
    DimensionsTooLarge {
        /// Configured feature dimension.
        dimensions: usize,
        /// Maximum supported feature dimension.
        maximum: usize,
    },
    /// The sparse projection has no units.
    ZeroEncoderUnits,
    /// The configured projection-unit count exceeds the supported allocation bound.
    EncoderUnitsTooLarge {
        /// Configured projection-unit count.
        encoder_units: usize,
        /// Maximum supported projection-unit count.
        maximum: usize,
    },
    /// `k` is zero or larger than the projection.
    InvalidK {
        /// Configured retained-unit count.
        k: usize,
        /// Configured projection-unit count.
        encoder_units: usize,
    },
    /// The projection density is not finite or is outside `(0, 1]`.
    InvalidProjectionDensity,
    /// The short-term time constant is zero.
    ZeroShortTimeConstant,
    /// The long-term time constant is zero.
    ZeroLongTimeConstant,
    /// The long-term time constant must exceed the short-term one.
    LongTimeConstantNotLonger,
    /// The short-term reinforcement is invalid.
    InvalidShortReinforcement,
    /// The long-term reinforcement is invalid.
    InvalidLongReinforcement,
    /// The familiarity weights are invalid.
    InvalidFamiliarityWeights,
    /// The input-strength scale is invalid.
    InvalidInputStrengthScale,
    /// The feature-profile scale is invalid.
    InvalidTemporalShiftScale,
    /// The profile variance floor is invalid.
    InvalidVarianceFloor,
    /// No exact-pattern records can be retained.
    ZeroMaxPatterns,
    /// The exact-pattern record limit exceeds the supported allocation bound.
    MaxPatternsTooLarge {
        /// Configured exact-pattern limit.
        max_patterns: usize,
        /// Maximum supported exact-pattern limit.
        maximum: usize,
    },
    /// The long-term revisit interval is zero.
    ZeroLongRevisitInterval,
    /// The repeated-normal policy has too few consecutive observations.
    InvalidRepeatedNormalCount,
    /// The compatibility-only every-commit policy cannot be used for a new
    /// configuration.
    LegacyLearningPolicy,
    /// No streams can be retained.
    ZeroMaxStreams,
    /// The stream limit exceeds the supported allocation bound.
    MaxStreamsTooLarge {
        /// Configured stream limit.
        max_streams: usize,
        /// Supported stream limit.
        maximum: usize,
    },
    /// No memory elements can be retained.
    ZeroMaxMemoryElements,
    /// The memory-element limit exceeds the supported allocation bound.
    MaxMemoryElementsTooLarge {
        /// Configured memory-element limit.
        max_memory_elements: usize,
        /// Supported memory-element limit.
        maximum: usize,
    },
    /// The model version is zero.
    ZeroModelVersion,
    /// The feature schema version is zero.
    ZeroFeatureSchemaVersion,
    /// The encoder dimension does not match the shared configuration.
    EncoderDimensionMismatch {
        /// Dimension declared by the shared configuration.
        expected: usize,
        /// Dimension reported by the encoder.
        actual: usize,
    },
    /// A built-in memory does not use the memory parameters in this config.
    MemoryConfigurationMismatch,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimensions => write!(formatter, "dimensions must be greater than zero"),
            Self::DimensionsTooLarge {
                dimensions,
                maximum,
            } => {
                write!(
                    formatter,
                    "dimensions must not exceed {maximum}, got {dimensions}"
                )
            }
            Self::ZeroEncoderUnits => write!(formatter, "encoder_units must be greater than zero"),
            Self::EncoderUnitsTooLarge {
                encoder_units,
                maximum,
            } => write!(
                formatter,
                "encoder_units must not exceed {maximum}, got {encoder_units}"
            ),
            Self::InvalidK { k, encoder_units } => {
                write!(formatter, "k must be in 1..={encoder_units}, got {k}")
            }
            Self::InvalidProjectionDensity => {
                write!(formatter, "projection_density must be finite and in (0, 1]")
            }
            Self::ZeroShortTimeConstant => {
                write!(formatter, "short_time_constant must be non-zero")
            }
            Self::ZeroLongTimeConstant => write!(formatter, "long_time_constant must be non-zero"),
            Self::LongTimeConstantNotLonger => {
                write!(
                    formatter,
                    "long_time_constant must exceed short_time_constant"
                )
            }
            Self::InvalidShortReinforcement => {
                write!(formatter, "short_reinforcement must be finite and in 0..=1")
            }
            Self::InvalidLongReinforcement => {
                write!(formatter, "long_reinforcement must be finite and in 0..=1")
            }
            Self::InvalidFamiliarityWeights => write!(
                formatter,
                "familiarity weights must be finite, non-negative, and not both zero"
            ),
            Self::InvalidInputStrengthScale => {
                write!(
                    formatter,
                    "input_strength_scale must be finite and positive"
                )
            }
            Self::InvalidTemporalShiftScale => {
                write!(
                    formatter,
                    "temporal_shift_scale must be finite and positive"
                )
            }
            Self::InvalidVarianceFloor => {
                write!(formatter, "variance_floor must be finite and positive")
            }
            Self::ZeroMaxPatterns => write!(formatter, "max_patterns must be greater than zero"),
            Self::MaxPatternsTooLarge {
                max_patterns,
                maximum,
            } => write!(
                formatter,
                "max_patterns must not exceed {maximum}, got {max_patterns}"
            ),
            Self::ZeroLongRevisitInterval => {
                write!(formatter, "long_revisit_interval must be non-zero")
            }
            Self::InvalidRepeatedNormalCount => write!(
                formatter,
                "repeated-normal policy requires at least two observations"
            ),
            Self::LegacyLearningPolicy => write!(
                formatter,
                "legacy every-commit learning policy is only valid for old state files"
            ),
            Self::ZeroMaxStreams => write!(formatter, "max_streams must be greater than zero"),
            Self::MaxStreamsTooLarge {
                max_streams,
                maximum,
            } => write!(
                formatter,
                "max_streams must not exceed {maximum}, got {max_streams}"
            ),
            Self::ZeroMaxMemoryElements => {
                write!(formatter, "max_memory_elements must be greater than zero")
            }
            Self::MaxMemoryElementsTooLarge {
                max_memory_elements,
                maximum,
            } => write!(
                formatter,
                "max_memory_elements must not exceed {maximum}, got {max_memory_elements}"
            ),
            Self::ZeroModelVersion => write!(formatter, "model_version must be greater than zero"),
            Self::ZeroFeatureSchemaVersion => {
                write!(
                    formatter,
                    "feature_schema_version must be greater than zero"
                )
            }
            Self::EncoderDimensionMismatch { expected, actual } => write!(
                formatter,
                "encoder dimensions do not match config: expected {expected}, got {actual}"
            ),
            Self::MemoryConfigurationMismatch => {
                write!(formatter, "memory configuration does not match config")
            }
        }
    }
}

impl std::error::Error for ConfigError {}
