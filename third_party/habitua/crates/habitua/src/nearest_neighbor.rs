use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::{MAX_CONFIG_DIMENSIONS, MAX_CONFIG_PATTERNS, MAX_CONFIG_STREAMS};
use crate::model::{
    Evaluation, Model, ModelError, ModelIdentity, PersistentModel, validate_learning_weight,
};
use crate::pattern::stable_hash;
use crate::persistence::{
    LoadBudget, PersistenceError, read_count, read_f32, read_string, read_timestamp, read_u8,
    read_u16, read_u32, read_u64, write_f32, write_string, write_timestamp, write_u8, write_u16,
    write_u32, write_u64,
};
use crate::reaction::Readiness;
use crate::timestamp::Timestamp;

const NEAREST_STATE_VERSION: u16 = 1;
const NEAREST_MAGIC: &[u8; 8] = b"NNMODEL\0";
const NEAREST_ALGORITHM_VERSION: u64 = 1;
static NEXT_NEAREST_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Configuration for NearestNeighborModel.
#[derive(Clone, Debug, PartialEq)]
pub struct NearestNeighborConfig {
    /// Number of input dimensions.
    pub dimensions: usize,
    /// Maximum number of representative vectors per stream.
    pub max_representatives: usize,
    /// Representatives at or below this cosine distance are merged.
    pub merge_distance: f32,
    /// Maximum exact value patterns retained for recency evidence.
    pub max_patterns: usize,
    /// Maximum number of streams retained by the model.
    pub max_streams: usize,
    /// Version of the model algorithm.
    pub model_version: u32,
    /// Version of the adapter-defined feature schema.
    pub feature_schema_version: u32,
}

impl Default for NearestNeighborConfig {
    fn default() -> Self {
        Self {
            dimensions: 128,
            max_representatives: 256,
            // A conservative default avoids merging unrelated sparse vectors.
            // Applications should calibrate this against the diagnostic
            // distance distribution for their adapter.
            merge_distance: 0.03,
            max_patterns: 4_096,
            max_streams: 4_096,
            model_version: 1,
            feature_schema_version: 1,
        }
    }
}

impl NearestNeighborConfig {
    fn validate(&self) -> Result<(), NearestNeighborError> {
        if self.dimensions == 0
            || self.max_representatives == 0
            || self.max_patterns == 0
            || self.max_streams == 0
        {
            return Err(NearestNeighborError::InvalidParameter {
                name: "retention_limits",
            });
        }
        if self.dimensions > MAX_CONFIG_DIMENSIONS
            || self.max_representatives > MAX_CONFIG_PATTERNS
            || self.max_patterns > MAX_CONFIG_PATTERNS
            || self.max_streams > MAX_CONFIG_STREAMS
        {
            return Err(NearestNeighborError::InvalidParameter {
                name: "retention_limits",
            });
        }
        if !self.merge_distance.is_finite() || !(0.0..=2.0).contains(&self.merge_distance) {
            return Err(NearestNeighborError::InvalidParameter {
                name: "merge_distance",
            });
        }
        if self.model_version == 0 || self.feature_schema_version == 0 {
            return Err(NearestNeighborError::InvalidParameter {
                name: "identity_version",
            });
        }
        Ok(())
    }
}

/// Diagnostics returned by NearestNeighborModel.
#[derive(Clone, Debug, PartialEq)]
pub struct NearestNeighborDiagnostics {
    /// Distance to the closest representative, or None before learning.
    pub nearest_distance: Option<f32>,
    /// Mean distance to all retained representatives, or None before learning.
    pub distance_mean: Option<f32>,
    /// Median distance to all retained representatives, or None before learning.
    pub distance_median: Option<f32>,
    /// Maximum distance to all retained representatives, or None before learning.
    pub distance_max: Option<f32>,
    /// Number of representative vectors retained for the stream.
    pub representative_count: usize,
    /// Maximum configured representative count.
    pub maximum_representatives: usize,
}

