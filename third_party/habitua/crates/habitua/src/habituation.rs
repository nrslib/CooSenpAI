use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::config::{Config, ConfigError};
use crate::decaying_memory::{DecayingMemory, MemoryConfigError};
use crate::encoder::{EncodeError, Encoder, EncoderConfigError, FlyHashEncoder};
use crate::memory::{
    Memory, MemoryError, MemoryObservation, MemoryObservationError, MemoryStats, PersistentMemory,
};
use crate::model::{
    Evaluation, Model, ModelError, ModelIdentity, ObserveError, PersistentModel,
    validate_learning_weight,
};
use crate::pattern::{Pattern, PatternId, stable_hash};
use crate::persistence::{
    CURRENT_FORMAT_VERSION, INTERMEDIATE_FORMAT_VERSION, LEGACY_FORMAT_VERSION, LoadBudget,
    PREVIOUS_FORMAT_VERSION, PersistenceError, read_config, read_count, read_header, read_string,
    read_u64, write_config, write_header, write_string, write_u64,
};
use crate::profile::FeatureProfile;
use crate::timestamp::Timestamp;

/// Diagnostics produced by the built-in Fly model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlyDiagnostics {
    /// Normalized root-mean-square magnitude of the input vector.
    pub input_strength: f32,
    /// Deviation from the stream's running feature profile.
    pub profile_deviation: f32,
}

/// An uncommitted update produced by a [`Habituation`] evaluation.
pub struct FlyUpdate<U, M> {
    stream_id: String,
    expected_generation: Option<u64>,
    model_token: u64,
    configuration_fingerprint: u64,
    input_dimensions: usize,
    features: Vec<f32>,
    estimated_memory_units: usize,
    memory_update: U,
    new_memory: Option<M>,
}

/// Public statistics for one stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamStats {
    /// Number of distinct patterns retained by the stream.
    pub pattern_count: usize,
    /// Number of units with at least one memory trace.
    pub remembered_unit_count: usize,
    /// Aggregate short-term familiarity at the latest observation time.
    pub short_familiarity: f32,
    /// Aggregate long-term familiarity at the latest observation time.
    pub long_familiarity: f32,
    /// Latest timestamp handled by the stream.
    pub last_observed: Option<Timestamp>,
}

impl From<MemoryStats> for StreamStats {
    fn from(stats: MemoryStats) -> Self {
        Self {
            pattern_count: stats.pattern_count,
            remembered_unit_count: stats.remembered_unit_count,
            short_familiarity: stats.short_familiarity,
            long_familiarity: stats.long_familiarity,
            last_observed: stats.last_observed,
        }
    }
}

/// A generic Fly-style habituation implementation composed from an encoder
/// and a per-stream memory factory.
pub struct Habituation<E, M> {
    encoder: E,
    memory_factory: Box<dyn Fn() -> Result<M, MemoryError> + Send + Sync>,
    memory_fingerprint: u64,
    version_three_memory_fingerprint: u64,
    previous_memory_fingerprint: u64,
    legacy_memory_fingerprint: u64,
    model_token: u64,
    config: Config,
    streams: BTreeMap<String, StreamState<M>>,
}

