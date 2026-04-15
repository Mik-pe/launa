//! Testable time abstraction.
//!
//! Provides a [`Clock`] trait that abstracts time reading, enabling deterministic
//! time-dependent tests. Implementations:
//! - `EmbassyClock` (in `app/`) wraps `embassy_time::Instant::now()`
//! - `VirtualClock` (in `launa-sim`) is tick-based and manually advanceable
//!
//! The trait uses raw `u64` milliseconds to stay `no_std`-compatible without
//! pulling in `embassy-time` as a dependency of `launa-hal`.

/// A monotonically increasing time source.
///
/// All values are in milliseconds. Use this trait for timeout checks,
/// interval calculations, and duration comparisons — not for async scheduling
/// (keep using `Timer::after().await` for that).
pub trait Clock {
    /// Returns the current time in milliseconds (monotonically increasing).
    fn now_ms(&self) -> u64;

    /// Returns milliseconds elapsed since the given earlier timestamp.
    ///
    /// Uses saturating subtraction so it returns 0 if `earlier_ms` is in the future.
    fn elapsed_ms(&self, earlier_ms: u64) -> u64 {
        self.now_ms().saturating_sub(earlier_ms)
    }
}
