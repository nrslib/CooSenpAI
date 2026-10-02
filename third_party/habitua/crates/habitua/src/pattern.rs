use std::fmt;

/// Stable fingerprint for a sparse activation pattern.
///
/// This is not a collision-free identity. Callers that require guaranteed
/// uniqueness must retain the pattern units themselves.
pub type PatternId = u64;

/// A sparse pattern represented by the indices of its active units.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    units: Vec<usize>,
}

impl Pattern {
    /// Creates a pattern from distinct unit indices.
    pub fn new(mut units: Vec<usize>) -> Result<Self, PatternError> {
        if units.is_empty() {
            return Err(PatternError::Empty);
        }
        units.sort_unstable();
        if units.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(PatternError::DuplicateUnit);
        }
        Ok(Self { units })
    }

    /// Returns the active unit indices in ascending order.
    pub fn units(&self) -> &[usize] {
        &self.units
    }

    /// Returns the stable fingerprint derived from the active units.
    pub fn id(&self) -> PatternId {
        pattern_id(&self.units)
    }
}

pub(crate) fn pattern_id(units: &[usize]) -> PatternId {
    stable_hash(units.iter().map(|unit| *unit as u64))
}

pub(crate) fn stable_hash(words: impl IntoIterator<Item = u64>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut length = 0_u64;
    for word in words {
        hash ^= word;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        length += 1;
    }
    hash ^= length;
    hash.wrapping_mul(0x0000_0100_0000_01b3)
}

/// Errors found while constructing a sparse pattern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatternError {
    /// A pattern must contain at least one active unit.
    Empty,
    /// A unit may appear only once.
    DuplicateUnit,
}

impl fmt::Display for PatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "a pattern must contain an active unit"),
            Self::DuplicateUnit => write!(formatter, "a pattern cannot contain duplicate units"),
        }
    }
}

impl std::error::Error for PatternError {}