impl<E, M> Habituation<E, M>
where
    E: Encoder,
    M: Memory,
{
    /// Creates an empty engine from an encoder, a per-stream memory factory,
    /// and shared configuration.
    ///
    /// The first factory result establishes the memory fingerprint and must be
    /// empty. Every later result is checked for both the same fingerprint and
    /// emptiness before it is assigned to a new stream.
    pub fn new<F>(encoder: E, memory_factory: F, config: Config) -> Result<Self, HabituationError>
    where
        F: Fn() -> Result<M, MemoryError> + Send + Sync + 'static,
    {
        Self::new_with_validation(encoder, memory_factory, config, false)
    }

    fn new_with_validation<F>(
        encoder: E,
        memory_factory: F,
        config: Config,
        allow_legacy_policy: bool,
    ) -> Result<Self, HabituationError>
    where
        F: Fn() -> Result<M, MemoryError> + Send + Sync + 'static,
    {
        config.validate_for_format(allow_legacy_policy)?;
        let actual_dimensions = encoder.input_dimensions();
        if actual_dimensions != config.dimensions {
            return Err(ConfigError::EncoderDimensionMismatch {
                expected: config.dimensions,
                actual: actual_dimensions,
            }
            .into());
        }
        let memory_factory: Box<dyn Fn() -> Result<M, MemoryError> + Send + Sync> =
            Box::new(memory_factory);
        let first_memory = (memory_factory)()?;
        first_memory.validate_config(&config)?;
        if !first_memory.is_empty() {
            return Err(MemoryError::FactoryReturnedNonEmpty.into());
        }
        let memory_fingerprint = first_memory.fingerprint();
        let version_three_memory_fingerprint =
            first_memory.fingerprint_for_format(crate::persistence::SEQUENCED_FORMAT_VERSION);
        let previous_memory_fingerprint =
            first_memory.fingerprint_for_format(PREVIOUS_FORMAT_VERSION);
        let legacy_memory_fingerprint =
            first_memory.fingerprint_for_format(INTERMEDIATE_FORMAT_VERSION);
        Ok(Self {
            encoder,
            memory_factory,
            memory_fingerprint,
            version_three_memory_fingerprint,
            previous_memory_fingerprint,
            legacy_memory_fingerprint,
            model_token: next_model_token(),
            config,
            streams: BTreeMap::new(),
        })
    }

    /// Returns the configuration used by this engine.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Evaluates a vector without changing engine state.
    pub fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: Timestamp,
    ) -> Result<Evaluation<FlyDiagnostics, FlyUpdate<M::Update, M>>, ObserveError> {
        self.validate_features(features)?;
        let pattern = self.encoder.encode(features)?;
        if let Some(state) = self.streams.get(stream_id) {
            let evaluated = evaluate_stream(state, &pattern, features, at, &self.config)?;
            Ok(Evaluation {
                novelty_short: Some(1.0 - evaluated.memory_observation.short_familiarity),
                novelty_long: Some(1.0 - evaluated.memory_observation.long_familiarity),
                familiarity_short: Some(evaluated.memory_observation.short_familiarity),
                familiarity_long: Some(evaluated.memory_observation.long_familiarity),
                recency: evaluated.memory_observation.recency,
                pattern_id: Some(evaluated.memory_observation.pattern_id),
                readiness: crate::reaction::Readiness::Evaluated,
                diagnostics: FlyDiagnostics {
                    input_strength: input_strength(features, self.config.input_strength_scale),
                    profile_deviation: evaluated.profile_deviation,
                },
                update: FlyUpdate {
                    stream_id: stream_id.to_owned(),
                    expected_generation: Some(state.generation),
                    model_token: self.model_token,
                    configuration_fingerprint: self.configuration_fingerprint(),
                    input_dimensions: self.config.dimensions,
                    features: features.to_vec(),
                    estimated_memory_units: evaluated.estimated_memory_units,
                    memory_update: evaluated.memory_update,
                    new_memory: None,
                },
            })
        } else {
            let memory = self.create_memory()?;
            let state = StreamState {
                memory,
                profile: FeatureProfile::new(self.config.dimensions),
                generation: 0,
            };
            let evaluated = evaluate_stream(&state, &pattern, features, at, &self.config)?;
            Ok(Evaluation {
                novelty_short: Some(1.0 - evaluated.memory_observation.short_familiarity),
                novelty_long: Some(1.0 - evaluated.memory_observation.long_familiarity),
                familiarity_short: Some(evaluated.memory_observation.short_familiarity),
                familiarity_long: Some(evaluated.memory_observation.long_familiarity),
                recency: evaluated.memory_observation.recency,
                pattern_id: Some(evaluated.memory_observation.pattern_id),
                readiness: crate::reaction::Readiness::Evaluated,
                diagnostics: FlyDiagnostics {
                    input_strength: input_strength(features, self.config.input_strength_scale),
                    profile_deviation: evaluated.profile_deviation,
                },
                update: FlyUpdate {
                    stream_id: stream_id.to_owned(),
                    expected_generation: None,
                    model_token: self.model_token,
                    configuration_fingerprint: self.configuration_fingerprint(),
                    input_dimensions: self.config.dimensions,
                    features: features.to_vec(),
                    estimated_memory_units: evaluated.estimated_memory_units,
                    memory_update: evaluated.memory_update,
                    new_memory: Some(state.memory),
                },
            })
        }
    }

    /// Evaluates a vector without changing engine state.
    ///
    /// This name is retained as a direct API spelling for callers migrating
    /// from the earlier observation API. The returned update must still be
    /// passed to [`Self::commit`] when learning is wanted.
    pub fn observe(
        &self,
        stream_id: &str,
        features: &[f32],
        at: Timestamp,
    ) -> Result<Evaluation<FlyDiagnostics, FlyUpdate<M::Update, M>>, ObserveError> {
        self.evaluate(stream_id, features, at)
    }

    /// Commits an evaluation update with a learning weight in `0.0..=1.0`.
    ///
    /// An update evaluated from an older generation is rejected. This prevents
    /// a delayed result from overwriting a newer stream state.
    pub fn commit(
        &mut self,
        update: FlyUpdate<M::Update, M>,
        weight: f32,
    ) -> Result<(), ModelError> {
        validate_learning_weight(weight)?;
        let FlyUpdate {
            stream_id,
            expected_generation,
            model_token,
            configuration_fingerprint,
            input_dimensions,
            features,
            estimated_memory_units,
            memory_update,
            new_memory,
        } = update;
        if model_token != self.model_token
            || configuration_fingerprint != self.configuration_fingerprint()
            || input_dimensions != self.config.dimensions
            || features.len() != self.config.dimensions
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
        let projected_existing = self.streams.get(&stream_id).map(|state| {
            self.total_allocation_units()
                .saturating_sub(self.stream_allocation_units(&stream_id, state))
                .saturating_add(self.stream_base_allocation_units(&stream_id))
                .saturating_add(estimated_memory_units)
        });
        if projected_existing.is_some_and(|units| units > self.config.max_memory_elements) {
            return Err(ModelError::ResourceLimit(
                "max_memory_elements was reached".to_owned(),
            ));
        }
        if let Some(state) = self.streams.get_mut(&stream_id) {
            if weight > 0.0 && !state.profile.can_update(weight) {
                return Err(ModelError::ProfileSampleCountOverflow);
            }
            if weight > 0.0 && !state.profile.update_weighted(&features, weight) {
                return Err(ModelError::ProfileSampleCountOverflow);
            }
            // Profile validation and update happen before the infallible
            // memory commit, so a profile overflow cannot leave the two
            // learned components out of sync.
            state.memory.commit_with_weight(memory_update, weight);
            state.generation = next_generation;
        } else {
            if self.streams.len() >= self.config.max_streams {
                return Err(ModelError::ResourceLimit(
                    "max_streams was reached".to_owned(),
                ));
            }
            let Some(mut memory) = new_memory else {
                return Err(ModelError::StaleUpdate);
            };
            let projected = self
                .total_allocation_units()
                .saturating_add(self.stream_base_allocation_units_for_id(&stream_id))
                .saturating_add(estimated_memory_units);
            if projected > self.config.max_memory_elements {
                return Err(ModelError::ResourceLimit(
                    "max_memory_elements was reached".to_owned(),
                ));
            }
            let mut profile = FeatureProfile::new(self.config.dimensions);
            if weight > 0.0 && !profile.update_weighted(&features, weight) {
                return Err(ModelError::ProfileSampleCountOverflow);
            }
            memory.commit_with_weight(memory_update, weight);
            self.streams.insert(
                stream_id,
                StreamState {
                    memory,
                    profile,
                    generation: next_generation,
                },
            );
        }
        Ok(())
    }

    /// Removes a stream and returns whether it existed.
    pub fn forget(&mut self, stream_id: &str) -> bool {
        self.streams.remove(stream_id).is_some()
    }

    /// Returns a stream's statistics, or `None` if it has not been committed.
    pub fn stats(&self, stream_id: &str) -> Option<StreamStats> {
        self.streams
            .get(stream_id)
            .map(|state| state.memory.stats().into())
    }

    fn validate_features(&self, features: &[f32]) -> Result<(), ObserveError> {
        if features.is_empty() {
            return Err(EncodeError::EmptyFeatures.into());
        }
        if features.len() != self.config.dimensions {
            return Err(EncodeError::DimensionMismatch {
                expected: self.config.dimensions,
                actual: features.len(),
            }
            .into());
        }
        if let Some(index) = features.iter().position(|value| !value.is_finite()) {
            return Err(EncodeError::NonFiniteFeature { index }.into());
        }
        Ok(())
    }

    fn create_memory(&self) -> Result<M, MemoryError> {
        let memory = (self.memory_factory)()?;
        let actual = memory.fingerprint();
        if actual != self.memory_fingerprint {
            return Err(MemoryError::FactoryFingerprintMismatch {
                expected: self.memory_fingerprint,
                actual,
            });
        }
        if !memory.is_empty() {
            return Err(MemoryError::FactoryReturnedNonEmpty);
        }
        Ok(memory)
    }

    fn configuration_fingerprint(&self) -> u64 {
        stable_hash([
            self.encoder.fingerprint(),
            self.memory_fingerprint,
            self.identity_fingerprint(),
            u64::from(self.config.input_strength_scale.to_bits()),
            u64::from(self.config.temporal_shift_scale.to_bits()),
            u64::from(self.config.variance_floor.to_bits()),
            self.config.max_streams as u64,
            self.config.max_memory_elements as u64,
        ])
    }

    fn stream_base_allocation_units(&self, stream_id: &str) -> usize {
        self.stream_base_allocation_units_for_id(stream_id)
    }

    fn stream_base_allocation_units_for_id(&self, stream_id: &str) -> usize {
        1_usize
            .saturating_add(stream_id.len())
            .saturating_add(self.config.dimensions.saturating_mul(2))
    }

    fn stream_allocation_units(&self, stream_id: &str, state: &StreamState<M>) -> usize {
        self.stream_base_allocation_units(stream_id)
            .saturating_add(state.memory.allocation_units())
    }

    fn total_allocation_units(&self) -> usize {
        self.streams
            .iter()
            .map(|(stream_id, state)| self.stream_allocation_units(stream_id, state))
            .fold(0_usize, usize::saturating_add)
    }

    fn recorded_memory_fingerprint(&self, format_version: u16) -> u64 {
        if format_version <= INTERMEDIATE_FORMAT_VERSION {
            self.legacy_memory_fingerprint
        } else if format_version == crate::persistence::SEQUENCED_FORMAT_VERSION {
            self.version_three_memory_fingerprint
        } else if format_version == PREVIOUS_FORMAT_VERSION {
            self.previous_memory_fingerprint
        } else {
            self.memory_fingerprint
        }
    }

    fn identity(&self) -> ModelIdentity {
        ModelIdentity {
            model_version: self.config.model_version,
            feature_schema_version: self.config.feature_schema_version,
        }
    }

    fn identity_fingerprint(&self) -> u64 {
        stable_hash([
            u64::from(self.config.model_version),
            u64::from(self.config.feature_schema_version),
        ])
    }
}

