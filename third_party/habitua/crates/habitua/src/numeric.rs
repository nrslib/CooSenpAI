use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Duration;

use crate::config::{MAX_CONFIG_DIMENSIONS, MAX_CONFIG_PATTERNS, MAX_CONFIG_STREAMS};
use crate::model::{
    Evaluation, Model, ModelError, ModelIdentity, PersistentModel, validate_learning_weight,
};
use crate::pattern::{PatternId, stable_hash};
use crate::persistence::{
    LoadBudget, PersistenceError, read_count, read_duration, read_f32, read_string, read_timestamp,
    read_u8, read_u16, read_u32, read_u64, write_duration, write_f32, write_string,
    write_timestamp, write_u8, write_u16, write_u32, write_u64,
};
use crate::reaction::Readiness;
use crate::timestamp::Timestamp;

const NUMERIC_ALGORITHM_VERSION: u64 = 2;
const NUMERIC_STATE_VERSION: u16 = 1;
const NUMERIC_MAGIC: &[u8; 8] = b"NUMERIC\0";
const MAD_TO_STANDARD_DEVIATION: f64 = 1.4826;
static NEXT_NUMERIC_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Direction in which a numeric value is considered abnormal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericDirection {
    /// Only values above the baseline are abnormal.
    Up,
    /// Only values below the baseline are abnormal.
    Down,
    /// Both sides of the baseline are abnormal.
    Both,
}

/// Configuration for NumericDeviationModel.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericDeviationConfig {
    /// Minimum denominator used when converting a MAD to a z-score.
    pub mad_floor: f32,
    /// Maximum contribution of one committed sample before commit weight.
    pub learning_rate: f32,
    /// Maximum number of weighted samples retained per stream.
    pub max_samples: usize,
    /// Maximum number of streams retained by the model.
    pub max_streams: usize,
    /// Duration covered by the baseline sample window.
    pub sample_window: Duration,
    /// Number of retained samples required before a baseline is ready.
    pub minimum_samples: usize,
    /// Side of the baseline that produces a deviation.
    pub direction: NumericDirection,
    /// Maximum number of exact value patterns retained for recency evidence.
    pub max_patterns: usize,
    /// Version of the numeric model algorithm.
    pub model_version: u32,
    /// Version of the adapter-defined numeric feature schema.
    pub feature_schema_version: u32,
}

impl Default for NumericDeviationConfig {
    fn default() -> Self {
        Self {
            mad_floor: 0.001,
            learning_rate: 1.0,
            max_samples: 256,
            max_streams: 4_096,
            sample_window: Duration::from_secs(30 * 60),
            minimum_samples: 3,
            direction: NumericDirection::Both,
            max_patterns: 4_096,
            model_version: 1,
            feature_schema_version: 1,
        }
    }
}

impl NumericDeviationConfig {
    fn validate(&self) -> Result<(), NumericDeviationError> {
        if !self.mad_floor.is_finite() || self.mad_floor <= 0.0 {
            return Err(NumericDeviationError::InvalidParameter { name: "mad_floor" });
        }
        if !self.learning_rate.is_finite() || !(0.0..=1.0).contains(&self.learning_rate) {
            return Err(NumericDeviationError::InvalidParameter {
                name: "learning_rate",
            });
        }
        if self.max_samples == 0
            || self.max_streams == 0
            || self.max_patterns == 0
            || self.sample_window.is_zero()
            || self.minimum_samples == 0
        {
            return Err(NumericDeviationError::InvalidParameter {
                name: "retention_limits",
            });
        }
        if self.max_samples > MAX_CONFIG_PATTERNS
            || self.max_patterns > MAX_CONFIG_PATTERNS
            || self.max_streams > MAX_CONFIG_STREAMS
        {
            return Err(NumericDeviationError::InvalidParameter {
                name: "retention_limits",
            });
        }
        if self.minimum_samples > self.max_samples {
            return Err(NumericDeviationError::InvalidParameter {
                name: "minimum_samples",
            });
        }
        if self.model_version == 0 {
            return Err(NumericDeviationError::InvalidParameter {
                name: "model_version",
            });
        }
        if self.feature_schema_version == 0 {
            return Err(NumericDeviationError::InvalidParameter {
                name: "feature_schema_version",
            });
        }
        Ok(())
    }
}

