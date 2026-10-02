use std::cmp::Ordering;
use std::fmt;

use crate::config::Config;
use crate::pattern::{Pattern, PatternError, stable_hash};

const MAX_PROJECTION_VALUES: usize = 64 * 1024 * 1024;
const ENCODER_ALGORITHM_VERSION: u64 = 1;

/// Converts a feature vector into a sparse activation pattern.
pub trait Encoder {
    /// Encodes `features` into a sparse pattern.
    fn encode(&self, features: &[f32]) -> Result<Pattern, EncodeError>;

    /// Returns the number of input features accepted by this encoder.
    fn input_dimensions(&self) -> usize;

    /// Returns a stable identity for the algorithm and its configuration.
    ///
    /// Persistent generic engines store this value and reject a loader whose
    /// encoder does not match the saved encoder.
    fn fingerprint(&self) -> u64;
}

/// A deterministic sparse random projection inspired by fly mushroom-body
/// hashing.
///
/// The projection is generated once from `seed`. For the same dimensions,
/// unit count, `k`, and seed, the same feature vector produces the same
/// pattern on every process and platform supported by this crate. Low-density
/// projections are stored sparsely so zero coefficients are not retained.
#[derive(Clone, Debug)]
pub struct FlyHashEncoder {
    dimensions: usize,
    encoder_units: usize,
    k: usize,
    projection_density: f32,
    seed: u64,
    projections: ProjectionStorage,
}

impl FlyHashEncoder {
    /// Creates a sparse projection encoder with the default projection density.
    pub fn new(
        dimensions: usize,
        encoder_units: usize,
        k: usize,
        seed: u64,
    ) -> Result<Self, EncoderConfigError> {
        Self::new_with_density(
            dimensions,
            encoder_units,
            k,
            seed,
            Config::default().projection_density,
        )
    }

    /// Creates a sparse projection encoder with an explicit non-zero density.
    pub fn new_with_density(
        dimensions: usize,
        encoder_units: usize,
        k: usize,
        seed: u64,
        projection_density: f32,
    ) -> Result<Self, EncoderConfigError> {
        if dimensions == 0 {
            return Err(EncoderConfigError::ZeroDimensions);
        }
        if encoder_units == 0 {
            return Err(EncoderConfigError::ZeroEncoderUnits);
        }
        if k == 0 || k > encoder_units {
            return Err(EncoderConfigError::InvalidK { k, encoder_units });
        }
        if !projection_density.is_finite()
            || !(0.0..=1.0).contains(&projection_density)
            || projection_density == 0.0
        {
            return Err(EncoderConfigError::InvalidProjectionDensity);
        }
        let projection_values = dimensions
            .checked_mul(encoder_units)
            .ok_or(EncoderConfigError::ProjectionSizeOverflow)?;
        if projection_values > MAX_PROJECTION_VALUES {
            return Err(EncoderConfigError::ProjectionTooLarge {
                values: projection_values,
                maximum: MAX_PROJECTION_VALUES,
            });
        }

        let zero_buckets = ((1.0 - projection_density) * 256.0).floor() as u64;
        let mut random = SplitMix64::new(seed);
        let projections = if projection_density < 0.5 {
            let mut rows: Vec<Vec<(usize, f32)>> = (0..encoder_units).map(|_| Vec::new()).collect();
            for row in &mut rows {
                for dimension in 0..dimensions {
                    let value = next_projection_value(&mut random, zero_buckets);
                    if value != 0.0 {
                        row.push((dimension, value));
                    }
                }
            }
            ProjectionStorage::Sparse(rows)
        } else {
            let mut values = Vec::with_capacity(projection_values);
            for _ in 0..projection_values {
                values.push(next_projection_value(&mut random, zero_buckets));
            }
            ProjectionStorage::Dense(values)
        };

        Ok(Self {
            dimensions,
            encoder_units,
            k,
            projection_density,
            seed,
            projections,
        })
    }

    /// Creates an encoder using the encoder parameters in `config`.
    pub fn from_config(config: &Config) -> Result<Self, EncoderConfigError> {
        Self::new_with_density(
            config.dimensions,
            config.encoder_units,
            config.k,
            config.seed,
            config.projection_density,
        )
    }
}

