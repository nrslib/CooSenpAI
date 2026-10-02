use std::fmt;
use std::io::{Read, Write};
use std::time::Duration;

use crate::config::{Config, ConfigError};
use crate::pattern::{Pattern, PatternId};
use crate::timestamp::Timestamp;

/// Records the familiarity result produced by a memory implementation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemoryObservation {
    /// Familiarity represented by the short-term trace, in `0..=1`.
    pub short_familiarity: f32,
    /// Familiarity represented by the long-term trace, in `0..=1`.
    pub long_familiarity: f32,
    /// The configured combination of the short- and long-term familiarity, in
    /// `0..=1`.
    pub familiarity: f32,
    /// The complement of `familiarity`, in `0..=1`.
    pub novelty: f32,
    /// Time since this exact pattern was last observed.
    pub recency: Option<Duration>,
    /// Stable fingerprint of the observed pattern; it is not collision-free.
    pub pattern_id: PatternId,
}

/// A memory result that can be committed after the core validates it.
///
/// Transactions are single-use values. Callers that use [`Memory`] directly
/// must commit them in evaluation order; an older transaction committed later
/// can overwrite newer state. [`crate::Habituation`] adds a generation check
/// at its public boundary and rejects that stale-update case.
pub struct MemoryTransaction<U> {
    observation: MemoryObservation,
    update: U,
}

impl<U> MemoryTransaction<U> {
    /// Creates a transaction from its public result and implementation update.
    pub fn new(observation: MemoryObservation, update: U) -> Self {
        Self {
            observation,
            update,
        }
    }

    /// Returns the result that the core must validate before committing.
    pub fn observation(&self) -> &MemoryObservation {
        &self.observation
    }

    /// Splits the transaction into its result and commit operation.
    pub fn into_parts(self) -> (MemoryObservation, U) {
        (self.observation, self.update)
    }
}

/// A snapshot of the state held by one stream's memory.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemoryStats {
    /// Number of distinct patterns retained by the memory.
    pub pattern_count: usize,
    /// Number of units with at least one memory trace.
    pub remembered_unit_count: usize,
    /// Aggregate short-term familiarity at the latest observation time.
    pub short_familiarity: f32,
    /// Aggregate long-term familiarity at the latest observation time.
    pub long_familiarity: f32,
    /// Latest timestamp handled by this memory.
    pub last_observed: Option<Timestamp>,
}

/// Stores and decays sparse patterns.
pub trait Memory {
    /// The implementation-specific update carried by a transaction.
    type Update;

    /// Validates configuration owned by this memory implementation.
    ///
    /// The default accepts any valid core configuration. Built-in memories
    /// override this to ensure that a generic constructor cannot pair a
    /// memory configured with different decay or unit parameters.
    fn validate_config(&self, _config: &Config) -> Result<(), ConfigError> {
        Ok(())
    }

    /// Evaluates a pattern and returns an update that has not been committed.
    ///
    /// This method must not mutate `self`. The core validates the returned
    /// [`MemoryObservation`] and commits the update only after validation
    /// succeeds. A failed evaluation must leave the memory unchanged.
    fn observe(
        &self,
        pattern: &Pattern,
        at: Timestamp,
    ) -> Result<MemoryTransaction<Self::Update>, MemoryError>;

    /// Commits an update returned by [`Self::observe`] at full strength.
    ///
    /// A direct [`Memory`] caller must not commit an older transaction after a
    /// newer one for the same memory. The generic engine enforces this with a
    /// stream generation check.
    fn commit(&mut self, update: Self::Update);

    /// Commits an update with exactly the requested learning weight.
    ///
    /// This is required rather than a default method so a generic model cannot
    /// silently turn a fractional weight into a full-strength update. A zero
    /// weight is a defined two-part operation: it must not reinforce learned
    /// state, and it must record the arrival metadata carried by the update
    /// (for example, the latest timestamp or exact-pattern recency). Values
    /// between zero and one must contribute proportionally to learned state.
    fn commit_with_weight(&mut self, update: Self::Update, weight: f32);

