use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

use crate::config::{
    Config, ConfigError, LEGACY_FEATURE_SCHEMA_VERSION, LEGACY_INPUT_STRENGTH_SCALE,
    LEGACY_LONG_FAMILIARITY_WEIGHT, LEGACY_LONG_REINFORCEMENT, LEGACY_MAX_PATTERNS,
    LEGACY_MODEL_VERSION, LEGACY_PROJECTION_DENSITY, LEGACY_SHORT_FAMILIARITY_WEIGHT,
    LEGACY_SHORT_REINFORCEMENT, LEGACY_TEMPORAL_SHIFT_SCALE, LEGACY_VARIANCE_FLOOR,
    LongLearningPolicy,
};
use crate::memory::MemoryError;
use crate::timestamp::Timestamp;

pub(crate) const MAGIC: &[u8; 8] = b"HABITUA\0";
pub(crate) const LEGACY_FORMAT_VERSION: u16 = 1;
pub(crate) const INTERMEDIATE_FORMAT_VERSION: u16 = 2;
pub(crate) const SEQUENCED_FORMAT_VERSION: u16 = 3;
pub(crate) const PREVIOUS_FORMAT_VERSION: u16 = 4;
pub(crate) const CURRENT_FORMAT_VERSION: u16 = 5;
const MAX_SERIALIZED_ITEMS: u64 = 4_000_000;
pub(crate) const MAX_LOAD_ELEMENTS: usize = 4_000_000;

/// Tracks the cumulative allocation units permitted while loading or saving
/// one state.
///
/// A unit is one owned collection element or one scalar slot. Custom
/// [`crate::PersistentMemory`] implementations must reserve their allocations
/// from this budget before creating collections, and must use the same count in
/// [`crate::PersistentMemory::reserve_load_budget`] before saving.
pub struct LoadBudget {
    remaining: usize,
}

impl LoadBudget {
    /// Creates a budget with the crate-wide maximum allocation limit.
    pub fn new() -> Self {
        Self {
            remaining: MAX_LOAD_ELEMENTS,
        }
    }

    /// Creates a budget with a caller-supplied limit capped at the crate maximum.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            remaining: limit.min(MAX_LOAD_ELEMENTS),
        }
    }

    /// Reserves allocation units for a collection or other owned data.
    pub fn reserve(&mut self, elements: usize) -> io::Result<()> {
        if elements > self.remaining {
            return Err(invalid_data(
                "serialized state exceeds the load allocation limit",
            ));
        }
        self.remaining -= elements;
        Ok(())
    }

    /// Returns the unconsumed allocation budget.
    pub fn remaining(&self) -> usize {
        self.remaining
    }
}

impl Default for LoadBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors produced while saving or loading a habitua state.
#[derive(Debug)]
pub enum PersistenceError {
    /// An underlying reader or writer failed.
    Io(io::Error),
    /// The input does not start with the habitua file marker.
    InvalidMagic,
    /// The input uses a format version this crate does not understand.
    UnsupportedVersion(u16),
    /// The input has a malformed value or violates a state invariant.
    InvalidData(String),
    /// A legacy state has no component identities and cannot use generic loading.
    LegacyComponentIdentityUnavailable,
    /// A supplied component differs from the component that saved the state.
    ComponentFingerprintMismatch {
        /// The component that did not match.
        component: &'static str,
    },
    /// A memory factory failed while reconstructing a stream.
    Memory(MemoryError),
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "persistence I/O error: {error}"),
            Self::InvalidMagic => write!(formatter, "invalid habitua persistence marker"),
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported habitua persistence version {version}"
                )
            }
            Self::InvalidData(message) => write!(formatter, "invalid habitua state: {message}"),
            Self::LegacyComponentIdentityUnavailable => write!(
                formatter,
                "legacy state does not contain component identities; use built-in loading"
            ),
            Self::ComponentFingerprintMismatch { component } => {
                write!(
                    formatter,
                    "saved {component} does not match supplied {component}"
                )
            }
            Self::Memory(error) => write!(formatter, "memory loading error: {error}"),
        }
    }
}

impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidMagic
            | Self::UnsupportedVersion(_)
            | Self::InvalidData(_)
            | Self::LegacyComponentIdentityUnavailable
            | Self::ComponentFingerprintMismatch { .. } => None,
            Self::Memory(error) => Some(error),
        }
    }
}

impl From<io::Error> for PersistenceError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::InvalidData {
            Self::InvalidData(error.to_string())
        } else {
            Self::Io(error)
        }
    }
}

pub(crate) fn write_header(writer: &mut dyn Write) -> io::Result<()> {
    writer.write_all(MAGIC)?;
    write_u16(writer, CURRENT_FORMAT_VERSION)
}

pub(crate) fn read_header(reader: &mut dyn Read) -> Result<u16, PersistenceError> {
    let mut magic = [0_u8; MAGIC.len()];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(PersistenceError::InvalidMagic);
    }
    let version = read_u16(reader)?;
    if version != LEGACY_FORMAT_VERSION
        && version != INTERMEDIATE_FORMAT_VERSION
        && version != SEQUENCED_FORMAT_VERSION
        && version != PREVIOUS_FORMAT_VERSION
        && version != CURRENT_FORMAT_VERSION
    {
        return Err(PersistenceError::UnsupportedVersion(version));
    }
    Ok(version)
}

pub(crate) fn write_config(writer: &mut dyn Write, config: &Config) -> io::Result<()> {
    write_u64(writer, config.dimensions as u64)?;
    write_u64(writer, config.encoder_units as u64)?;
    write_u64(writer, config.k as u64)?;
    write_u64(writer, config.seed)?;
    write_duration(writer, config.short_time_constant)?;
    write_duration(writer, config.long_time_constant)?;
    // These two slots are retained so versions 1 through 3 keep their layout.
    write_f32(writer, 0.0)?;
    write_f32(writer, 0.0)?;
    write_f32(writer, config.projection_density)?;
    write_f32(writer, config.short_reinforcement)?;
    write_f32(writer, config.long_reinforcement)?;
    write_f32(writer, config.short_familiarity_weight)?;
    write_f32(writer, config.long_familiarity_weight)?;
    write_f32(writer, config.input_strength_scale)?;
    write_f32(writer, config.temporal_shift_scale)?;
    write_f32(writer, config.variance_floor)?;
    write_u64(writer, config.max_patterns as u64)?;
    match &config.long_learning_policy {
        LongLearningPolicy::Revisit { interval } => {
            write_u8(writer, 0)?;
            write_duration(writer, *interval)?;
        }
        LongLearningPolicy::RepeatedNormal {
            consecutive_observations,
        } => {
            write_u8(writer, 1)?;
            write_u32(writer, *consecutive_observations)?;
        }
        LongLearningPolicy::LegacyEveryCommit => {
            // This marker is emitted only when a compatibility-loaded state
            // is migrated through save. New public configurations reject the
            // legacy policy before reaching this writer.
            write_u8(writer, 2)?;
            write_u32(writer, 0)?;
        }
    }
    write_u64(writer, config.max_streams as u64)?;
    write_u64(writer, config.max_memory_elements as u64)?;
    write_u32(writer, config.model_version)?;
    write_u32(writer, config.feature_schema_version)
}

