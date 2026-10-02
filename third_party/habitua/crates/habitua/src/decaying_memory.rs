use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

use crate::config::{Config, LongLearningPolicy};
use crate::memory::{
    Memory, MemoryError, MemoryObservation, MemoryStats, MemoryTransaction, PersistentMemory,
};
use crate::pattern::{Pattern, stable_hash};
use crate::persistence::{
    CURRENT_FORMAT_VERSION, INTERMEDIATE_FORMAT_VERSION, LoadBudget, SEQUENCED_FORMAT_VERSION,
    read_count, read_f32, read_timestamp, read_u8, read_u32, read_u64, read_usize, write_f32,
    write_timestamp, write_u8, write_u32, write_u64,
};
use crate::timestamp::Timestamp;

const MEMORY_ALGORITHM_VERSION: u64 = 3;
const VERSION_THREE_MEMORY_ALGORITHM: u64 = 2;
const LEGACY_MEMORY_ALGORITHM_VERSION: u64 = 1;

/// A memory with independent short- and long-term exponentially decaying
/// traces.
///
/// For a unit weight `w` last updated at `t₀`, the value used at `t` is
/// `w(t) = w(t₀) exp(-(t - t₀) / τ)`. Before reinforcement, the memory
/// calculates the familiarity of the observed pattern as the mean of its
/// active-unit weights:
///
/// `f_short = mean(w_short(t))`
///
/// `f_long = mean(w_long(t))`
///
/// `familiarity = (α f_short + β f_long) / (α + β)` and
/// `novelty = 1 - familiarity`. The result is calculated before the current
/// pattern is reinforced. The short-term update is
/// `min(1, w_short(t) + r_short × learning_weight)`. The long-term update
/// is `min(1, w_long(t) + r_long × learning_weight)` when
/// [`LongLearningPolicy`] permits it; otherwise the decayed long-term weight
/// is unchanged. A zero learning weight still records the arrival and
/// exact-pattern recency, but does not reinforce unit weights or the feature
/// profile.
///
/// Exact-pattern records are retained separately for `recency` and bounded by
/// `Config::max_patterns` using least-recently-seen eviction. An observation
/// sequence breaks timestamp ties and is persisted with the memory state.
/// Unit weights remain in the configured `0..Config::encoder_units` range.
#[derive(Clone, Debug)]
pub struct DecayingMemory {
    encoder_units: usize,
    short_time_constant: Duration,
    long_time_constant: Duration,
    short_reinforcement: f32,
    long_reinforcement: f32,
    short_familiarity_weight: f32,
    long_familiarity_weight: f32,
    max_patterns: usize,
    long_learning_policy: LongLearningPolicy,
    units: BTreeMap<usize, UnitWeight>,
    patterns: BTreeMap<Vec<usize>, PatternRecord>,
    last_observed: Option<Timestamp>,
    observation_sequence: u64,
}

impl DecayingMemory {
    /// Creates an empty memory using all memory parameters in `config`.
    pub fn new(config: &Config) -> Result<Self, MemoryConfigError> {
        Self::new_with_policy(config, false)
    }

    /// Creates an empty memory while allowing the legacy persistence policy.
    ///
    /// This is used only when decoding formats that predate the explicit
    /// long-term learning policy. New callers must use [`Self::new`], which
    /// rejects the compatibility-only every-commit policy.
    pub(crate) fn new_for_format(config: &Config) -> Result<Self, MemoryConfigError> {
        Self::new_with_policy(config, true)
    }