/// A named deviation and its baseline evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedDeviation {
    /// Name supplied when the model was constructed.
    pub name: String,
    /// Current value before learning this observation.
    pub current: f32,
    /// Weighted median of the time-window baseline.
    pub baseline: Option<f32>,
    /// Median absolute deviation of the baseline.
    pub mad: Option<f32>,
    /// One-sided or two-sided standardized deviation.
    pub z_plus: f32,
    /// Configured direction used for z_plus.
    pub direction: NumericDirection,
    /// Number of retained samples used by this baseline.
    pub sample_count: usize,
    /// Time window used for the baseline.
    pub window: Duration,
}

/// Diagnostics returned by NumericDeviationModel.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericDeviationDiagnostics {
    /// One deviation for every configured numeric name, in input order.
    pub deviations: Vec<NamedDeviation>,
    /// Total sample weight in the shared pre-observation baseline window.
    pub baseline_weight: f32,
    /// Whether the minimum sample condition was met.
    pub baseline_ready: bool,
    /// Number of retained samples in the active time window.
    pub sample_count: usize,
    /// Duration represented by the baseline.
    pub window: Duration,
}

/// Statistics for one numeric stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericStreamStats {
    /// Number of retained weighted samples.
    pub sample_count: usize,
    /// Number of retained exact value patterns.
    pub pattern_count: usize,
    /// Latest committed timestamp.
    pub last_observed: Option<Timestamp>,
}

/// The uncommitted update returned by NumericDeviationModel.
pub struct NumericUpdate {
    stream_id: String,
    expected_generation: Option<u64>,
    model_token: u64,
    configuration_fingerprint: u64,
    input_dimensions: usize,
    values: Vec<f32>,
    at: Timestamp,
    pattern_key: Vec<u32>,
}

/// A robust, bounded numeric deviation detector.
///
/// For every named value, evaluation uses only samples from the preceding
/// sample_window and calculates the baseline before the current value is
/// learned:
///
/// z+ = max(0, (x - median) / max(1.4826 * MAD, mad_floor))
///
/// Down substitutes median - x, and Both takes the larger of the two
/// one-sided values. The weighted median uses the lower median for an even
/// total weight: it returns the first sorted value whose accumulated weight
/// reaches half the total. Thus [50, 55] has median 50; this deterministic
/// rule is part of the model fingerprint.
pub struct NumericDeviationModel {
    names: Vec<String>,
    config: NumericDeviationConfig,
    streams: BTreeMap<String, NumericStream>,
    model_token: u64,
}

