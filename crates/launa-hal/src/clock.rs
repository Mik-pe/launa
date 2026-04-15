//! Testable time abstraction.
//!
//! Provides a [`Clock`] trait and [`Timestamp`] newtype that abstract time reading,
//! enabling deterministic time-dependent tests. Implementations:
//! - `EmbassyClock` (in `app/`) wraps `embassy_time::Instant::now()`
//! - `VirtualClock` (in `launa-sim`) is tick-based and manually advanceable
//!
//! All values use raw `u64` milliseconds to stay `no_std`-compatible without
//! pulling in `embassy-time` as a dependency of `launa-hal`.

/// A millisecond timestamp newtype.
///
/// Wraps a `u64` millisecond count from a monotonic clock. Use this instead of
/// bare `u64` for all time-related state (timeouts, timers, intervals) so that
/// the time source is always injectable and testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// The zero timestamp (epoch).
    pub const ZERO: Timestamp = Timestamp(0);

    /// Create a timestamp from milliseconds.
    pub const fn from_millis(ms: u64) -> Self {
        Timestamp(ms)
    }

    /// Create a timestamp from seconds.
    pub const fn from_secs(secs: u64) -> Self {
        Timestamp(secs * 1000)
    }

    /// Returns the raw milliseconds value.
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// Returns the whole seconds portion.
    pub const fn as_secs(self) -> u64 {
        self.0 / 1000
    }

    /// Returns milliseconds elapsed since an earlier timestamp.
    /// Uses saturating subtraction so it returns 0 if `earlier` is in the future.
    pub fn elapsed_since(self, earlier: Timestamp) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    /// Returns a new timestamp advanced by `ms` milliseconds.
    /// Uses saturating addition.
    pub fn saturating_add(self, ms: u64) -> Timestamp {
        Timestamp(self.0.saturating_add(ms))
    }

    /// Returns true if this timestamp is zero (unset).
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

/// A monotonically increasing time source.
///
/// All values are in milliseconds. Use this trait for timeout checks,
/// interval calculations, and duration comparisons — not for async scheduling
/// (keep using `Timer::after().await` for that).
pub trait Clock {
    /// Returns the current time as a [`Timestamp`].
    fn now(&self) -> Timestamp;

    /// Returns the current time in milliseconds (monotonically increasing).
    fn now_ms(&self) -> u64 {
        self.now().as_millis()
    }

    /// Returns milliseconds elapsed since the given earlier timestamp.
    ///
    /// Uses saturating subtraction so it returns 0 if `earlier_ms` is in the future.
    fn elapsed_ms(&self, earlier_ms: u64) -> u64 {
        self.now_ms().saturating_sub(earlier_ms)
    }

    /// Returns milliseconds elapsed since the given [`Timestamp`].
    fn elapsed_since(&self, earlier: Timestamp) -> u64 {
        self.now().elapsed_since(earlier)
    }
}