impl<E, M> Model for Habituation<E, M>
where
    E: Encoder,
    M: Memory,
{
    type Diagnostics = FlyDiagnostics;
    type Update = FlyUpdate<M::Update, M>;

    fn evaluate(
        &self,
        stream_id: &str,
        features: &[f32],
        at: Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        Habituation::evaluate(self, stream_id, features, at)
    }

    fn commit(&mut self, update: Self::Update, weight: f32) -> Result<(), ModelError> {
        Habituation::commit(self, update, weight)
    }

    fn identity(&self) -> ModelIdentity {
        Habituation::identity(self)
    }

    fn fingerprint(&self) -> u64 {
        self.configuration_fingerprint()
    }
}

static NEXT_MODEL_TOKEN: AtomicU64 = AtomicU64::new(1);

fn next_model_token() -> u64 {
    NEXT_MODEL_TOKEN.fetch_add(1, Ordering::Relaxed)
}

impl<E, M> Habituation<E, M>
where
    E: Encoder,
    M: PersistentMemory,
{
    /// Saves the engine in the current versioned habitua binary format.
    pub fn save<W: Write>(&self, writer: &mut W) -> Result<(), PersistenceError> {
        self.save_dyn(writer)
    }

    fn save_dyn(&self, writer: &mut dyn Write) -> Result<(), PersistenceError> {
        self.validate_save_budget()?;
        write_header(writer)?;
        write_config(writer, &self.config)?;
        write_u64(writer, self.encoder.fingerprint())?;
        write_u64(writer, self.memory_fingerprint)?;
        write_u64(writer, self.identity_fingerprint())?;
        write_u64(writer, self.configuration_fingerprint())?;
        write_u64(writer, self.streams.len() as u64)?;
        for (stream_id, state) in &self.streams {
            write_string(writer, stream_id)?;
            state.profile.save(writer)?;
            state
                .memory
                .save_state_for_format(writer, CURRENT_FORMAT_VERSION)?;
        }
        Ok(())
    }

    /// Loads an engine with caller-supplied encoder and memory components.
    pub fn load_with<R: Read, F>(
        reader: &mut R,
        encoder: E,
        memory_factory: F,
    ) -> Result<Self, PersistenceError>
    where
        F: Fn() -> Result<M, MemoryError> + Send + Sync + 'static,
    {
        Self::load_with_expected_identity(reader, encoder, memory_factory, None)
    }

    /// Loads an engine and checks the caller's model and feature-schema
    /// versions against the saved identity.
    pub fn load_with_identity<R: Read, F>(
        reader: &mut R,
        encoder: E,
        memory_factory: F,
        expected_identity: ModelIdentity,
    ) -> Result<Self, PersistenceError>
    where
        F: Fn() -> Result<M, MemoryError> + Send + Sync + 'static,
    {
        Self::load_with_expected_identity(reader, encoder, memory_factory, Some(expected_identity))
    }

    fn load_with_expected_identity<R: Read, F>(
        reader: &mut R,
        encoder: E,
        memory_factory: F,
        expected_identity: Option<ModelIdentity>,
    ) -> Result<Self, PersistenceError>
    where
        F: Fn() -> Result<M, MemoryError> + Send + Sync + 'static,
    {
        let version = read_header(reader)?;
        let config = read_config(reader, version)?;
        if version == LEGACY_FORMAT_VERSION {
            return Err(PersistenceError::LegacyComponentIdentityUnavailable);
        }
        let saved_encoder = read_u64(reader)?;
        let saved_memory = read_u64(reader)?;
        let saved_identity = (version >= PREVIOUS_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        let saved_configuration = (version >= CURRENT_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        if saved_encoder != encoder.fingerprint() {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "encoder",
            });
        }
        if expected_identity.is_some_and(|identity| {
            identity.model_version != config.model_version
                || identity.feature_schema_version != config.feature_schema_version
        }) {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "model identity",
            });
        }
        let allow_legacy_policy =
            version <= PREVIOUS_FORMAT_VERSION || config.long_learning_policy.is_legacy();
        let engine =
            match Self::new_with_validation(encoder, memory_factory, config, allow_legacy_policy) {
                Ok(engine) => engine,
                Err(HabituationError::Config(ConfigError::MemoryConfigurationMismatch)) => {
                    return Err(PersistenceError::ComponentFingerprintMismatch {
                        component: "memory",
                    });
                }
                Err(error) => return Err(error.into()),
            };
        if saved_memory != engine.recorded_memory_fingerprint(version) {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "memory",
            });
        }
        if saved_identity.is_some_and(|identity| identity != engine.identity_fingerprint()) {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "model identity",
            });
        }
        if saved_configuration
            .is_some_and(|fingerprint| fingerprint != engine.configuration_fingerprint())
        {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "model configuration",
            });
        }
        Self::load_body(reader, engine, version)
    }

    fn reserve_state_budget(&self, load_budget: &mut LoadBudget) -> std::io::Result<()> {
        if self.streams.len() > self.config.max_streams {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream count exceeds max_streams".to_owned(),
            ));
        }
        if self.total_allocation_units() > self.config.max_memory_elements {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "engine state exceeds max_memory_elements".to_owned(),
            ));
        }
        load_budget.reserve(self.streams.len())?;
        for (stream_id, state) in &self.streams {
            load_budget.reserve(stream_id.len())?;
            state.profile.reserve_load_budget(load_budget)?;
            state
                .memory
                .reserve_load_budget(CURRENT_FORMAT_VERSION, load_budget)?;
        }
        Ok(())
    }

    fn validate_save_budget(&self) -> Result<(), PersistenceError> {
        let mut load_budget = LoadBudget::new();
        self.reserve_state_budget(&mut load_budget)
            .map_err(PersistenceError::from)
    }

    fn load_body(
        reader: &mut dyn Read,
        engine: Self,
        format_version: u16,
    ) -> Result<Self, PersistenceError> {
        let mut load_budget = LoadBudget::new();
        let streams = Self::load_streams(reader, &engine, format_version, &mut load_budget)?;
        let mut loaded = engine;
        loaded.streams = streams;
        Ok(loaded)
    }

    fn load_streams(
        reader: &mut dyn Read,
        engine: &Self,
        format_version: u16,
        load_budget: &mut LoadBudget,
    ) -> Result<BTreeMap<String, StreamState<M>>, PersistenceError> {
        let stream_count = read_count(reader)?;
        if stream_count > engine.config.max_streams {
            return Err(PersistenceError::InvalidData(
                "stored stream count exceeds max_streams".to_owned(),
            ));
        }
        load_budget.reserve(stream_count)?;
        let profile_elements = engine.config.dimensions.checked_mul(2).ok_or_else(|| {
            PersistenceError::InvalidData(
                "feature profile dimensions overflowed during load".to_owned(),
            )
        })?;
        let total_profile_elements =
            stream_count.checked_mul(profile_elements).ok_or_else(|| {
                PersistenceError::InvalidData(
                    "feature profile element count overflowed during load".to_owned(),
                )
            })?;
        if total_profile_elements > load_budget.remaining() {
            return Err(PersistenceError::InvalidData(
                "serialized feature profiles exceed the load allocation limit".to_owned(),
            ));
        }
        let mut streams: BTreeMap<String, StreamState<M>> = BTreeMap::new();
        for _ in 0..stream_count {
            let stream_id = read_string(reader, load_budget)?;
            if streams.contains_key(&stream_id) {
                return Err(PersistenceError::InvalidData(
                    "duplicate stream id in state".to_owned(),
                ));
            }
            let profile = FeatureProfile::load(
                reader,
                engine.config.dimensions,
                load_budget,
                format_version,
            )?;
            let mut memory = engine.create_memory()?;
            if memory.fingerprint_for_format(format_version)
                != engine.recorded_memory_fingerprint(format_version)
            {
                return Err(PersistenceError::ComponentFingerprintMismatch {
                    component: "memory",
                });
            }
            memory.load_state_for_format(reader, format_version, load_budget)?;
            if memory.last_observed().is_none() && profile.is_empty() {
                return Err(PersistenceError::InvalidData(
                    "stored stream has no committed observation".to_owned(),
                ));
            }
            let stream_elements = engine
                .stream_base_allocation_units(&stream_id)
                .saturating_add(memory.allocation_units());
            let current_elements = streams
                .iter()
                .map(|(id, state)| engine.stream_allocation_units(id, state))
                .fold(stream_elements, usize::saturating_add);
            if current_elements > engine.config.max_memory_elements {
                return Err(PersistenceError::InvalidData(
                    "stored state exceeds max_memory_elements".to_owned(),
                ));
            }
            streams.insert(
                stream_id,
                StreamState {
                    memory,
                    profile,
                    generation: 0,
                },
            );
        }
        let mut trailing_byte = [0_u8; 1];
        if reader.read(&mut trailing_byte)? != 0 {
            return Err(PersistenceError::InvalidData(
                "trailing bytes after habitua state".to_owned(),
            ));
        }
        Ok(streams)
    }
}