    fn new_with_policy(
        config: &Config,
        allow_legacy_policy: bool,
    ) -> Result<Self, MemoryConfigError> {
        if config.encoder_units == 0 {
            return Err(MemoryConfigError::InvalidParameter {
                name: "encoder_units",
            });
        }
        if config.short_time_constant.is_zero() {
            return Err(MemoryConfigError::ZeroShortTimeConstant);
        }
        if config.long_time_constant.is_zero() {
            return Err(MemoryConfigError::ZeroLongTimeConstant);
        }
        if config.long_time_constant <= config.short_time_constant {
            return Err(MemoryConfigError::LongTimeConstantNotLonger);
        }
        if !config.short_reinforcement.is_finite()
            || !(0.0..=1.0).contains(&config.short_reinforcement)
        {
            return Err(MemoryConfigError::InvalidParameter {
                name: "short_reinforcement",
            });
        }
        if !config.long_reinforcement.is_finite()
            || !(0.0..=1.0).contains(&config.long_reinforcement)
        {
            return Err(MemoryConfigError::InvalidParameter {
                name: "long_reinforcement",
            });
        }
        let familiarity_weight_sum =
            config.short_familiarity_weight + config.long_familiarity_weight;
        if !config.short_familiarity_weight.is_finite()
            || config.short_familiarity_weight < 0.0
            || !config.long_familiarity_weight.is_finite()
            || config.long_familiarity_weight < 0.0
            || !familiarity_weight_sum.is_finite()
            || familiarity_weight_sum <= 0.0
        {
            return Err(MemoryConfigError::InvalidParameter {
                name: "familiarity_weights",
            });
        }
        if config.max_patterns == 0 {
            return Err(MemoryConfigError::InvalidParameter {
                name: "max_patterns",
            });
        }
        match &config.long_learning_policy {
            LongLearningPolicy::Revisit { interval } if interval.is_zero() => {
                return Err(MemoryConfigError::ZeroLongRevisitInterval);
            }
            LongLearningPolicy::RepeatedNormal {
                consecutive_observations,
            } if *consecutive_observations < 2 => {
                return Err(MemoryConfigError::InvalidParameter {
                    name: "consecutive_observations",
                });
            }
            LongLearningPolicy::LegacyEveryCommit if !allow_legacy_policy => {
                return Err(MemoryConfigError::LegacyLearningPolicy);
            }
            _ => {}
        }
        Ok(Self {
            encoder_units: config.encoder_units,
            short_time_constant: config.short_time_constant,
            long_time_constant: config.long_time_constant,
            short_reinforcement: config.short_reinforcement,
            long_reinforcement: config.long_reinforcement,
            short_familiarity_weight: config.short_familiarity_weight,
            long_familiarity_weight: config.long_familiarity_weight,
            max_patterns: config.max_patterns,
            long_learning_policy: config.long_learning_policy.clone(),
            units: BTreeMap::new(),
            patterns: BTreeMap::new(),
            last_observed: None,
            observation_sequence: 0,
        })
    }

    /// Creates an empty memory using the memory parameters in `config`.
    pub fn from_config(config: &Config) -> Result<Self, MemoryConfigError> {
        Self::new(config)
    }

    pub(crate) fn empty_like(&self) -> Self {
        Self {
            encoder_units: self.encoder_units,
            short_time_constant: self.short_time_constant,
            long_time_constant: self.long_time_constant,
            short_reinforcement: self.short_reinforcement,
            long_reinforcement: self.long_reinforcement,
            short_familiarity_weight: self.short_familiarity_weight,
            long_familiarity_weight: self.long_familiarity_weight,
            max_patterns: self.max_patterns,
            long_learning_policy: self.long_learning_policy.clone(),
            units: BTreeMap::new(),
            patterns: BTreeMap::new(),
            last_observed: None,
            observation_sequence: 0,
        }
    }

    fn current_weight(&self, unit: usize, at: Timestamp) -> (f32, f32) {
        self.units
            .get(&unit)
            .map(|weight| {
                (
                    decay(
                        weight.short,
                        weight.updated_at,
                        at,
                        self.short_time_constant,
                    ),
                    decay(weight.long, weight.updated_at, at, self.long_time_constant),
                )
            })
            .unwrap_or((0.0, 0.0))
    }

    fn aggregate_familiarity(&self, at: Option<Timestamp>) -> (f32, f32) {
        let Some(at) = at else {
            return (0.0, 0.0);
        };
        if self.units.is_empty() {
            return (0.0, 0.0);
        }
        let (short_sum, long_sum) = self
            .units
            .keys()
            .map(|unit| self.current_weight(*unit, at))
            .fold(
                (0.0_f32, 0.0_f32),
                |(short, long), (next_short, next_long)| (short + next_short, long + next_long),
            );
        let count = self.units.len() as f32;
        (short_sum / count, long_sum / count)
    }

