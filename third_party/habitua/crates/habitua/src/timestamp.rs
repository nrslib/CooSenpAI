use std::fmt;
use std::time::Duration;

/// A caller-provided, monotonic timestamp.
///
/// `Timestamp` does not read the system clock. It wraps a `Duration` whose
/// origin is chosen by the caller, so tests and replayed event streams can use
/// deterministic time.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(Duration);

impl Timestamp {
    /// The zero timestamp.
    pub const ZERO: Self = Self(Duration::ZERO);

    /// Creates a timestamp measured in seconds from the caller's origin.
    pub const fn from_secs(seconds: u64) -> Self {
        Self(Duration::from_secs(seconds))
    }

    /// Creates a timestamp measured in milliseconds from the caller's origin.
    pub const fn from_millis(milliseconds: u64) -> Self {
        Self(Duration::from_millis(milliseconds))
    }

    /// Creates a timestamp measured in nanoseconds from the caller's origin.
    pub const fn from_nanos(nanoseconds: u64) -> Self {
        Self(Duration::from_nanos(nanoseconds))
    }

    /// Returns the timestamp as seconds and fractional nanoseconds.
    pub const fn as_duration(self) -> Duration {
        self.0
    }

    /// Returns the elapsed duration from `earlier`, or `None` if time moved
    /// backwards.
    pub fn checked_duration_since(self, earlier: Self) -> Option<Duration> {
        self.0.checked_sub(earlier.0)
    }

    pub(crate) const fn from_parts(seconds: u64, nanoseconds: u32) -> Self {
        Self(Duration::new(seconds, nanoseconds))
    }

    pub(crate) const fn parts(self) -> (u64, u32) {
        (self.0.as_secs(), self.0.subsec_nanos())
    }
}

impl From<Duration> for Timestamp {
    fn from(value: Duration) -> Self {
        Self(value)
    }
}

impl From<Timestamp> for Duration {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (seconds, nanoseconds) = self.parts();
        write!(formatter, "{seconds}.{nanoseconds:09}s")
    }
}