impl Habituation<FlyHashEncoder, DecayingMemory> {
    /// Creates the built-in Fly model from explicit configuration.
    pub fn with_config(config: Config) -> Result<Self, EngineConfigError> {
        config.validate()?;
        let encoder = FlyHashEncoder::from_config(&config)?;
        let memory = DecayingMemory::from_config(&config)?;
        let memory_factory: Box<dyn Fn() -> Result<DecayingMemory, MemoryError> + Send + Sync> =
            Box::new(move || Ok(memory.empty_like()));
        Self::new(encoder, memory_factory, config).map_err(|error| match error {
            HabituationError::Config(error) => EngineConfigError::Config(error),
            HabituationError::MemoryFactory(error) => EngineConfigError::Factory(error),
        })
    }

    /// Creates the built-in Fly model with the documented default parameters.
    pub fn fly_default() -> Self {
        Self::with_config(Config::default()).expect("built-in default configuration is valid")
    }

    /// Loads the built-in model and reconstructs its deterministic encoder.
    pub fn load<R: Read>(reader: &mut R) -> Result<Self, PersistenceError> {
        Self::load_dyn(reader)
    }

    fn load_dyn(reader: &mut dyn Read) -> Result<Self, PersistenceError> {
        let version = read_header(reader)?;
        let config = read_config(reader, version)?;
        let saved_encoder = (version >= INTERMEDIATE_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        let saved_memory = (version >= INTERMEDIATE_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        let saved_identity = (version >= PREVIOUS_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        let saved_configuration = (version >= CURRENT_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        let encoder = FlyHashEncoder::from_config(&config)
            .map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
        let memory = DecayingMemory::new_for_format(&config)
            .map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
        if saved_encoder.is_some_and(|fingerprint| fingerprint != encoder.fingerprint()) {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "encoder",
            });
        }
        let memory_factory: Box<dyn Fn() -> Result<DecayingMemory, MemoryError> + Send + Sync> =
            Box::new(move || Ok(memory.empty_like()));
        let allow_legacy_policy =
            version <= PREVIOUS_FORMAT_VERSION || config.long_learning_policy.is_legacy();
        let engine =
            Self::new_with_validation(encoder, memory_factory, config, allow_legacy_policy)
                .map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
        if saved_memory
            .is_some_and(|fingerprint| fingerprint != engine.recorded_memory_fingerprint(version))
        {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "memory",
            });
        }
        if saved_identity.is_some_and(|identity| identity != engine.identity_fingerprint()) {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "model identity",
            });
        }
        if saved_configuration
            .is_some_and(|fingerprint| fingerprint != engine.configuration_fingerprint())
        {
            return Err(PersistenceError::ComponentFingerprintMismatch {
                component: "model configuration",
            });
        }
        Self::load_body(reader, engine, version)
    }
}