    /// Returns the last timestamp for the exact pattern, if retained.
    fn last_seen(&self, pattern: &Pattern) -> Option<Timestamp>;

    /// Returns the timestamp of the latest committed observation.
    ///
    /// The default delegates to [`Memory::stats`]. Built-in memories override
    /// this method so evaluating an input does not aggregate every unit.
    fn last_observed(&self) -> Option<Timestamp> {
        self.stats().last_observed
    }

    /// Returns a stable identity for the algorithm and configuration.
    ///
    /// Persistent generic engines store this value and reject a loader whose
    /// components do not match the saved components.
    fn fingerprint(&self) -> u64;

    /// Returns the component identity expected by a persistence format.
    ///
    /// Implementations whose identity changed after an older format may
    /// override this method to accept a compatible legacy identity.
    fn fingerprint_for_format(&self, _format_version: u16) -> u64 {
        self.fingerprint()
    }

    /// Removes all remembered state from this memory.
    fn forget(&mut self);

    /// Returns a snapshot of the memory state.
    fn stats(&self) -> MemoryStats;

    /// Returns the number of owned state elements used by this memory.
    ///
    /// The core uses this conservative count for the engine-wide memory
    /// budget. Implementations with additional collections should override
    /// it and count every element that their persistence loader allocates.
    fn allocation_units(&self) -> usize {
        let stats = self.stats();
        stats
            .pattern_count
            .saturating_add(stats.remembered_unit_count)
    }

    /// Returns a conservative allocation count after recording `pattern`.
    ///
    /// This is evaluated before a transaction is committed so the core can
    /// reject a stream without partially updating it.
    fn projected_allocation_units(&self, pattern: &Pattern) -> usize {
        self.allocation_units()
            .saturating_add(pattern.units().len().saturating_mul(2))
            .saturating_add(1)
    }

    /// Returns whether this memory contains no learned state.
    ///
    /// A per-stream factory must return a fresh empty memory. The default uses
    /// the public statistics as the emptiness check.
    fn is_empty(&self) -> bool {
        let stats = self.stats();
        stats.pattern_count == 0
            && stats.remembered_unit_count == 0
            && stats.last_observed.is_none()
    }
}

/// Persistence operations required by [`crate::Habituation::save`] and
/// [`crate::Habituation::load_with`].
pub trait PersistentMemory: Memory {
    /// Writes this memory's state after the core persistence header.
    fn save_state(&self, writer: &mut dyn Write) -> std::io::Result<()>;

    /// Replaces this memory's state from the core persistence stream.
    fn load_state(&mut self, reader: &mut dyn Read) -> std::io::Result<()>;

    /// Writes state using a specific core persistence format.
    fn save_state_for_format(
        &self,
        writer: &mut dyn Write,
        _format_version: u16,
    ) -> std::io::Result<()> {
        self.save_state(writer)
    }

    /// Replaces state using a specific core persistence format.
    ///
    /// The caller's budget covers allocations made while loading the whole
    /// engine. Implementations must reserve collection elements from it
    /// before allocating them and return `InvalidData` when the budget is
    /// insufficient.
    fn load_state_for_format(
        &mut self,
        reader: &mut dyn Read,
        _format_version: u16,
        load_budget: &mut crate::persistence::LoadBudget,
    ) -> std::io::Result<()>;

    /// Reserves the same allocation units that [`Self::load_state_for_format`]
    /// will use for this state.
    ///
    /// The core calls this before writing any bytes. This makes a successful
    /// save loadable under the same cumulative allocation budget.
    fn reserve_load_budget(
        &self,
        format_version: u16,
        load_budget: &mut crate::persistence::LoadBudget,
    ) -> std::io::Result<()>;
}