impl Encoder for FlyHashEncoder {
    fn encode(&self, features: &[f32]) -> Result<Pattern, EncodeError> {
        if features.is_empty() {
            return Err(EncodeError::EmptyFeatures);
        }
        if features.len() != self.dimensions {
            return Err(EncodeError::DimensionMismatch {
                expected: self.dimensions,
                actual: features.len(),
            });
        }
        if let Some(index) = features.iter().position(|value| !value.is_finite()) {
            return Err(EncodeError::NonFiniteFeature { index });
        }

        let mut scores = Vec::with_capacity(self.encoder_units);
        for unit in 0..self.encoder_units {
            let score = match &self.projections {
                ProjectionStorage::Dense(projections) => {
                    let row_start = unit * self.dimensions;
                    projections[row_start..row_start + self.dimensions]
                        .iter()
                        .zip(features)
                        .map(|(projection, feature)| f64::from(*projection) * f64::from(*feature))
                        .sum::<f64>()
                }
                ProjectionStorage::Sparse(rows) => rows[unit]
                    .iter()
                    .map(|(dimension, projection)| {
                        f64::from(*projection) * f64::from(features[*dimension])
                    })
                    .sum::<f64>(),
            };
            scores.push((unit, score));
        }

        scores.sort_unstable_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });
        let active_units = scores
            .into_iter()
            .take(self.k)
            .map(|(unit, _)| unit)
            .collect();
        Pattern::new(active_units).map_err(EncodeError::Pattern)
    }

    fn input_dimensions(&self) -> usize {
        self.dimensions
    }

    fn fingerprint(&self) -> u64 {
        stable_hash([
            ENCODER_ALGORITHM_VERSION,
            self.dimensions as u64,
            self.encoder_units as u64,
            self.k as u64,
            self.seed,
            u64::from(self.projection_density.to_bits()),
        ])
    }
}

#[derive(Clone, Debug)]
enum ProjectionStorage {
    Dense(Vec<f32>),
    Sparse(Vec<Vec<(usize, f32)>>),
}

fn next_projection_value(random: &mut SplitMix64, zero_buckets: u64) -> f32 {
    let bits = random.next_u64();
    if bits % 256 < zero_buckets {
        0.0
    } else {
        random_signed_value(bits)
    }
}

fn random_signed_value(bits: u64) -> f32 {
    let fraction = (bits >> 8) as f64 / (u64::MAX >> 8) as f64;
    (fraction.mul_add(2.0, -1.0)) as f32
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

/// Errors found while constructing a [`FlyHashEncoder`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncoderConfigError {
    /// The input dimension is zero.
    ZeroDimensions,
    /// The projection has no output units.
    ZeroEncoderUnits,
    /// `k` is zero or larger than the projection.
    InvalidK {
        /// Configured active-unit count.
        k: usize,
        /// Configured projection-unit count.
        encoder_units: usize,
    },
    /// The projection density is not finite or is outside `(0, 1]`.
    InvalidProjectionDensity,
    /// The projection matrix size overflowed `usize`.
    ProjectionSizeOverflow,
    /// The projection matrix exceeded the built-in allocation limit.
    ProjectionTooLarge {
        /// Number of values requested.
        values: usize,
        /// Maximum accepted number of values.
        maximum: usize,
    },
}

impl fmt::Display for EncoderConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimensions => write!(formatter, "dimensions must be greater than zero"),
            Self::ZeroEncoderUnits => write!(formatter, "encoder_units must be greater than zero"),
            Self::InvalidK { k, encoder_units } => {
                write!(formatter, "k must be in 1..={encoder_units}, got {k}")
            }
            Self::InvalidProjectionDensity => {
                write!(formatter, "projection density must be finite and in (0, 1]")
            }
            Self::ProjectionSizeOverflow => write!(formatter, "projection matrix size overflowed"),
            Self::ProjectionTooLarge { values, maximum } => write!(
                formatter,
                "projection matrix has {values} values, maximum is {maximum}"
            ),
        }
    }
}

impl std::error::Error for EncoderConfigError {}

/// Errors found while encoding a feature vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodeError {
    /// The feature vector is empty.
    EmptyFeatures,
    /// The vector length does not match the encoder.
    DimensionMismatch {
        /// Expected input dimension.
        expected: usize,
        /// Actual input dimension.
        actual: usize,
    },
    /// A feature is NaN or infinite.
    NonFiniteFeature {
        /// Index of the invalid feature.
        index: usize,
    },
    /// The encoder could not construct a non-empty pattern.
    Pattern(PatternError),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFeatures => write!(formatter, "feature vector must not be empty"),
            Self::DimensionMismatch { expected, actual } => {
                write!(formatter, "expected {expected} features, got {actual}")
            }
            Self::NonFiniteFeature { index } => {
                write!(formatter, "feature at index {index} is not finite")
            }
            Self::Pattern(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EncodeError {}