impl<E, M> PersistentModel for Habituation<E, M>
where
    E: Encoder + 'static,
    M: PersistentMemory + 'static,
{
    fn state_format_version(&self) -> u16 {
        CURRENT_FORMAT_VERSION
    }

    fn save_state(&self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.save_dyn(writer).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })
    }

    fn load_state(&mut self, reader: &mut dyn Read) -> std::io::Result<()> {
        let mut load_budget = LoadBudget::new();
        self.load_state_with_budget(reader, &mut load_budget)
    }

    fn reserve_load_budget(&self, load_budget: &mut LoadBudget) -> std::io::Result<()> {
        self.reserve_state_budget(load_budget)
    }

    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        load_budget: &mut LoadBudget,
    ) -> std::io::Result<()> {
        let version = crate::persistence::read_header(reader).map_err(persistence_to_io)?;
        let config = crate::persistence::read_config(reader, version).map_err(persistence_to_io)?;
        if version == LEGACY_FORMAT_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "legacy generic model state has no component identities",
            ));
        }
        let saved_encoder = read_u64(reader)?;
        let saved_memory = read_u64(reader)?;
        let saved_identity = (version >= PREVIOUS_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        let saved_configuration = (version >= CURRENT_FORMAT_VERSION)
            .then(|| read_u64(reader))
            .transpose()?;
        if config != self.config || saved_encoder != self.encoder.fingerprint() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "model configuration or encoder fingerprint mismatch",
            ));
        }
        if saved_memory != self.recorded_memory_fingerprint(version)
            || saved_identity.is_some_and(|identity| identity != self.identity_fingerprint())
            || saved_configuration
                .is_some_and(|fingerprint| fingerprint != self.configuration_fingerprint())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "model component or configuration fingerprint mismatch",
            ));
        }
        let streams =
            Self::load_streams(reader, self, version, load_budget).map_err(persistence_to_io)?;
        self.streams = streams;
        self.model_token = next_model_token();
        Ok(())
    }
}