/// Statistics for one nearest-neighbor stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NearestNeighborStreamStats {
    /// Number of representative vectors.
    pub representative_count: usize,
    /// Number of retained exact patterns.
    pub pattern_count: usize,
    /// Latest committed timestamp.
    pub last_observed: Option<Timestamp>,
}

/// The uncommitted update returned by NearestNeighborModel.
pub struct NearestNeighborUpdate {
    stream_id: String,
    expected_generation: Option<u64>,
    model_token: u64,
    configuration_fingerprint: u64,
    input_dimensions: usize,
    values: Vec<f32>,
    at: Timestamp,
    pattern_key: Vec<u32>,
}

/// A bounded reference detector based on cosine distance to representative
/// examples. Near-duplicate observations are merged; new representatives
/// evict the lowest-weight, oldest example when the limit is reached.
pub struct NearestNeighborModel {
    config: NearestNeighborConfig,
    streams: BTreeMap<String, NeighborStream>,
    model_token: u64,
}

impl NearestNeighborModel {
    /// Creates an empty model with the default configuration.
    pub fn new(config: NearestNeighborConfig) -> Result<Self, NearestNeighborError> {
        config.validate()?;
        Ok(Self {
            config,
            streams: BTreeMap::new(),
            model_token: NEXT_NEAREST_TOKEN.fetch_add(1, Ordering::Relaxed),
        })
    }

    /// Returns the model configuration.
    pub fn config(&self) -> &NearestNeighborConfig {
        &self.config
    }

    /// Returns statistics for a stream, if it exists.
    pub fn stats(&self, stream_id: &str) -> Option<NearestNeighborStreamStats> {
        self.streams.get(stream_id).map(NeighborStream::stats)
    }

    /// Removes one stream and returns whether it existed.
    pub fn forget(&mut self, stream_id: &str) -> bool {
        self.streams.remove(stream_id).is_some()
    }