    fn load_weight(
        &mut self,
        unit: usize,
        short: f32,
        long: f32,
        updated_at: Timestamp,
    ) -> io::Result<()> {
        self.validate_unit(unit).map_err(memory_error_to_io)?;
        if !short.is_finite() || !(0.0..=1.0).contains(&short) {
            return Err(invalid_data("short memory weight is outside 0..=1"));
        }
        if !long.is_finite() || !(0.0..=1.0).contains(&long) {
            return Err(invalid_data("long memory weight is outside 0..=1"));
        }
        if let Some(last_observed) = self.last_observed
            && updated_at > last_observed
        {
            return Err(invalid_data(
                "unit timestamp is newer than last observation",
            ));
        }
        if self
            .units
            .insert(
                unit,
                UnitWeight {
                    short,
                    long,
                    updated_at,
                },
            )
            .is_some()
        {
            return Err(invalid_data("duplicate unit in memory state"));
        }
        Ok(())
    }

    fn validate_unit(&self, unit: usize) -> Result<(), MemoryError> {
        if unit >= self.encoder_units {
            return Err(MemoryError::UnitOutOfRange {
                unit,
                encoder_units: self.encoder_units,
            });
        }
        Ok(())
    }

    fn validate_pattern(&self, pattern: &Pattern) -> Result<(), MemoryError> {
        for unit in pattern.units() {
            self.validate_unit(*unit)?;
        }
        Ok(())
    }

    fn oldest_pattern(&self) -> Option<Vec<usize>> {
        self.patterns
            .iter()
            .min_by(|(units_a, record_a), (units_b, record_b)| {
                record_a
                    .last_seen
                    .cmp(&record_b.last_seen)
                    .then_with(|| {
                        record_a
                            .last_seen_sequence
                            .cmp(&record_b.last_seen_sequence)
                    })
                    .then_with(|| units_a.cmp(units_b))
            })
            .map(|(units, _)| units.clone())
    }

    fn decode_state(
        reader: &mut dyn Read,
        prototype: &Self,
        format_version: u16,
        load_budget: &mut LoadBudget,
    ) -> io::Result<Self> {
        let mut state = prototype.empty_like();
        let has_last_observed = read_u8(reader)?;
        if has_last_observed > 1 {
            return Err(invalid_data("invalid last-observed flag"));
        }
        state.last_observed = (has_last_observed == 1)
            .then(|| read_timestamp(reader))
            .transpose()?;
        if format_version >= SEQUENCED_FORMAT_VERSION {
            state.observation_sequence = read_u64(reader)?;
        }

        let unit_count = read_count(reader)?;
        if unit_count > prototype.encoder_units {
            return Err(invalid_data(
                "stored unit count exceeds configured encoder_units",
            ));
        }
        load_budget.reserve(unit_count)?;
        for _ in 0..unit_count {
            let unit = read_usize(reader)?;
            let short = read_f32(reader)?;
            let long = read_f32(reader)?;
            let updated_at = read_timestamp(reader)?;
            state.load_weight(unit, short, long, updated_at)?;
        }

        let pattern_count = read_count(reader)?;
        if pattern_count > state.max_patterns {
            return Err(invalid_data(
                "stored pattern count exceeds configured max_patterns",
            ));
        }
        load_budget.reserve(pattern_count)?;
        for pattern_index in 0..pattern_count {
            let unit_count = read_count(reader)?;
            if unit_count > prototype.encoder_units {
                return Err(invalid_data(
                    "stored pattern unit count exceeds configured encoder_units",
                ));
            }
            load_budget.reserve(unit_count)?;
            let mut units = Vec::with_capacity(unit_count);
            for _ in 0..unit_count {
                units.push(read_usize(reader)?);
            }
            let pattern =
                Pattern::new(units).map_err(|_| invalid_data("invalid pattern in memory state"))?;
            for unit in pattern.units() {
                state.validate_unit(*unit).map_err(memory_error_to_io)?;
            }
            let last_seen = read_timestamp(reader)?;
            if let Some(last_observed) = state.last_observed
                && last_seen > last_observed
            {
                return Err(invalid_data(
                    "pattern timestamp is newer than last observation",
                ));
            }
            if state.patterns.contains_key(pattern.units()) {
                return Err(invalid_data("duplicate pattern in memory state"));
            }
            let last_seen_sequence = if format_version >= SEQUENCED_FORMAT_VERSION {
                read_u64(reader)?
            } else {
                u64::try_from(pattern_index + 1)
                    .map_err(|_| invalid_data("legacy observation sequence overflowed"))?
            };
            if format_version >= SEQUENCED_FORMAT_VERSION
                && last_seen_sequence > state.observation_sequence
            {
                return Err(invalid_data(
                    "pattern sequence is newer than observation sequence",
                ));
            }
            let consecutive_experiences = if format_version >= CURRENT_FORMAT_VERSION {
                read_u32(reader)?
            } else {
                u32::from(!pattern.units().is_empty())
            };
            load_budget.reserve(unit_count)?;
            state.patterns.insert(
                pattern.units().to_vec(),
                PatternRecord {
                    last_seen,
                    last_seen_sequence,
                    consecutive_experiences,
                },
            );
            if format_version < SEQUENCED_FORMAT_VERSION {
                state.observation_sequence = last_seen_sequence;
            }
        }
        if state.last_observed.is_none() && state.observation_sequence != 0 {
            return Err(invalid_data(
                "memory sequence exists without an observation timestamp",
            ));
        }
        if state.last_observed.is_none() && (!state.units.is_empty() || !state.patterns.is_empty())
        {
            return Err(invalid_data(
                "memory state has data without an observation timestamp",
            ));
        }
        Ok(state)
    }