impl NumericDeviationModel {
    /// Creates a model with NumericDeviationConfig::default.
    pub fn new<I, S>(names: I) -> Result<Self, NumericDeviationError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::with_config(names, NumericDeviationConfig::default())
    }

    /// Creates a model with explicit names and configuration.
    pub fn with_config<I, S>(
        names: I,
        config: NumericDeviationConfig,
    ) -> Result<Self, NumericDeviationError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        config.validate()?;
        let names = names.into_iter().map(Into::into).collect::<Vec<_>>();
        if names.is_empty() {
            return Err(NumericDeviationError::EmptyNames);
        }
        if names.len() > MAX_CONFIG_DIMENSIONS {
            return Err(NumericDeviationError::InvalidParameter { name: "names" });
        }
        if names.iter().any(String::is_empty) {
            return Err(NumericDeviationError::EmptyName);
        }
        let mut sorted_names = names.clone();
        sorted_names.sort();
        if sorted_names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(NumericDeviationError::DuplicateName);
        }
        Ok(Self {
            names,
            config,
            streams: BTreeMap::new(),
            model_token: next_numeric_token(),
        })
    }

    /// Returns the configured numeric names in input order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Returns the model configuration.
    pub fn config(&self) -> &NumericDeviationConfig {
        &self.config
    }

    /// Returns statistics for a stream, if it exists.
    pub fn stats(&self, stream_id: &str) -> Option<NumericStreamStats> {
        self.streams.get(stream_id).map(NumericStream::stats)
    }

    /// Removes one stream and returns whether it existed.
    pub fn forget(&mut self, stream_id: &str) -> bool {
        self.streams.remove(stream_id).is_some()
    }

    /// Evaluates values without changing model state.
    pub fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: Timestamp,
    ) -> Result<Evaluation<NumericDeviationDiagnostics, NumericUpdate>, ModelError> {
        self.validate_features(features)?;
        let stream = self.streams.get(stream_id);
        if let Some(stream) = stream
            && let Some(previous) = stream.last_observed
            && at < previous
        {
            return Err(NumericDeviationError::TimestampOutOfOrder {
                previous,
                current: at,
            }
            .into_model_error());
        }

        let mut deviations = Vec::with_capacity(self.names.len());
        let mut baseline_weight = 0.0_f32;
        for (index, name) in self.names.iter().enumerate() {
            let baseline = stream.and_then(|state| state.baseline(index, at, &self.config));
            let (median, mad, weight, sample_count) = baseline
                .map(|value| {
                    (
                        Some(value.median),
                        Some(value.mad),
                        value.weight,
                        value.count,
                    )
                })
                .unwrap_or((None, None, 0.0, 0));
            if index == 0 {
                baseline_weight = weight;
            }
            let z_plus = match (median, mad) {
                (Some(median), Some(mad)) => {
                    let denominator = (MAD_TO_STANDARD_DEVIATION * f64::from(mad))
                        .max(f64::from(self.config.mad_floor));
                    let delta_up = (f64::from(features[index]) - f64::from(median)).max(0.0);
                    let delta_down = (f64::from(median) - f64::from(features[index])).max(0.0);
                    let delta = match self.config.direction {
                        NumericDirection::Up => delta_up,
                        NumericDirection::Down => delta_down,
                        NumericDirection::Both => delta_up.max(delta_down),
                    };
                    finite_f32(delta / denominator)
                }
                _ => 0.0,
            };
            deviations.push(NamedDeviation {
                name: name.clone(),
                current: features[index],
                baseline: median,
                mad,
                z_plus,
                direction: self.config.direction,
                sample_count,
                window: self.config.sample_window,
            });
        }
        let sample_count = stream
            .map(|state| state.windowed_sample_count(at, self.config.sample_window))
            .unwrap_or(0);
        let baseline_ready = sample_count >= self.config.minimum_samples;
        let pattern_key = features
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>();
        let pattern_id = numeric_pattern_id(&self.names, features);
        let recency = stream.and_then(|state| {
            state
                .patterns
                .get(&pattern_key)
                .and_then(|record| at.checked_duration_since(record.last_seen))
        });
        Ok(Evaluation {
            novelty_short: None,
            novelty_long: None,
            familiarity_short: None,
            familiarity_long: None,
            recency,
            pattern_id: Some(pattern_id),
            readiness: if baseline_ready {
                Readiness::Evaluated
            } else {
                Readiness::BaselineInsufficient
            },
            diagnostics: NumericDeviationDiagnostics {
                deviations,
                baseline_weight,
                baseline_ready,
                sample_count,
                window: self.config.sample_window,
            },
            update: NumericUpdate {
                stream_id: stream_id.to_owned(),
                expected_generation: stream.map(|state| state.generation),
                model_token: self.model_token,
                configuration_fingerprint: self.fingerprint(),
                input_dimensions: self.names.len(),
                values: features.to_vec(),
                at,
                pattern_key,
            },
        })
    }

    /// Commits a numeric evaluation update with a learning weight.
    pub fn commit(&mut self, update: NumericUpdate, weight: f32) -> Result<(), ModelError> {
        validate_learning_weight(weight)?;
        let NumericUpdate {
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
            || input_dimensions != self.names.len()
            || values.len() != self.names.len()
        {
            return Err(ModelError::ForeignUpdate);
        }
        let actual_generation = self.streams.get(&stream_id).map(|state| state.generation);
        if actual_generation != expected_generation {
            return Err(ModelError::StaleUpdate);
        }
        let next_generation = expected_generation
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ModelError::UpdateSequenceOverflow)?;
        let sample_weight = self.config.learning_rate * weight;
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.commit(values, sample_weight, at, pattern_key, &self.config);
            stream.generation = next_generation;
        } else {
            if self.streams.len() >= self.config.max_streams {
                return Err(NumericDeviationError::TooManyStreams.into_model_error());
            }
            let mut stream = NumericStream::new();
            stream.commit(values, sample_weight, at, pattern_key, &self.config);
            stream.generation = next_generation;
            self.streams.insert(stream_id, stream);
        }
        Ok(())
    }

    fn validate_features(&self, features: &[f32]) -> Result<(), ModelError> {
        if features.is_empty() {
            return Err(NumericDeviationError::EmptyFeatures.into_model_error());
        }
        if features.len() != self.names.len() {
            return Err(NumericDeviationError::DimensionMismatch {
                expected: self.names.len(),
                actual: features.len(),
            }
            .into_model_error());
        }
        if let Some(index) = features.iter().position(|value| !value.is_finite()) {
            return Err(NumericDeviationError::NonFiniteFeature { index }.into_model_error());
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
        let mut words = vec![
            NUMERIC_ALGORITHM_VERSION,
            u64::from(self.config.mad_floor.to_bits()),
            u64::from(self.config.learning_rate.to_bits()),
            self.config.max_samples as u64,
            self.config.max_streams as u64,
            self.config.sample_window.as_secs(),
            u64::from(self.config.sample_window.subsec_nanos()),
            self.config.minimum_samples as u64,
            self.config.max_patterns as u64,
            u64::from(self.config.model_version),
            u64::from(self.config.feature_schema_version),
            u64::from(self.config.direction as u8),
        ];
        words.extend(
            self.names
                .iter()
                .map(|name| stable_hash(name.bytes().map(u64::from))),
        );
        stable_hash(words)
    }

    /// Saves the complete model state in the numeric model format.
    pub fn save<W: Write>(&self, writer: &mut W) -> Result<(), PersistenceError> {
        self.validate_save_budget()
            .map_err(PersistenceError::from)?;
        self.save_state(writer).map_err(PersistenceError::from)
    }

    /// Loads a complete numeric model state transactionally.
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
        let mut magic = [0_u8; NUMERIC_MAGIC.len()];
        reader.read_exact(&mut magic)?;
        if &magic != NUMERIC_MAGIC {
            return Err(PersistenceError::InvalidMagic);
        }
        let version = read_u16(reader)?;
        if version != NUMERIC_STATE_VERSION {
            return Err(PersistenceError::UnsupportedVersion(version));
        }
        let model_version = read_u32(reader)?;
        let feature_schema_version = read_u32(reader)?;
        let saved_fingerprint = read_u64(reader)?;
        let config = read_numeric_config(reader, model_version, feature_schema_version)?;
        let name_count = read_count(reader)?;
        budget.reserve(name_count)?;
        let mut names = Vec::with_capacity(name_count);
        for _ in 0..name_count {
            names.push(read_string(reader, budget)?);
        }
        let mut model = Self::with_config(names, config)
            .map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
        if saved_fingerprint != model.fingerprint() {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "numeric model",
            });
        }
        let stream_count = read_count(reader)?;
        budget.reserve(stream_count)?;
        if stream_count > model.config.max_streams {
            return Err(PersistenceError::InvalidData(
                "numeric stream count exceeds max_streams".to_owned(),
            ));
        }
        for _ in 0..stream_count {
            let stream_id = read_string(reader, budget)?;
            if model.streams.contains_key(&stream_id) {
                return Err(PersistenceError::InvalidData(
                    "duplicate numeric stream id".to_owned(),
                ));
            }
            let stream = NumericStream::load(reader, &model.config, model.names.len(), budget)?;
            model.streams.insert(stream_id, stream);
        }
        let mut trailing = [0_u8; 1];
        if reader.read(&mut trailing)? != 0 {
            return Err(PersistenceError::InvalidData(
                "trailing bytes after numeric state".to_owned(),
            ));
        }
        Ok(model)
    }

    fn reserve_state_budget(&self, budget: &mut LoadBudget) -> io::Result<()> {
        budget.reserve(self.names.len())?;
        for name in &self.names {
            budget.reserve(name.len())?;
        }
        if self.streams.len() > self.config.max_streams {
            return Err(invalid_data("numeric stream count exceeds max_streams"));
        }
        budget.reserve(self.streams.len())?;
        for (stream_id, stream) in &self.streams {
            budget.reserve(stream_id.len())?;
            budget.reserve(stream.samples.len())?;
            for sample in &stream.samples {
                budget.reserve(sample.values.len())?;
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

impl Model for NumericDeviationModel {
    type Diagnostics = NumericDeviationDiagnostics;
    type Update = NumericUpdate;

    fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        NumericDeviationModel::evaluate(self, stream_id, features, at)
    }

    fn commit(&mut self, update: Self::Update, weight: f32) -> Result<(), ModelError> {
        NumericDeviationModel::commit(self, update, weight)
    }

    fn identity(&self) -> ModelIdentity {
        NumericDeviationModel::identity(self)
    }

    fn fingerprint(&self) -> u64 {
        NumericDeviationModel::fingerprint(self)
    }
}

impl PersistentModel for NumericDeviationModel {
    fn state_format_version(&self) -> u16 {
        NUMERIC_STATE_VERSION
    }

    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()> {
        self.validate_save_budget()?;
        writer.write_all(NUMERIC_MAGIC)?;
        write_u16(writer, NUMERIC_STATE_VERSION)?;
        write_u32(writer, self.config.model_version)?;
        write_u32(writer, self.config.feature_schema_version)?;
        write_u64(writer, self.fingerprint())?;
        write_numeric_config(writer, &self.config)?;
        write_u64(writer, self.names.len() as u64)?;
        for name in &self.names {
            write_string(writer, name)?;
        }
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
                "numeric model identity or configuration mismatch",
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
                "numeric model identity or configuration mismatch",
            ));
        }
        *self = loaded;
        Ok(())
    }
}