fn persistence_to_io(error: PersistenceError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
}

fn evaluate_stream<M: Memory>(
    state: &StreamState<M>,
    pattern: &Pattern,
    features: &[f32],
    at: Timestamp,
    config: &Config,
) -> Result<EvaluatedMemory<M::Update>, ObserveError> {
    if let Some(previous) = state.memory.last_observed()
        && at < previous
    {
        return Err(ModelError::Memory(MemoryError::TimestampOutOfOrder {
            previous,
            current: at,
        }));
    }
    if let Some(unit) = pattern
        .units()
        .iter()
        .copied()
        .find(|unit| *unit >= config.encoder_units)
    {
        return Err(ModelError::Memory(MemoryError::UnitOutOfRange {
            unit,
            encoder_units: config.encoder_units,
        }));
    }
    let profile_deviation =
        state
            .profile
            .deviation(features, config.temporal_shift_scale, config.variance_floor);
    let expected_recency = state
        .memory
        .last_seen(pattern)
        .map(|last_seen| {
            at.checked_duration_since(last_seen)
                .ok_or(ModelError::InvalidMemoryObservation(
                    MemoryObservationError::PatternTimestampInFuture {
                        current: at,
                        last_seen,
                    },
                ))
        })
        .transpose()?;
    let transaction = state.memory.observe(pattern, at)?;
    validate_memory_observation(transaction.observation(), pattern, expected_recency)?;
    let (memory_observation, memory_update) = transaction.into_parts();
    Ok(EvaluatedMemory {
        memory_observation,
        memory_update,
        profile_deviation,
        estimated_memory_units: state.memory.projected_allocation_units(pattern),
    })
}