    fn legacy_fingerprint(&self) -> u64 {
        stable_hash([
            LEGACY_MEMORY_ALGORITHM_VERSION,
            self.short_time_constant.as_secs(),
            u64::from(self.short_time_constant.subsec_nanos()),
            self.long_time_constant.as_secs(),
            u64::from(self.long_time_constant.subsec_nanos()),
            u64::from(self.short_reinforcement.to_bits()),
            u64::from(self.long_reinforcement.to_bits()),
            u64::from(self.short_familiarity_weight.to_bits()),
            u64::from(self.long_familiarity_weight.to_bits()),
            self.max_patterns as u64,
        ])
    }

    fn version_three_fingerprint(&self) -> u64 {
        stable_hash([
            VERSION_THREE_MEMORY_ALGORITHM,
            self.encoder_units as u64,
            self.short_time_constant.as_secs(),
            u64::from(self.short_time_constant.subsec_nanos()),
            self.long_time_constant.as_secs(),
            u64::from(self.long_time_constant.subsec_nanos()),
            u64::from(self.short_reinforcement.to_bits()),
            u64::from(self.long_reinforcement.to_bits()),
            u64::from(self.short_familiarity_weight.to_bits()),
            u64::from(self.long_familiarity_weight.to_bits()),
            self.max_patterns as u64,
        ])
    }

    fn previous_fingerprint(&self) -> u64 {
        let (requires_revisit, interval) = match &self.long_learning_policy {
            LongLearningPolicy::Revisit { interval } => (1_u64, *interval),
            LongLearningPolicy::LegacyEveryCommit => (0_u64, Duration::ZERO),
            LongLearningPolicy::RepeatedNormal {
                consecutive_observations,
            } => (
                1_u64,
                Duration::from_secs(u64::from(*consecutive_observations)),
            ),
        };
        stable_hash([
            MEMORY_ALGORITHM_VERSION,
            self.encoder_units as u64,
            self.short_time_constant.as_secs(),
            u64::from(self.short_time_constant.subsec_nanos()),
            self.long_time_constant.as_secs(),
            u64::from(self.long_time_constant.subsec_nanos()),
            u64::from(self.short_reinforcement.to_bits()),
            u64::from(self.long_reinforcement.to_bits()),
            u64::from(self.short_familiarity_weight.to_bits()),
            u64::from(self.long_familiarity_weight.to_bits()),
            self.max_patterns as u64,
            requires_revisit,
            interval.as_secs(),
            u64::from(interval.subsec_nanos()),
        ])
    }
}

impl Memory for DecayingMemory {
    type Update = Box<dyn FnOnce(&mut Self, f32)>;