#[derive(Default)]
struct NumericStream {
    samples: VecDeque<WeightedSample>,
    patterns: BTreeMap<Vec<u32>, NumericPatternRecord>,
    last_observed: Option<Timestamp>,
    observation_sequence: u64,
    generation: u64,
}

impl NumericStream {
    fn new() -> Self {
        Self::default()
    }

    fn stats(&self) -> NumericStreamStats {
        NumericStreamStats {
            sample_count: self.samples.len(),
            pattern_count: self.patterns.len(),
            last_observed: self.last_observed,
        }
    }

    fn windowed_sample_count(&self, at: Timestamp, window: Duration) -> usize {
        self.samples
            .iter()
            .filter(|sample| {
                at.checked_duration_since(sample.at)
                    .is_some_and(|age| age <= window)
            })
            .count()
    }

    fn baseline(
        &self,
        index: usize,
        at: Timestamp,
        config: &NumericDeviationConfig,
    ) -> Option<Baseline> {
        let values = self
            .samples
            .iter()
            .filter(|sample| {
                at.checked_duration_since(sample.at)
                    .is_some_and(|age| age <= config.sample_window)
            })
            .map(|sample| (sample.values[index], sample.weight))
            .collect::<Vec<_>>();
        let weight = values.iter().map(|(_, weight)| *weight).sum::<f32>();
        if values.is_empty() || !weight.is_finite() || weight <= 0.0 {
            return None;
        }
        let median = weighted_median(values.clone(), weight);
        let deviations = values
            .into_iter()
            .map(|(value, weight)| ((value - median).abs(), weight))
            .collect::<Vec<_>>();
        let mad = weighted_median(deviations, weight);
        Some(Baseline {
            median,
            mad,
            weight,
            count: self.windowed_sample_count(at, config.sample_window),
        })
    }