    /// Evaluates an input without changing model state.
    pub fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: Timestamp,
    ) -> Result<Evaluation<NearestNeighborDiagnostics, NearestNeighborUpdate>, ModelError> {
        self.validate_features(features)?;
        let stream = self.streams.get(stream_id);
        if let Some(stream) = stream
            && let Some(previous) = stream.last_observed
            && at < previous
        {
            return Err(NearestNeighborError::TimestampOutOfOrder {
                previous,
                current: at,
            }
            .into_model_error());
        }
        let distances = stream
            .map(|state| {
                state
                    .representatives
                    .iter()
                    .map(|representative| cosine_distance(&representative.values, features))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let nearest_distance = distances.iter().copied().min_by(f32::total_cmp);
        let distance_mean =
            (!distances.is_empty()).then(|| distances.iter().sum::<f32>() / distances.len() as f32);
        let distance_median = if distances.is_empty() {
            None
        } else {
            let mut sorted = distances.clone();
            sorted.sort_unstable_by(f32::total_cmp);
            Some(sorted[(sorted.len() - 1) / 2])
        };
        let distance_max = distances.iter().copied().max_by(f32::total_cmp);
        let pattern_key = features
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>();
        let recency = stream.and_then(|state| {
            state
                .patterns
                .get(&pattern_key)
                .and_then(|record| at.checked_duration_since(record.last_seen))
        });
        let pattern_id = stable_hash(pattern_key.iter().map(|bits| u64::from(*bits)));
        Ok(Evaluation {
            novelty_short: None,
            novelty_long: None,
            familiarity_short: None,
            familiarity_long: None,
            recency,
            pattern_id: Some(pattern_id),
            readiness: if nearest_distance.is_some() {
                Readiness::Evaluated
            } else {
                Readiness::BaselineInsufficient
            },
            diagnostics: NearestNeighborDiagnostics {
                nearest_distance,
                distance_mean,
                distance_median,
                distance_max,
                representative_count: stream.map_or(0, |state| state.representatives.len()),
                maximum_representatives: self.config.max_representatives,
            },
            update: NearestNeighborUpdate {
                stream_id: stream_id.to_owned(),
                expected_generation: stream.map(|state| state.generation),
                model_token: self.model_token,
                configuration_fingerprint: self.fingerprint(),
                input_dimensions: self.config.dimensions,
                values: features.to_vec(),
                at,
                pattern_key,
            },
        })
    }

    /// Commits an evaluation update with a learning weight.
    pub fn commit(&mut self, update: NearestNeighborUpdate, weight: f32) -> Result<(), ModelError> {
        validate_learning_weight(weight)?;
        let NearestNeighborUpdate {
            stream_id,
            expected_generation,
            model_token,
            configuration_fingerprint,
            input_dimensions,
            values,
            at,
            pattern_key,
        } = update;
        if model_token != self.model_token
            || configuration_fingerprint != self.fingerprint()
            || input_dimensions != self.config.dimensions
            || values.len() != self.config.dimensions
        {
            return Err(ModelError::ForeignUpdate);
        }
        if self.streams.get(&stream_id).map(|state| state.generation) != expected_generation {
            return Err(ModelError::StaleUpdate);
        }
        let next_generation = expected_generation
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ModelError::UpdateSequenceOverflow)?;
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.commit(&self.config, values, weight, at, pattern_key);
            stream.generation = next_generation;
        } else {
            if self.streams.len() >= self.config.max_streams {
                return Err(NearestNeighborError::TooManyStreams.into_model_error());
            }
            let mut stream = NeighborStream::new();
            stream.commit(&self.config, values, weight, at, pattern_key);
            stream.generation = next_generation;
            self.streams.insert(stream_id, stream);
        }
        Ok(())
    }

    fn validate_features(&self, features: &[f32]) -> Result<(), ModelError> {
        if features.is_empty() {
            return Err(NearestNeighborError::EmptyFeatures.into_model_error());
        }
        if features.len() != self.config.dimensions {
            return Err(NearestNeighborError::DimensionMismatch {
                expected: self.config.dimensions,
                actual: features.len(),
            }
            .into_model_error());
        }
        if let Some(index) = features.iter().position(|value| !value.is_finite()) {
            return Err(NearestNeighborError::NonFiniteFeature { index }.into_model_error());
        }
        if norm(features) == 0.0 {
            return Err(NearestNeighborError::ZeroNorm.into_model_error());
        }
        Ok(())
    }

    fn identity(&self) -> ModelIdentity {
        ModelIdentity {
            model_version: self.config.model_version,
            feature_schema_version: self.config.feature_schema_version,
        }
    }

    fn fingerprint(&self) -> u64 {
        stable_hash([
            NEAREST_ALGORITHM_VERSION,
            self.config.dimensions as u64,
            self.config.max_representatives as u64,
            u64::from(self.config.merge_distance.to_bits()),
            self.config.max_patterns as u64,
            self.config.max_streams as u64,
            u64::from(self.config.model_version),
            u64::from(self.config.feature_schema_version),
        ])
    }

    /// Saves the complete nearest-neighbor state.
    pub fn save<W: Write>(&self, writer: &mut W) -> Result<(), PersistenceError> {
        self.validate_save_budget()
            .map_err(PersistenceError::from)?;
        self.save_state(writer).map_err(PersistenceError::from)
    }

    /// Loads a complete nearest-neighbor state transactionally.
    pub fn load<R: Read>(reader: &mut R) -> Result<Self, PersistenceError> {
        Self::load_dyn(reader)
    }

    fn load_dyn(reader: &mut dyn Read) -> Result<Self, PersistenceError> {
        let mut budget = LoadBudget::new();
        Self::load_dyn_with_budget(reader, &mut budget)
    }

    fn load_dyn_with_budget(
        reader: &mut dyn Read,
        budget: &mut LoadBudget,
    ) -> Result<Self, PersistenceError> {
        let mut magic = [0_u8; NEAREST_MAGIC.len()];
        reader.read_exact(&mut magic)?;
        if &magic != NEAREST_MAGIC {
            return Err(PersistenceError::InvalidMagic);
        }
        let version = read_u16(reader)?;
        if version != NEAREST_STATE_VERSION {
            return Err(PersistenceError::UnsupportedVersion(version));
        }
        let model_version = read_u32(reader)?;
        let feature_schema_version = read_u32(reader)?;
        let saved_fingerprint = read_u64(reader)?;
        let config = read_config(reader, model_version, feature_schema_version)?;
        let mut model =
            Self::new(config).map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
        if model.fingerprint() != saved_fingerprint {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "nearest-neighbor model",
            });
        }
        let stream_count = read_count(reader)?;
        if stream_count > model.config.max_streams {
            return Err(PersistenceError::InvalidData(
                "nearest-neighbor stream count exceeds max_streams".to_owned(),
            ));
        }
        budget.reserve(stream_count)?;
        for _ in 0..stream_count {
            let stream_id = read_string(reader, budget)?;
            if model.streams.contains_key(&stream_id) {
                return Err(PersistenceError::InvalidData(
                    "duplicate nearest-neighbor stream id".to_owned(),
                ));
            }
            let stream = NeighborStream::load(reader, &model.config, budget)?;
            model.streams.insert(stream_id, stream);
        }
        let mut trailing = [0_u8; 1];
        if reader.read(&mut trailing)? != 0 {
            return Err(PersistenceError::InvalidData(
                "trailing bytes after nearest-neighbor state".to_owned(),
            ));
        }
        Ok(model)
    }

    fn reserve_state_budget(&self, budget: &mut LoadBudget) -> io::Result<()> {
        if self.streams.len() > self.config.max_streams {
            return Err(invalid_data(
                "nearest-neighbor stream count exceeds max_streams",
            ));
        }
        budget.reserve(self.streams.len())?;
        for (stream_id, stream) in &self.streams {
            budget.reserve(stream_id.len())?;
            budget.reserve(stream.representatives.len())?;
            for representative in &stream.representatives {
                budget.reserve(representative.values.len())?;
            }
            budget.reserve(stream.patterns.len())?;
            for key in stream.patterns.keys() {
                budget.reserve(key.len())?;
            }
        }
        Ok(())
    }

    fn validate_save_budget(&self) -> io::Result<()> {
        let mut budget = LoadBudget::new();
        self.reserve_state_budget(&mut budget)
    }
}