struct EvaluatedMemory<U> {
    memory_observation: MemoryObservation,
    memory_update: U,
    profile_deviation: f32,
    estimated_memory_units: usize,
}

fn input_strength(features: &[f32], scale: f32) -> f32 {
    let mean_square = features
        .iter()
        .map(|feature| {
            let value = f64::from(*feature);
            value * value
        })
        .sum::<f64>()
        / features.len() as f64;
    let root_mean_square = mean_square.sqrt();
    (1.0 - (-root_mean_square / f64::from(scale)).exp()) as f32
}

struct StreamState<M> {
    memory: M,
    profile: FeatureProfile,
    generation: u64,
}

/// Errors found while constructing a generic habituation engine.
#[derive(Debug)]
pub enum HabituationError {
    /// The shared configuration or its relationship to the encoder is invalid.
    Config(ConfigError),
    /// The first memory could not be created or validated by the factory.
    MemoryFactory(MemoryError),
}

impl fmt::Display for HabituationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::MemoryFactory(error) => write!(formatter, "memory factory error: {error}"),
        }
    }
}

impl std::error::Error for HabituationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::MemoryFactory(error) => Some(error),
        }
    }
}

impl From<ConfigError> for HabituationError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<MemoryError> for HabituationError {
    fn from(error: MemoryError) -> Self {
        Self::MemoryFactory(error)
    }
}