    fn commit(
        &mut self,
        values: Vec<f32>,
        sample_weight: f32,
        at: Timestamp,
        pattern_key: Vec<u32>,
        config: &NumericDeviationConfig,
    ) {
        let sequence = self.observation_sequence.saturating_add(1);
        if sample_weight > 0.0 {
            while self.samples.front().is_some_and(|sample| {
                at.checked_duration_since(sample.at)
                    .is_some_and(|age| age > config.sample_window)
            }) {
                self.samples.pop_front();
            }
            if self.samples.len() >= config.max_samples {
                self.samples.pop_front();
            }
            self.samples.push_back(WeightedSample {
                values,
                weight: sample_weight,
                at,
            });
        }
        if !self.patterns.contains_key(&pattern_key)
            && self.patterns.len() >= config.max_patterns
            && let Some(oldest) = self.oldest_pattern()
        {
            self.patterns.remove(&oldest);
        }
        let consecutive =
            self.patterns
                .get(&pattern_key)
                .map_or(u32::from(sample_weight > 0.0), |record| {
                    if sample_weight > 0.0 {
                        record.consecutive_experiences.saturating_add(1)
                    } else {
                        record.consecutive_experiences
                    }
                });
        self.patterns.insert(
            pattern_key,
            NumericPatternRecord {
                last_seen: at,
                last_seen_sequence: sequence,
                consecutive_experiences: consecutive,
            },
        );
        self.last_observed = Some(at);
        self.observation_sequence = sequence;
    }