pub(crate) fn read_config(reader: &mut dyn Read, version: u16) -> Result<Config, PersistenceError> {
    let dimensions = read_usize(reader)?;
    let encoder_units = read_usize(reader)?;
    let k = read_usize(reader)?;
    let seed = read_u64(reader)?;
    let short_time_constant = read_duration(reader)?;
    let long_time_constant = read_duration(reader)?;
    let _legacy_input_strength_weight = read_f32(reader)?;
    let _legacy_temporal_shift_weight = read_f32(reader)?;
    let (projection_density, short_reinforcement, long_reinforcement) =
        if version == LEGACY_FORMAT_VERSION {
            (
                LEGACY_PROJECTION_DENSITY,
                LEGACY_SHORT_REINFORCEMENT,
                LEGACY_LONG_REINFORCEMENT,
            )
        } else {
            (read_f32(reader)?, read_f32(reader)?, read_f32(reader)?)
        };
    let (short_familiarity_weight, long_familiarity_weight) = if version == LEGACY_FORMAT_VERSION {
        (
            LEGACY_SHORT_FAMILIARITY_WEIGHT,
            LEGACY_LONG_FAMILIARITY_WEIGHT,
        )
    } else {
        (read_f32(reader)?, read_f32(reader)?)
    };
    let (input_strength_scale, temporal_shift_scale, variance_floor, max_patterns) =
        if version == LEGACY_FORMAT_VERSION {
            (
                LEGACY_INPUT_STRENGTH_SCALE,
                LEGACY_TEMPORAL_SHIFT_SCALE,
                LEGACY_VARIANCE_FLOOR,
                LEGACY_MAX_PATTERNS,
            )
        } else {
            (
                read_f32(reader)?,
                read_f32(reader)?,
                read_f32(reader)?,
                read_usize(reader)?,
            )
        };
    let (
        long_learning_policy,
        max_streams,
        max_memory_elements,
        model_version,
        feature_schema_version,
    ) = if version >= CURRENT_FORMAT_VERSION {
        let mode = read_u8(reader)?;
        let policy = match mode {
            0 => LongLearningPolicy::Revisit {
                interval: read_duration(reader)?,
            },
            1 => LongLearningPolicy::RepeatedNormal {
                consecutive_observations: read_u32(reader)?,
            },
            2 => {
                let marker = read_u32(reader)?;
                if marker != 0 {
                    return Err(PersistenceError::InvalidData(
                        "invalid legacy learning policy marker".to_owned(),
                    ));
                }
                LongLearningPolicy::LegacyEveryCommit
            }
            _ => {
                return Err(PersistenceError::InvalidData(
                    "invalid long-learning policy".to_owned(),
                ));
            }
        };
        (
            policy,
            read_usize(reader)?,
            read_usize(reader)?,
            read_u32(reader)?,
            read_u32(reader)?,
        )
    } else if version == PREVIOUS_FORMAT_VERSION {
        let flag = read_u8(reader)?;
        if flag > 1 {
            return Err(PersistenceError::InvalidData(
                "invalid legacy long-reinforcement mode".to_owned(),
            ));
        }
        let interval = read_duration(reader)?;
        (
            if flag == 1 {
                LongLearningPolicy::Revisit { interval }
            } else {
                LongLearningPolicy::LegacyEveryCommit
            },
            4_096,
            crate::config::MAX_CONFIG_MEMORY_ELEMENTS,
            read_u32(reader)?,
            read_u32(reader)?,
        )
    } else {
        (
            LongLearningPolicy::LegacyEveryCommit,
            4_096,
            crate::config::MAX_CONFIG_MEMORY_ELEMENTS,
            LEGACY_MODEL_VERSION,
            LEGACY_FEATURE_SCHEMA_VERSION,
        )
    };
    let config = Config {
        dimensions,
        encoder_units,
        k,
        seed,
        projection_density,
        short_time_constant,
        long_time_constant,
        short_reinforcement,
        long_reinforcement,
        short_familiarity_weight,
        long_familiarity_weight,
        input_strength_scale,
        temporal_shift_scale,
        variance_floor,
        max_patterns,
        long_learning_policy,
        max_streams,
        max_memory_elements,
        model_version,
        feature_schema_version,
    };
    config
        .validate_for_format(
            version <= PREVIOUS_FORMAT_VERSION || config.long_learning_policy.is_legacy(),
        )
        .map_err(|error| PersistenceError::InvalidData(error.to_string()))?;
    Ok(config)
}