/// Errors from the built-in engine constructor.
#[derive(Debug)]
pub enum EngineConfigError {
    /// The shared configuration is invalid.
    Config(ConfigError),
    /// The projection encoder cannot be constructed.
    Encoder(EncoderConfigError),
    /// The decaying memory cannot be constructed.
    Memory(MemoryConfigError),
    /// A per-stream memory factory failed during construction.
    Factory(MemoryError),
}

impl fmt::Display for EngineConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Encoder(error) => error.fmt(formatter),
            Self::Memory(error) => error.fmt(formatter),
            Self::Factory(error) => write!(formatter, "memory factory error: {error}"),
        }
    }
}

impl std::error::Error for EngineConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Encoder(error) => Some(error),
            Self::Memory(error) => Some(error),
            Self::Factory(error) => Some(error),
        }
    }
}

impl From<ConfigError> for EngineConfigError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<EncoderConfigError> for EngineConfigError {
    fn from(error: EncoderConfigError) -> Self {
        Self::Encoder(error)
    }
}

impl From<MemoryConfigError> for EngineConfigError {
    fn from(error: MemoryConfigError) -> Self {
        Self::Memory(error)
    }
}

impl From<HabituationError> for PersistenceError {
    fn from(error: HabituationError) -> Self {
        match error {
            HabituationError::Config(error) => Self::InvalidData(error.to_string()),
            HabituationError::MemoryFactory(error) => Self::Memory(error),
        }
    }
}

fn validate_memory_observation(
    observation: &MemoryObservation,
    pattern: &Pattern,
    expected_recency: Option<Duration>,
) -> Result<(), ObserveError> {
    for (field, value) in [
        ("short_familiarity", observation.short_familiarity),
        ("long_familiarity", observation.long_familiarity),
        ("familiarity", observation.familiarity),
        ("novelty", observation.novelty),
    ] {
        if !value.is_finite() {
            return Err(MemoryObservationError::NonFiniteValue { field }.into());
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(MemoryObservationError::ValueOutOfRange { field }.into());
        }
    }
    if (1.0 - observation.familiarity - observation.novelty).abs() > 1.0e-5 {
        return Err(MemoryObservationError::NoveltyFamiliarityMismatch.into());
    }
    let expected_pattern_id: PatternId = pattern.id();
    if observation.pattern_id != expected_pattern_id {
        return Err(MemoryObservationError::PatternIdMismatch {
            expected: expected_pattern_id,
            actual: observation.pattern_id,
        }
        .into());
    }
    if observation.recency != expected_recency {
        return Err(MemoryObservationError::RecencyMismatch {
            expected: expected_recency,
            actual: observation.recency,
        }
        .into());
    }
    Ok(())
}