    fn oldest_pattern(&self) -> Option<Vec<u32>> {
        self.patterns
            .iter()
            .min_by(|(key_a, record_a), (key_b, record_b)| {
                record_a
                    .last_seen
                    .cmp(&record_b.last_seen)
                    .then_with(|| {
                        record_a
                            .last_seen_sequence
                            .cmp(&record_b.last_seen_sequence)
                    })
                    .then_with(|| key_a.cmp(key_b))
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
        write_u64(writer, self.samples.len() as u64)?;
        for sample in &self.samples {
            write_timestamp(writer, sample.at)?;
            write_f32(writer, sample.weight)?;
            write_u64(writer, sample.values.len() as u64)?;
            for value in &sample.values {
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
            write_u32(writer, record.consecutive_experiences)?;
        }
        Ok(())
    }

    fn load(
        reader: &mut dyn Read,
        config: &NumericDeviationConfig,
        dimensions: usize,
        budget: &mut LoadBudget,
    ) -> io::Result<Self> {
        let mut stream = Self::new();
        let has_last = read_u8(reader)?;
        if has_last > 1 {
            return Err(invalid_data("invalid numeric last-observed flag"));
        }
        stream.last_observed = (has_last == 1)
            .then(|| read_timestamp(reader))
            .transpose()?;
        stream.observation_sequence = read_u64(reader)?;
        stream.generation = read_u64(reader)?;
        let sample_count = read_count(reader)?;
        if sample_count > config.max_samples {
            return Err(invalid_data("numeric sample count exceeds max_samples"));
        }
        budget.reserve(sample_count)?;
        for _ in 0..sample_count {
            let at = read_timestamp(reader)?;
            let weight = read_f32(reader)?;
            if !weight.is_finite() || weight <= 0.0 {
                return Err(invalid_data("numeric sample weight is invalid"));
            }
            let value_count = read_count(reader)?;
            if value_count != dimensions {
                return Err(invalid_data("numeric sample dimension mismatch"));
            }
            budget.reserve(value_count)?;
            let mut values = Vec::with_capacity(value_count);
            for _ in 0..value_count {
                let value = read_f32(reader)?;
                if !value.is_finite() {
                    return Err(invalid_data("numeric sample contains a non-finite value"));
                }
                values.push(value);
            }
            stream
                .samples
                .push_back(WeightedSample { values, weight, at });
        }
        let pattern_count = read_count(reader)?;
        if pattern_count > config.max_patterns {
            return Err(invalid_data("numeric pattern count exceeds max_patterns"));
        }
        budget.reserve(pattern_count)?;
        for _ in 0..pattern_count {
            let key_count = read_count(reader)?;
            if key_count != dimensions {
                return Err(invalid_data("numeric pattern dimension mismatch"));
            }
            budget.reserve(key_count)?;
            let mut key = Vec::with_capacity(key_count);
            for _ in 0..key_count {
                key.push(read_u32(reader)?);
            }
            let last_seen = read_timestamp(reader)?;
            let last_seen_sequence = read_u64(reader)?;
            let consecutive_experiences = read_u32(reader)?;
            if stream
                .patterns
                .insert(
                    key,
                    NumericPatternRecord {
                        last_seen,
                        last_seen_sequence,
                        consecutive_experiences,
                    },
                )
                .is_some()
            {
                return Err(invalid_data("duplicate numeric pattern"));
            }
        }
        if stream.last_observed.is_none()
            && (!stream.samples.is_empty() || !stream.patterns.is_empty())
        {
            return Err(invalid_data("numeric state has data without a timestamp"));
        }
        if stream.last_observed.is_some() && stream.observation_sequence == 0 {
            return Err(invalid_data(
                "numeric state has a timestamp without an observation sequence",
            ));
        }
        if stream
            .samples
            .iter()
            .zip(stream.samples.iter().skip(1))
            .any(|(previous, current)| current.at < previous.at)
        {
            return Err(invalid_data("numeric samples are not in timestamp order"));
        }
        if let Some(last_observed) = stream.last_observed {
            for sample in &stream.samples {
                if sample.at > last_observed {
                    return Err(invalid_data(
                        "numeric sample is newer than last observation",
                    ));
                }
            }
            for record in stream.patterns.values() {
                if record.last_seen > last_observed
                    || record.last_seen_sequence == 0
                    || record.last_seen_sequence > stream.observation_sequence
                {
                    return Err(invalid_data(
                        "numeric pattern sequence or timestamp is invalid",
                    ));
                }
            }
        }
        Ok(stream)
    }
}

struct WeightedSample {
    values: Vec<f32>,
    weight: f32,
    at: Timestamp,
}

struct NumericPatternRecord {
    last_seen: Timestamp,
    last_seen_sequence: u64,
    consecutive_experiences: u32,
}

struct Baseline {
    median: f32,
    mad: f32,
    weight: f32,
    count: usize,
}

fn next_numeric_token() -> u64 {
    NEXT_NUMERIC_TOKEN.fetch_add(1, AtomicOrdering::Relaxed)
}

fn finite_f32(value: f64) -> f32 {
    if value >= f64::from(f32::MAX) {
        f32::MAX
    } else {
        value as f32
    }
}

fn weighted_median(mut values: Vec<(f32, f32)>, total_weight: f32) -> f32 {
    values.sort_unstable_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
    let target = total_weight / 2.0;
    let mut accumulated = 0.0;
    for (value, weight) in values {
        accumulated += weight;
        if accumulated >= target {
            return value;
        }
    }
    0.0
}

fn numeric_pattern_id(names: &[String], values: &[f32]) -> PatternId {
    let words = names.iter().zip(values).flat_map(|(name, value)| {
        [
            stable_hash(name.bytes().map(u64::from)),
            u64::from(value.to_bits()),
        ]
    });
    stable_hash(words)
}

fn write_numeric_config(writer: &mut dyn Write, config: &NumericDeviationConfig) -> io::Result<()> {
    write_f32(writer, config.mad_floor)?;
    write_f32(writer, config.learning_rate)?;
    write_u64(writer, config.max_samples as u64)?;
    write_u64(writer, config.max_streams as u64)?;
    write_duration(writer, config.sample_window)?;
    write_u64(writer, config.minimum_samples as u64)?;
    write_u8(writer, config.direction as u8)?;
    write_u64(writer, config.max_patterns as u64)
}

fn read_numeric_config(
    reader: &mut dyn Read,
    model_version: u32,
    feature_schema_version: u32,
) -> Result<NumericDeviationConfig, PersistenceError> {
    let mad_floor = read_f32(reader)?;
    let learning_rate = read_f32(reader)?;
    let max_samples = read_usize_checked(reader)?;
    let max_streams = read_usize_checked(reader)?;
    let sample_window = read_duration(reader)?;
    let minimum_samples = read_usize_checked(reader)?;
    let direction = match read_u8(reader)? {
        0 => NumericDirection::Up,
        1 => NumericDirection::Down,
        2 => NumericDirection::Both,
        _ => {
            return Err(PersistenceError::InvalidData(
                "invalid numeric direction".to_owned(),
            ));
        }
    };
    let max_patterns = read_usize_checked(reader)?;
    let config = NumericDeviationConfig {
        mad_floor,
        learning_rate,
        max_samples,
        max_streams,
        sample_window,
        minimum_samples,
        direction,
        max_patterns,
        model_version,
        feature_schema_version,
    };
    config
        .validate()
        .map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
    Ok(config)
}

fn read_usize_checked(reader: &mut dyn Read) -> io::Result<usize> {
    let value = read_u64(reader)?;
    usize::try_from(value).map_err(|_| invalid_data("numeric integer does not fit usize"))
}

fn persistence_to_io(error: PersistenceError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl NumericDeviationError {
    fn into_model_error(self) -> ModelError {
        ModelError::Numeric(self.to_string())
    }
}

impl From<NumericDeviationError> for ModelError {
    fn from(error: NumericDeviationError) -> Self {
        error.into_model_error()
    }
}

/// Errors returned by NumericDeviationModel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NumericDeviationError {
    /// No numeric names were supplied.
    EmptyNames,
    /// A numeric name was empty.
    EmptyName,
    /// Numeric names must be unique.
    DuplicateName,
    /// A numeric parameter is invalid.
    InvalidParameter {
        /// Name of the invalid numeric-model parameter.
        name: &'static str,
    },
    /// The input vector is empty.
    EmptyFeatures,
    /// The input vector length differs from the configured names.
    DimensionMismatch {
        /// Number of names configured for the model.
        expected: usize,
        /// Number of values supplied by the caller.
        actual: usize,
    },
    /// A feature is not finite.
    NonFiniteFeature {
        /// Index of the non-finite value.
        index: usize,
    },
    /// The timestamp precedes the last committed timestamp for the stream.
    TimestampOutOfOrder {
        /// Last committed timestamp.
        previous: Timestamp,
        /// Timestamp supplied by the caller.
        current: Timestamp,
    },
    /// The model has reached its configured stream limit.
    TooManyStreams,
}

impl fmt::Display for NumericDeviationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyNames => write!(formatter, "numeric model requires at least one name"),
            Self::EmptyName => write!(formatter, "numeric model names must not be empty"),
            Self::DuplicateName => write!(formatter, "numeric model names must be unique"),
            Self::InvalidParameter { name } => {
                write!(formatter, "numeric parameter {name} is invalid")
            }
            Self::EmptyFeatures => write!(formatter, "numeric feature vector must not be empty"),
            Self::DimensionMismatch { expected, actual } => {
                write!(
                    formatter,
                    "expected {expected} numeric features, got {actual}"
                )
            }
            Self::NonFiniteFeature { index } => {
                write!(formatter, "numeric feature at index {index} is not finite")
            }
            Self::TimestampOutOfOrder { previous, current } => write!(
                formatter,
                "numeric timestamp {current} precedes previous timestamp {previous}"
            ),
            Self::TooManyStreams => write!(formatter, "numeric model stream limit was reached"),
        }
    }
}

impl std::error::Error for NumericDeviationError {}
