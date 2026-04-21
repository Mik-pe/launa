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

#[cfg(test)]
mod tests {
    use super::*;

    // --- Timestamp construction and accessors ---

    #[test]
    fn test_zero_constant_is_zero() {
        assert_eq!(Timestamp::ZERO.0, 0);
        assert!(Timestamp::ZERO.is_zero());
    }

    #[test]
    fn test_from_millis_and_as_millis() {
        let ts = Timestamp::from_millis(5_000);
        assert_eq!(ts.as_millis(), 5_000);
    }

    #[test]
    fn test_from_secs_converts_to_millis() {
        let ts = Timestamp::from_secs(7);
        assert_eq!(ts.as_millis(), 7_000);
        assert_eq!(ts.as_secs(), 7);
    }

    #[test]
    fn test_as_secs_truncates() {
        let ts = Timestamp::from_millis(2_500);
        assert_eq!(ts.as_secs(), 2);
    }

    #[test]
    fn test_is_zero_true_and_false() {
        assert!(Timestamp::ZERO.is_zero());
        assert!(Timestamp::from_millis(0).is_zero());
        assert!(!Timestamp::from_millis(1).is_zero());
    }

    // --- elapsed_since ---

    #[test]
    fn test_elapsed_since_basic() {
        let earlier = Timestamp::from_millis(1_000);
        let later = Timestamp::from_millis(3_000);
        assert_eq!(later.elapsed_since(earlier), 2_000);
    }

    #[test]
    fn test_elapsed_since_same_timestamp() {
        let ts = Timestamp::from_millis(5_000);
        assert_eq!(ts.elapsed_since(ts), 0);
    }

    #[test]
    fn test_elapsed_since_zero_timestamp() {
        let ts = Timestamp::from_millis(1_000);
        assert_eq!(ts.elapsed_since(Timestamp::ZERO), 1_000);
    }

    #[test]
    fn test_elapsed_since_future_returns_zero() {
        // If "earlier" is actually in the future, saturating_sub returns 0
        let earlier = Timestamp::from_millis(10_000);
        let later = Timestamp::from_millis(3_000);
        assert_eq!(later.elapsed_since(earlier), 0);
    }

    #[test]
    fn test_elapsed_since_max_values() {
        let earlier = Timestamp::from_millis(u64::MAX - 100);
        let later = Timestamp::from_millis(u64::MAX);
        assert_eq!(later.elapsed_since(earlier), 100);
    }

    // --- saturating_add ---

    #[test]
    fn test_saturating_add_basic() {
        let ts = Timestamp::from_millis(1_000);
        let result = ts.saturating_add(500);
        assert_eq!(result, Timestamp::from_millis(1_500));
    }

    #[test]
    fn test_saturating_add_zero() {
        let ts = Timestamp::from_millis(1_000);
        let result = ts.saturating_add(0);
        assert_eq!(result, ts);
    }

    #[test]
    fn test_saturating_add_overflow_saturates() {
        let ts = Timestamp::from_millis(u64::MAX);
        let result = ts.saturating_add(1);
        assert_eq!(result, Timestamp::from_millis(u64::MAX));
    }

    #[test]
    fn test_saturating_add_large_no_overflow() {
        let ts = Timestamp::from_millis(u64::MAX - 100);
        let result = ts.saturating_add(100);
        assert_eq!(result, Timestamp::from_millis(u64::MAX));
    }

    // --- Ordering and equality ---

    #[test]
    fn test_timestamp_ordering() {
        let a = Timestamp::from_millis(100);
        let b = Timestamp::from_millis(200);
        assert!(a < b);
        assert!(b > a);
        assert_eq!(a, a);
    }

    // --- Clock trait default methods ---

    /// A trivial clock that returns a fixed timestamp.
    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.0
        }
    }

    #[test]
    fn test_clock_now_ms_delegates() {
        let clock = FixedClock(Timestamp::from_millis(9_876));
        assert_eq!(clock.now_ms(), 9_876);
    }

    #[test]
    fn test_clock_elapsed_ms() {
        let clock = FixedClock(Timestamp::from_millis(5_000));
        assert_eq!(clock.elapsed_ms(3_000), 2_000);
    }

    #[test]
    fn test_clock_elapsed_ms_future_returns_zero() {
        let clock = FixedClock(Timestamp::from_millis(1_000));
        assert_eq!(clock.elapsed_ms(5_000), 0);
    }

    #[test]
    fn test_clock_elapsed_since() {
        let clock = FixedClock(Timestamp::from_millis(10_000));
        let earlier = Timestamp::from_millis(4_000);
        assert_eq!(clock.elapsed_since(earlier), 6_000);
    }
}
