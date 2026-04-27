//! Rate-limited logging with suppress + count summary.
//!
//! Each log site gets its own `RateLog` instance. The first occurrence logs
//! immediately. Subsequent occurrences within the cooldown window are silently
//! counted. When the cooldown expires, a summary is emitted with the suppressed
//! count so the operator knows how many messages were dropped.
//!
//! This is a `no_std`-compatible, `AtomicU32`-based implementation that only
//! needs a monotonically increasing seconds counter.

use core::sync::atomic::{AtomicU32, Ordering};

/// Default cooldown between repeated log emissions (seconds).
/// Used by the app crate's rate-limited logging macros.
#[allow(dead_code)]
pub const RATE_LOG_COOLDOWN_SECS: u32 = 5;

/// Tracks rate-limiting state for a single log site.
///
/// Uses `AtomicU32` internally so it can be stored as a `static` or a struct
/// field without requiring `&mut self`.
pub struct RateLog {
    last_emit_secs: AtomicU32,
    suppressed: AtomicU32,
}

impl RateLog {
    /// Create a new rate limiter with no prior state.
    pub const fn new() -> Self {
        Self {
            last_emit_secs: AtomicU32::new(0),
            suppressed: AtomicU32::new(0),
        }
    }

    /// Check whether a log message should be emitted now.
    ///
    /// Returns `Ok(suppressed_count)` if the cooldown expired and the message
    /// should be emitted, where `suppressed_count` is the number of messages
    /// that were held back since the last emit.
    ///
    /// Returns `Err(suppressed_count)` if the message should be suppressed.
    /// The count includes the current message.
    ///
    /// # Arguments
    /// * `now_secs` - Monotonically increasing seconds-since-boot counter.
    /// * `cooldown_secs` - Minimum seconds between emissions for this site.
    pub fn check(&self, now_secs: u32, cooldown_secs: u32) -> Result<u32, u32> {
        let last = self.last_emit_secs.load(Ordering::Relaxed);
        let elapsed = now_secs.saturating_sub(last);

        // Emit if: first use (last == 0 and now > 0), or cooldown expired.
        // Note: we treat last==0 && now==0 as first-use to handle boot at t=0.
        let should_emit = last == 0 || elapsed >= cooldown_secs;

        if should_emit {
            let count = self.suppressed.swap(0, Ordering::Relaxed);
            self.last_emit_secs.store(now_secs, Ordering::Relaxed);
            Ok(count)
        } else {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            Err(self.suppressed.load(Ordering::Relaxed))
        }
    }
}

impl Default for RateLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_log_emits_immediately() {
        let rl = RateLog::new();
        assert_eq!(rl.check(1, 5), Ok(0));
    }

    #[test]
    fn test_second_within_cooldown_suppressed() {
        let rl = RateLog::new();
        let _ = rl.check(1, 5);
        assert_eq!(rl.check(2, 5), Err(1));
        assert_eq!(rl.check(4, 5), Err(2));
    }

    #[test]
    fn test_after_cooldown_emits_with_count() {
        let rl = RateLog::new();
        let _ = rl.check(1, 5);
        let _ = rl.check(2, 5);
        let _ = rl.check(4, 5);
        assert_eq!(rl.check(6, 5), Ok(2)); // 2 suppressed messages
    }

    #[test]
    fn test_count_resets_after_emit() {
        let rl = RateLog::new();
        let _ = rl.check(1, 5);
        let _ = rl.check(2, 5);
        let _ = rl.check(4, 5);
        let _ = rl.check(6, 5); // emit with count=2
        assert_eq!(rl.check(7, 5), Err(1)); // fresh suppression
    }

    #[test]
    fn test_custom_cooldown() {
        let rl = RateLog::new();
        let _ = rl.check(1, 10);
        assert_eq!(rl.check(5, 10), Err(1)); // within 10s cooldown
        assert_eq!(rl.check(11, 10), Ok(1)); // after cooldown
    }
}