pub(crate) fn write_duration(writer: &mut dyn Write, duration: Duration) -> io::Result<()> {
    write_u64(writer, duration.as_secs())?;
    write_u32(writer, duration.subsec_nanos())
}

pub(crate) fn read_duration(reader: &mut dyn Read) -> io::Result<Duration> {
    let seconds = read_u64(reader)?;
    let nanoseconds = read_u32(reader)?;
    if nanoseconds >= 1_000_000_000 {
        return Err(invalid_data("duration nanoseconds are out of range"));
    }
    Ok(Duration::new(seconds, nanoseconds))
}

pub(crate) fn write_timestamp(writer: &mut dyn Write, timestamp: Timestamp) -> io::Result<()> {
    let (seconds, nanoseconds) = timestamp.parts();
    write_u64(writer, seconds)?;
    write_u32(writer, nanoseconds)
}

pub(crate) fn read_timestamp(reader: &mut dyn Read) -> io::Result<Timestamp> {
    let seconds = read_u64(reader)?;
    let nanoseconds = read_u32(reader)?;
    if nanoseconds >= 1_000_000_000 {
        return Err(invalid_data("timestamp nanoseconds are out of range"));
    }
    Ok(Timestamp::from_parts(seconds, nanoseconds))
}

pub(crate) fn read_count(reader: &mut dyn Read) -> io::Result<usize> {
    let count = read_u64(reader)?;
    if count > MAX_SERIALIZED_ITEMS {
        return Err(invalid_data("serialized collection is too large"));
    }
    usize::try_from(count).map_err(|_| invalid_data("serialized count does not fit usize"))
}

pub(crate) fn read_usize(reader: &mut dyn Read) -> io::Result<usize> {
    let value = read_u64(reader)?;
    usize::try_from(value).map_err(|_| invalid_data("serialized integer does not fit usize"))
}

pub(crate) fn write_u8(writer: &mut dyn Write, value: u8) -> io::Result<()> {
    writer.write_all(&[value])
}

pub(crate) fn read_u8(reader: &mut dyn Read) -> io::Result<u8> {
    let mut bytes = [0_u8; 1];
    reader.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

pub(crate) fn write_u16(writer: &mut dyn Write, value: u16) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

pub(crate) fn read_u16(reader: &mut dyn Read) -> io::Result<u16> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

pub(crate) fn write_u32(writer: &mut dyn Write, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

pub(crate) fn read_u32(reader: &mut dyn Read) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

pub(crate) fn write_u64(writer: &mut dyn Write, value: u64) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

pub(crate) fn read_u64(reader: &mut dyn Read) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

pub(crate) fn write_f32(writer: &mut dyn Write, value: f32) -> io::Result<()> {
    write_u32(writer, value.to_bits())
}

pub(crate) fn read_f32(reader: &mut dyn Read) -> io::Result<f32> {
    Ok(f32::from_bits(read_u32(reader)?))
}

pub(crate) fn write_f64(writer: &mut dyn Write, value: f64) -> io::Result<()> {
    write_u64(writer, value.to_bits())
}

pub(crate) fn read_f64(reader: &mut dyn Read) -> io::Result<f64> {
    Ok(f64::from_bits(read_u64(reader)?))
}

pub(crate) fn write_string(writer: &mut dyn Write, value: &str) -> io::Result<()> {
    write_u64(writer, value.len() as u64)?;
    writer.write_all(value.as_bytes())
}

pub(crate) fn read_string(
    reader: &mut dyn Read,
    load_budget: &mut LoadBudget,
) -> io::Result<String> {
    let length = read_count(reader)?;
    load_budget.reserve(length)?;
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| invalid_data("stream id is not valid UTF-8"))
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl From<ConfigError> for PersistenceError {
    fn from(error: ConfigError) -> Self {
        Self::InvalidData(error.to_string())
    }
}

impl From<MemoryError> for PersistenceError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}