/// Errors produced by a [`Memory`] implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryError {
    /// An observation timestamp precedes the previous timestamp in the same
    /// stream.
    TimestampOutOfOrder {
        /// The previous timestamp.
        previous: Timestamp,
        /// The timestamp supplied by the caller.
        current: Timestamp,
    },
    /// A pattern contains a unit outside the configured projection.
    UnitOutOfRange {
        /// Unit index returned by the encoder.
        unit: usize,
        /// Number of units configured for the memory.
        encoder_units: usize,
    },
    /// The observation order counter cannot represent another observation.
    ObservationSequenceOverflow,
    /// The factory could not create a memory instance.
    FactoryCreationFailed,
    /// A memory factory returned a memory that already contains learned state.
    FactoryReturnedNonEmpty,
    /// A memory created after the first one has a different fingerprint.
    FactoryFingerprintMismatch {
        /// Fingerprint recorded from the first factory result.
        expected: u64,
        /// Fingerprint returned by a later factory result.
        actual: u64,
    },
}

impl fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimestampOutOfOrder { previous, current } => write!(
                formatter,
                "observation timestamp {current} precedes previous timestamp {previous}"
            ),
            Self::UnitOutOfRange {
                unit,
                encoder_units,
            } => write!(
                formatter,
                "pattern unit {unit} is outside encoder range 0..{encoder_units}"
            ),
            Self::ObservationSequenceOverflow => {
                write!(formatter, "memory observation sequence overflowed")
            }
            Self::FactoryCreationFailed => {
                write!(formatter, "memory factory failed to create memory")
            }
            Self::FactoryReturnedNonEmpty => {
                write!(formatter, "memory factory returned a non-empty memory")
            }
            Self::FactoryFingerprintMismatch { expected, actual } => write!(
                formatter,
                "memory factory fingerprint changed: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for MemoryError {}

/// Postcondition violations returned by a custom [`Memory`] implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryObservationError {
    /// A familiarity or novelty field was not finite.
    NonFiniteValue {
        /// Name of the invalid field.
        field: &'static str,
    },
    /// A familiarity or novelty field was outside `0..=1`.
    ValueOutOfRange {
        /// Name of the invalid field.
        field: &'static str,
    },
    /// The returned pattern fingerprint did not identify the input pattern.
    PatternIdMismatch {
        /// Fingerprint of the input pattern.
        expected: PatternId,
        /// Fingerprint returned by the memory.
        actual: PatternId,
    },
    /// The returned recency did not match the exact-pattern record.
    RecencyMismatch {
        /// Recency calculated from [`Memory::last_seen`].
        expected: Option<Duration>,
        /// Recency returned by the memory.
        actual: Option<Duration>,
    },
    /// The memory reported a last-seen timestamp after the current timestamp.
    PatternTimestampInFuture {
        /// Current observation timestamp.
        current: Timestamp,
        /// Timestamp reported by the memory.
        last_seen: Timestamp,
    },
    /// Novelty was not the complement of familiarity.
    NoveltyFamiliarityMismatch,
}

impl fmt::Display for MemoryObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteValue { field } => {
                write!(formatter, "memory field {field} is not finite")
            }
            Self::ValueOutOfRange { field } => {
                write!(formatter, "memory field {field} is outside 0..=1")
            }
            Self::PatternIdMismatch { expected, actual } => write!(
                formatter,
                "memory returned pattern id {actual}, expected {expected}"
            ),
            Self::RecencyMismatch { expected, actual } => write!(
                formatter,
                "memory returned recency {actual:?}, expected {expected:?}"
            ),
            Self::PatternTimestampInFuture { current, last_seen } => write!(
                formatter,
                "memory pattern timestamp {last_seen} is after observation timestamp {current}"
            ),
            Self::NoveltyFamiliarityMismatch => {
                write!(
                    formatter,
                    "memory novelty is not the complement of familiarity"
                )
            }
        }
    }
}

impl std::error::Error for MemoryObservationError {}