impl Model for NearestNeighborModel {
    type Diagnostics = NearestNeighborDiagnostics;
    type Update = NearestNeighborUpdate;

    fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        NearestNeighborModel::evaluate(self, stream_id, features, at)
    }

    fn commit(&mut self, update: Self::Update, weight: f32) -> Result<(), ModelError> {
        NearestNeighborModel::commit(self, update, weight)
    }

    fn identity(&self) -> ModelIdentity {
        NearestNeighborModel::identity(self)
    }

    fn fingerprint(&self) -> u64 {
        NearestNeighborModel::fingerprint(self)
    }
}

impl PersistentModel for NearestNeighborModel {
    fn state_format_version(&self) -> u16 {
        NEAREST_STATE_VERSION
    }

    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()> {
        self.validate_save_budget()?;
        writer.write_all(NEAREST_MAGIC)?;
        write_u16(writer, NEAREST_STATE_VERSION)?;
        write_u32(writer, self.config.model_version)?;
        write_u32(writer, self.config.feature_schema_version)?;
        write_u64(writer, self.fingerprint())?;
        write_config(writer, &self.config)?;
        write_u64(writer, self.streams.len() as u64)?;
        for (stream_id, stream) in &self.streams {
            write_string(writer, stream_id)?;
            stream.save(writer)?;
        }
        Ok(())
    }

    fn load_state(&mut self, reader: &mut dyn Read) -> io::Result<()> {
        let loaded = Self::load_dyn(reader).map_err(persistence_to_io)?;
        if loaded.identity() != self.identity() || loaded.fingerprint() != self.fingerprint() {
            return Err(invalid_data(
                "nearest-neighbor identity or configuration mismatch",
            ));
        }
        *self = loaded;
        Ok(())
    }