    fn validate_config(&self, config: &Config) -> Result<(), crate::config::ConfigError> {
        let matches = self.encoder_units == config.encoder_units
            && self.short_time_constant == config.short_time_constant
            && self.long_time_constant == config.long_time_constant
            && self.short_reinforcement.to_bits() == config.short_reinforcement.to_bits()
            && self.long_reinforcement.to_bits() == config.long_reinforcement.to_bits()
            && self.short_familiarity_weight.to_bits() == config.short_familiarity_weight.to_bits()
            && self.long_familiarity_weight.to_bits() == config.long_familiarity_weight.to_bits()
            && self.max_patterns == config.max_patterns
            && self.long_learning_policy == config.long_learning_policy;
        if matches {
            Ok(())
        } else {
            Err(crate::config::ConfigError::MemoryConfigurationMismatch)
        }
    }

    fn observe(
        &self,
        pattern: &Pattern,
        at: Timestamp,
    ) -> Result<MemoryTransaction<Self::Update>, MemoryError> {
        if let Some(previous) = self.last_observed
            && at < previous
        {
            return Err(MemoryError::TimestampOutOfOrder {
                previous,
                current: at,
            });
        }
        self.validate_pattern(pattern)?;
        let observation_sequence = self
            .observation_sequence
            .checked_add(1)
            .ok_or(MemoryError::ObservationSequenceOverflow)?;

        let (short_sum, long_sum) = pattern
            .units()
            .iter()
            .map(|unit| self.current_weight(*unit, at))
            .fold(
                (0.0_f32, 0.0_f32),
                |(short, long), (next_short, next_long)| (short + next_short, long + next_long),
            );
        let active_count = pattern.units().len() as f32;
        let short_familiarity = clamp01(short_sum / active_count);
        let long_familiarity = clamp01(long_sum / active_count);
        let familiarity = clamp01(
            (self.short_familiarity_weight * short_familiarity
                + self.long_familiarity_weight * long_familiarity)
                / (self.short_familiarity_weight + self.long_familiarity_weight),
        );
        let novelty = clamp01(1.0 - familiarity);
        let recency = self
            .patterns
            .get(pattern.units())
            .and_then(|record| at.checked_duration_since(record.last_seen));
        let previous_consecutive_experiences = self
            .patterns
            .get(pattern.units())
            .filter(|record| record.last_seen_sequence == self.observation_sequence)
            .map_or(0, |record| record.consecutive_experiences);
        let long_should_reinforce = match &self.long_learning_policy {
            LongLearningPolicy::Revisit { interval } => {
                recency.is_some_and(|elapsed| elapsed >= *interval)
            }
            LongLearningPolicy::RepeatedNormal {
                consecutive_observations,
            } => previous_consecutive_experiences.saturating_add(1) >= *consecutive_observations,
            LongLearningPolicy::LegacyEveryCommit => true,
        };
        let evicted_pattern = (!self.patterns.contains_key(pattern.units())
            && self.patterns.len() >= self.max_patterns)
            .then(|| self.oldest_pattern())
            .flatten();
        let active_units = pattern.units().to_vec();
        let pattern_units = active_units.clone();
        let short_reinforcement = self.short_reinforcement;
        let long_reinforcement = self.long_reinforcement;
        let update = Box::new(move |memory: &mut DecayingMemory, learning_weight: f32| {
            if !learning_weight.is_finite() || !(0.0..=1.0).contains(&learning_weight) {
                return;
            }
            if learning_weight > 0.0 {
                for unit in &active_units {
                    let (short, long) = memory.current_weight(*unit, at);
                    let next_long = if long_should_reinforce {
                        clamp01(long + long_reinforcement * learning_weight)
                    } else {
                        long
                    };
                    memory.units.insert(
                        *unit,
                        UnitWeight {
                            short: clamp01(short + short_reinforcement * learning_weight),
                            long: next_long,
                            updated_at: at,
                        },
                    );
                }
            }
            if let Some(evicted_pattern) = evicted_pattern {
                memory.patterns.remove(&evicted_pattern);
            }
            let next_consecutive_experiences = if learning_weight > 0.0 {
                previous_consecutive_experiences.saturating_add(1)
            } else {
                0
            };
            memory.patterns.insert(
                pattern_units,
                PatternRecord {
                    last_seen: at,
                    last_seen_sequence: observation_sequence,
                    consecutive_experiences: next_consecutive_experiences,
                },
            );
            memory.last_observed = Some(at);
            memory.observation_sequence = observation_sequence;
        });

        Ok(MemoryTransaction::new(
            MemoryObservation {
                short_familiarity,
                long_familiarity,
                familiarity,
                novelty,
                recency,
                pattern_id: pattern.id(),
            },
            update,
        ))
    }