    fn reserve_load_budget(&self, load_budget: &mut LoadBudget) -> io::Result<()> {
        self.reserve_state_budget(load_budget)
    }

    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()> {
        let loaded = Self::load_dyn_with_budget(reader, load_budget).map_err(persistence_to_io)?;
        if loaded.identity() != self.identity() || loaded.fingerprint() != self.fingerprint() {
            return Err(invalid_data(
                "nearest-neighbor identity or configuration mismatch",
            ));
        }
        *self = loaded;
        Ok(())
    }
}

struct NeighborStream {
    representatives: Vec<Representative>,
    patterns: BTreeMap<Vec<u32>, PatternRecord>,
    last_observed: Option<Timestamp>,
    observation_sequence: u64,
    generation: u64,
}

impl NeighborStream {
    fn new() -> Self {
        Self {
            representatives: Vec::new(),
            patterns: BTreeMap::new(),
            last_observed: None,
            observation_sequence: 0,
            generation: 0,
        }
    }

    fn stats(&self) -> NearestNeighborStreamStats {
        NearestNeighborStreamStats {
            representative_count: self.representatives.len(),
            pattern_count: self.patterns.len(),
            last_observed: self.last_observed,
        }
    }

    fn commit(
        &mut self,
        config: &NearestNeighborConfig,
        values: Vec<f32>,
        weight: f32,
        at: Timestamp,
        pattern_key: Vec<u32>,
    ) {
        let sequence = self.observation_sequence.saturating_add(1);
        if weight > 0.0 {
            if let Some((index, distance)) = self
                .representatives
                .iter()
                .enumerate()
                .map(|(index, representative)| {
                    (index, cosine_distance(&representative.values, &values))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1))
                && distance <= config.merge_distance
            {
                let representative = &mut self.representatives[index];
                let total_weight = representative.weight + weight;
                if total_weight.is_finite() && total_weight > 0.0 {
                    for (current, next) in representative.values.iter_mut().zip(values.iter()) {
                        *current =
                            (*current * representative.weight + *next * weight) / total_weight;
                    }
                    representative.weight = total_weight;
                    representative.last_seen = at;
                }
            } else {
                self.representatives.push(Representative {
                    values,
                    weight,
                    last_seen: at,
                });
                while self.representatives.len() > config.max_representatives {
                    let oldest = self
                        .representatives
                        .iter()
                        .enumerate()
                        .min_by(|(_, left), (_, right)| {
                            left.weight
                                .total_cmp(&right.weight)
                                .then_with(|| left.last_seen.cmp(&right.last_seen))
                        })
                        .map(|(index, _)| index)
                        .unwrap_or(0);
                    self.representatives.remove(oldest);
                }
            }
        }
        if !self.patterns.contains_key(&pattern_key)
            && self.patterns.len() >= config.max_patterns
            && let Some(oldest) = self.oldest_pattern()
        {
            self.patterns.remove(&oldest);
        }
        self.patterns.insert(
            pattern_key,
            PatternRecord {
                last_seen: at,
                last_seen_sequence: sequence,
            },
        );
        self.last_observed = Some(at);
        self.observation_sequence = sequence;
    }