    fn commit(&mut self, update: Self::Update) {
        update(self, 1.0);
    }

    fn commit_with_weight(&mut self, update: Self::Update, weight: f32) {
        update(self, weight);
    }

    fn last_seen(&self, pattern: &Pattern) -> Option<Timestamp> {
        self.patterns
            .get(pattern.units())
            .map(|record| record.last_seen)
    }

    fn last_observed(&self) -> Option<Timestamp> {
        self.last_observed
    }

    fn fingerprint(&self) -> u64 {
        stable_hash([
            MEMORY_ALGORITHM_VERSION,
            self.encoder_units as u64,
            self.short_time_constant.as_secs(),
            u64::from(self.short_time_constant.subsec_nanos()),
            self.long_time_constant.as_secs(),
            u64::from(self.long_time_constant.subsec_nanos()),
            u64::from(self.short_reinforcement.to_bits()),
            u64::from(self.long_reinforcement.to_bits()),
            u64::from(self.short_familiarity_weight.to_bits()),
            u64::from(self.long_familiarity_weight.to_bits()),
            self.max_patterns as u64,
            long_policy_fingerprint(&self.long_learning_policy),
        ])
    }

    fn fingerprint_for_format(&self, format_version: u16) -> u64 {
        if format_version <= INTERMEDIATE_FORMAT_VERSION {
            self.legacy_fingerprint()
        } else if format_version == SEQUENCED_FORMAT_VERSION {
            self.version_three_fingerprint()
        } else if format_version == crate::persistence::PREVIOUS_FORMAT_VERSION {
            self.previous_fingerprint()
        } else {
            self.fingerprint()
        }
    }

    fn forget(&mut self) {
        self.units.clear();
        self.patterns.clear();
        self.last_observed = None;
        self.observation_sequence = 0;
    }

    fn stats(&self) -> MemoryStats {
        let (short_familiarity, long_familiarity) = self.aggregate_familiarity(self.last_observed);
        MemoryStats {
            pattern_count: self.patterns.len(),
            remembered_unit_count: self.units.len(),
            short_familiarity,
            long_familiarity,
            last_observed: self.last_observed,
        }
    }

    fn allocation_units(&self) -> usize {
        let pattern_units = self
            .patterns
            .keys()
            .map(Vec::len)
            .fold(0_usize, usize::saturating_add);
        self.units
            .len()
            .saturating_add(self.patterns.len())
            .saturating_add(pattern_units.saturating_mul(2))
    }

    fn projected_allocation_units(&self, pattern: &Pattern) -> usize {
        let mut projected = self.allocation_units();
        for unit in pattern.units() {
            if !self.units.contains_key(unit) {
                projected = projected.saturating_add(1);
            }
        }
        if !self.patterns.contains_key(pattern.units()) {
            projected = projected
                .saturating_add(pattern.units().len().saturating_mul(2))
                .saturating_add(1);
            if self.patterns.len() >= self.max_patterns
                && let Some(oldest) = self.oldest_pattern()
            {
                projected = projected.saturating_sub(oldest.len().saturating_mul(2) + 1);
            }
        }
        projected
    }
}

impl PersistentMemory for DecayingMemory {
    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()> {
        self.save_state_for_format(writer, CURRENT_FORMAT_VERSION)
    }

    fn save_state_for_format(&self, writer: &mut dyn Write, format_version: u16) -> io::Result<()> {
        write_u8(writer, u8::from(self.last_observed.is_some()))?;
        if let Some(last_observed) = self.last_observed {
            write_timestamp(writer, last_observed)?;
        }
        if format_version >= SEQUENCED_FORMAT_VERSION {
            write_u64(writer, self.observation_sequence)?;
        }

        write_u64(writer, self.units.len() as u64)?;
        for (unit, weight) in &self.units {
            write_u64(writer, *unit as u64)?;
            write_f32(writer, weight.short)?;
            write_f32(writer, weight.long)?;
            write_timestamp(writer, weight.updated_at)?;
        }

        write_u64(writer, self.patterns.len() as u64)?;
        for (units, record) in &self.patterns {
            write_u64(writer, units.len() as u64)?;
            for unit in units {
                write_u64(writer, *unit as u64)?;
            }
            write_timestamp(writer, record.last_seen)?;
            if format_version >= SEQUENCED_FORMAT_VERSION {
                write_u64(writer, record.last_seen_sequence)?;
            }
            if format_version >= CURRENT_FORMAT_VERSION {
                write_u32(writer, record.consecutive_experiences)?;
            }
        }
        Ok(())
    }

    fn load_state(&mut self, reader: &mut dyn Read) -> io::Result<()> {
        let mut load_budget = LoadBudget::new();
        self.load_state_for_format(reader, CURRENT_FORMAT_VERSION, &mut load_budget)
    }

    fn load_state_for_format(
        &mut self,
        reader: &mut dyn Read,
        format_version: u16,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()> {
        let state = Self::decode_state(reader, self, format_version, load_budget)?;
        *self = state;
        Ok(())
    }

    fn reserve_load_budget(
        &self,
        _format_version: u16,
        load_budget: &mut LoadBudget,
    ) -> io::Result<()> {
        load_budget.reserve(self.units.len())?;
        load_budget.reserve(self.patterns.len())?;
        for units in self.patterns.keys() {
            load_budget.reserve(units.len())?;
            load_budget.reserve(units.len())?;
        }
        Ok(())
    }
}

fn decay(weight: f32, updated_at: Timestamp, at: Timestamp, time_constant: Duration) -> f32 {
    let Some(elapsed) = at.checked_duration_since(updated_at) else {
        return 0.0;
    };
    let retention = (-(elapsed.as_secs_f64() / time_constant.as_secs_f64())).exp() as f32;
    clamp01(weight * retention)
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn memory_error_to_io(error: MemoryError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[derive(Clone, Copy, Debug)]
struct UnitWeight {
    short: f32,
    long: f32,
    updated_at: Timestamp,
}

#[derive(Clone, Copy, Debug)]
struct PatternRecord {
    last_seen: Timestamp,
    last_seen_sequence: u64,
    consecutive_experiences: u32,
}

fn long_policy_fingerprint(policy: &LongLearningPolicy) -> u64 {
    match policy {
        LongLearningPolicy::Revisit { interval } => {
            stable_hash([0, interval.as_secs(), u64::from(interval.subsec_nanos())])
        }
        LongLearningPolicy::RepeatedNormal {
            consecutive_observations,
        } => stable_hash([1, u64::from(*consecutive_observations)]),
        LongLearningPolicy::LegacyEveryCommit => stable_hash([2]),
    }
}

/// Errors found while constructing a [`DecayingMemory`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryConfigError {
    /// The short-term time constant is zero.
    ZeroShortTimeConstant,
    /// The long-term time constant is zero.
    ZeroLongTimeConstant,
    /// The long-term time constant must exceed the short-term one.
    LongTimeConstantNotLonger,
    /// The long-term revisit interval is zero.
    ZeroLongRevisitInterval,
    /// The every-commit policy is available only for old persistence files.
    LegacyLearningPolicy,
    /// A memory parameter is invalid.
    InvalidParameter {
        /// Name of the invalid memory parameter.
        name: &'static str,
    },
}

impl fmt::Display for MemoryConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::ZeroLongRevisitInterval => {
                write!(formatter, "long_revisit_interval must be non-zero")
            }
            Self::LegacyLearningPolicy => write!(
                formatter,
                "legacy every-commit learning policy is only valid for old state files"
            ),
            Self::InvalidParameter { name } => {
                write!(formatter, "memory parameter {name} is invalid")
            }
        }
    }
}

impl std::error::Error for MemoryConfigError {}