    fn oldest_pattern(&self) -> Option<Vec<u32>> {
        self.patterns
            .iter()
            .min_by(|(left_key, left), (right_key, right)| {
                left.last_seen
                    .cmp(&right.last_seen)
                    .then_with(|| left.last_seen_sequence.cmp(&right.last_seen_sequence))
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(key, _)| key.clone())
    }

    fn save(&self, writer: &mut dyn Write) -> io::Result<()> {
        write_u8(writer, u8::from(self.last_observed.is_some()))?;
        if let Some(last_observed) = self.last_observed {
            write_timestamp(writer, last_observed)?;
        }
        write_u64(writer, self.observation_sequence)?;
        write_u64(writer, self.generation)?;
        write_u64(writer, self.representatives.len() as u64)?;
        for representative in &self.representatives {
            write_f32(writer, representative.weight)?;
            write_timestamp(writer, representative.last_seen)?;
            write_u64(writer, representative.values.len() as u64)?;
            for value in &representative.values {
                write_f32(writer, *value)?;
            }
        }
        write_u64(writer, self.patterns.len() as u64)?;
        for (key, record) in &self.patterns {
            write_u64(writer, key.len() as u64)?;
            for bits in key {
                write_u32(writer, *bits)?;
            }
            write_timestamp(writer, record.last_seen)?;
            write_u64(writer, record.last_seen_sequence)?;
        }
        Ok(())
    }

    fn load(
        reader: &mut dyn Read,
        config: &NearestNeighborConfig,
        budget: &mut LoadBudget,
    ) -> io::Result<Self> {
        let mut stream = Self::new();
        let has_last = read_u8(reader)?;
        if has_last > 1 {
            return Err(invalid_data("invalid nearest-neighbor last-observed flag"));
        }
        stream.last_observed = (has_last == 1)
            .then(|| read_timestamp(reader))
            .transpose()?;
        stream.observation_sequence = read_u64(reader)?;
        stream.generation = read_u64(reader)?;
        let representative_count = read_count(reader)?;
        if representative_count > config.max_representatives {
            return Err(invalid_data(
                "nearest-neighbor representative count exceeds configured limit",
            ));
        }
        budget.reserve(representative_count)?;
        for _ in 0..representative_count {
            let weight = read_f32(reader)?;
            if !weight.is_finite() || weight <= 0.0 {
                return Err(invalid_data("nearest-neighbor weight is invalid"));
            }
            let last_seen = read_timestamp(reader)?;
            let dimensions = read_count(reader)?;
            if dimensions != config.dimensions {
                return Err(invalid_data(
                    "nearest-neighbor representative dimension mismatch",
                ));
            }
            budget.reserve(dimensions)?;
            let mut values = Vec::with_capacity(dimensions);
            for _ in 0..dimensions {
                let value = read_f32(reader)?;
                if !value.is_finite() {
                    return Err(invalid_data("nearest-neighbor value is not finite"));
                }
                values.push(value);
            }
            if norm(&values) == 0.0 {
                return Err(invalid_data(
                    "nearest-neighbor representative has zero norm",
                ));
            }
            stream.representatives.push(Representative {
                values,
                weight,
                last_seen,
            });
        }
        let pattern_count = read_count(reader)?;
        if pattern_count > config.max_patterns {
            return Err(invalid_data(
                "nearest-neighbor pattern count exceeds configured limit",
            ));
        }
        budget.reserve(pattern_count)?;
        for _ in 0..pattern_count {
            let key_count = read_count(reader)?;
            if key_count != config.dimensions {
                return Err(invalid_data("nearest-neighbor pattern dimension mismatch"));
            }
            budget.reserve(key_count)?;
            let mut key = Vec::with_capacity(key_count);
            for _ in 0..key_count {
                key.push(read_u32(reader)?);
            }
            let last_seen = read_timestamp(reader)?;
            let last_seen_sequence = read_u64(reader)?;
            if stream
                .patterns
                .insert(
                    key,
                    PatternRecord {
                        last_seen,
                        last_seen_sequence,
                    },
                )
                .is_some()
            {
                return Err(invalid_data("duplicate nearest-neighbor pattern"));
            }
        }
        if stream.last_observed.is_none()
            && (!stream.representatives.is_empty() || !stream.patterns.is_empty())
        {
            return Err(invalid_data(
                "nearest-neighbor state has data without timestamp",
            ));
        }
        if stream.last_observed.is_some() && stream.observation_sequence == 0 {
            return Err(invalid_data(
                "nearest-neighbor state has a timestamp without an observation sequence",
            ));
        }
        if let Some(last_observed) = stream.last_observed
            && (stream
                .representatives
                .iter()
                .any(|representative| representative.last_seen > last_observed)
                || stream.patterns.values().any(|record| {
                    record.last_seen > last_observed
                        || record.last_seen_sequence == 0
                        || record.last_seen_sequence > stream.observation_sequence
                }))
        {
            return Err(invalid_data(
                "nearest-neighbor state contains an invalid sequence or timestamp",
            ));
        }
        Ok(stream)
    }
}

struct Representative {
    values: Vec<f32>,
    weight: f32,
    last_seen: Timestamp,
}

struct PatternRecord {
    last_seen: Timestamp,
    last_seen_sequence: u64,
}

fn norm(values: &[f32]) -> f32 {
    values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt() as f32
}

fn cosine_distance(left: &[f32], right: &[f32]) -> f32 {
    let left_norm = norm(left);
    let right_norm = norm(right);
    if left_norm == 0.0 || right_norm == 0.0 {
        return 2.0;
    }
    let dot = left
        .iter()
        .zip(right.iter())
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum::<f64>();
    (1.0 - dot / f64::from(left_norm) / f64::from(right_norm)).clamp(0.0, 2.0) as f32
}

fn persistence_to_io(error: PersistenceError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn write_config(writer: &mut dyn Write, config: &NearestNeighborConfig) -> io::Result<()> {
    write_u64(writer, config.dimensions as u64)?;
    write_u64(writer, config.max_representatives as u64)?;
    write_f32(writer, config.merge_distance)?;
    write_u64(writer, config.max_patterns as u64)?;
    write_u64(writer, config.max_streams as u64)
}

fn read_config(
    reader: &mut dyn Read,
    model_version: u32,
    feature_schema_version: u32,
) -> Result<NearestNeighborConfig, PersistenceError> {
    let config = NearestNeighborConfig {
        dimensions: read_usize(reader)?,
        max_representatives: read_usize(reader)?,
        merge_distance: read_f32(reader)?,
        max_patterns: read_usize(reader)?,
        max_streams: read_usize(reader)?,
        model_version,
        feature_schema_version,
    };
    config
        .validate()
        .map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
    Ok(config)
}

fn read_usize(reader: &mut dyn Read) -> io::Result<usize> {
    usize::try_from(read_u64(reader)?)
        .map_err(|_| invalid_data("nearest-neighbor integer does not fit usize"))
}

/// Errors returned by NearestNeighborModel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NearestNeighborError {
    /// A model parameter is invalid.
    InvalidParameter {
        /// Name of the invalid parameter.
        name: &'static str,
    },
    /// The feature vector is empty.
    EmptyFeatures,
    /// The feature vector has the wrong dimension.
    DimensionMismatch {
        /// Expected input dimension.
        expected: usize,
        /// Actual input dimension.
        actual: usize,
    },
    /// A feature is not finite.
    NonFiniteFeature {
        /// Index of the non-finite feature.
        index: usize,
    },
    /// Cosine distance is undefined for a zero vector.
    ZeroNorm,
    /// The timestamp goes backwards within a stream.
    TimestampOutOfOrder {
        /// Previous stream timestamp.
        previous: Timestamp,
        /// Current timestamp.
        current: Timestamp,
    },
    /// The model reached its stream limit.
    TooManyStreams,
}

impl NearestNeighborError {
    fn into_model_error(self) -> ModelError {
        ModelError::Incompatible(self.to_string())
    }
}

impl fmt::Display for NearestNeighborError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParameter { name } => {
                write!(formatter, "nearest-neighbor parameter {name} is invalid")
            }
            Self::EmptyFeatures => write!(formatter, "nearest-neighbor features must not be empty"),
            Self::DimensionMismatch { expected, actual } => {
                write!(
                    formatter,
                    "nearest-neighbor expected {expected} dimensions, got {actual}"
                )
            }
            Self::NonFiniteFeature { index } => {
                write!(formatter, "nearest-neighbor feature {index} is not finite")
            }
            Self::ZeroNorm => write!(
                formatter,
                "nearest-neighbor cosine input must have non-zero norm"
            ),
            Self::TimestampOutOfOrder { previous, current } => {
                write!(
                    formatter,
                    "nearest-neighbor timestamp {current} precedes {previous}"
                )
            }
            Self::TooManyStreams => write!(formatter, "nearest-neighbor stream limit was reached"),
        }
    }
}

impl std::error::Error for NearestNeighborError {}
