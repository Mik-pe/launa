//! Rate limiting for MQTT commands.
//!
//! Tracks command count within a sliding time window to protect the
//! spa RS-485 bus from command flooding.

/// Maximum number of MQTT commands allowed per rate-limit window.
/// Protects the spa RS-485 bus from command flooding.
pub const RATE_LIMIT_MAX_COMMANDS: usize = 10;

/// Duration of the rate-limit window in milliseconds.
/// After this window elapses, the command counter resets.
pub const RATE_LIMIT_WINDOW_MS: u64 = 10_000;

/// Tracks command count within a sliding time window.
///
/// Commands exceeding `RATE_LIMIT_MAX_COMMANDS` per `RATE_LIMIT_WINDOW_MS`
/// are dropped to protect the spa RS-485 bus.
///
/// Uses the [`Clock`](launa_hal::Clock) trait for time injection, making it fully testable
/// on desktop without `embassy_time::Instant`.
pub struct RateLimiter {
    /// Number of commands seen in the current window.
    pub(crate) count: usize,
    /// Start time of the current window (milliseconds since epoch).
    pub(crate) window_start_ms: u64,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    /// Create a new rate limiter with no commands counted.
    pub const fn new() -> Self {
        RateLimiter {
            count: 0,
            window_start_ms: 0,
        }
    }

    /// Check if a command is allowed under the rate limit.
    ///
    /// Returns `true` if the command should be forwarded, `false` if it
    /// should be dropped. Automatically resets the window when it expires.
    ///
    /// # Arguments
    /// * `now_ms` - Current time in milliseconds from a Clock source.
    pub fn check(&mut self, now_ms: u64) -> bool {
        // Reset window if expired
        if now_ms.saturating_sub(self.window_start_ms) >= RATE_LIMIT_WINDOW_MS {
            self.count = 0;
            self.window_start_ms = now_ms;
        }

        self.count += 1;
        self.count <= RATE_LIMIT_MAX_COMMANDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_under_limit_passes() {
        let mut rl = RateLimiter::new();
        // All commands up to RATE_LIMIT_MAX_COMMANDS should pass
        for i in 1..=RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(1_000), "command {} should pass (under limit)", i);
        }
    }

    #[test]
    fn test_rate_limiter_over_limit_rejects() {
        let mut rl = RateLimiter::new();
        // Fill up to the limit
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(1_000));
        }
        // Next command should be rejected
        assert!(!rl.check(1_000), "command beyond limit should be rejected");
    }

    #[test]
    fn test_rate_limiter_window_resets_after_timeout() {
        let mut rl = RateLimiter::new();

        // Fill up to the limit at t=1000ms
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(1_000));
        }
        // Rejected at t=1000ms
        assert!(!rl.check(1_000));

        // Still rejected within the same window (t=5000ms, window not expired)
        assert!(!rl.check(5_000));

        // After window expires (RATE_LIMIT_WINDOW_MS = 10_000),
        // t=11000ms is 10000ms after window_start_ms=1000, so window resets
        assert!(rl.check(11_000), "command should pass after window expires");

        // Count should have reset — we can send RATE_LIMIT_MAX_COMMANDS - 1 more
        for i in 1..RATE_LIMIT_MAX_COMMANDS {
            assert!(
                rl.check(11_000),
                "command {} after window reset should pass",
                i
            );
        }
        // Next one should be rejected again
        assert!(
            !rl.check(11_000),
            "should be rejected after filling new window"
        );
    }

    #[test]
    fn test_rate_limiter_burst_of_max_plus_one_rejects_last() {
        let mut rl = RateLimiter::new();

        // Send exactly RATE_LIMIT_MAX_COMMANDS + 1 commands in a burst
        let mut passed = 0usize;
        let mut rejected = 0usize;
        for _ in 0..=RATE_LIMIT_MAX_COMMANDS {
            if rl.check(1_000) {
                passed += 1;
            } else {
                rejected += 1;
            }
        }

        assert_eq!(
            passed, RATE_LIMIT_MAX_COMMANDS,
            "exactly RATE_LIMIT_MAX_COMMANDS should pass"
        );
        assert_eq!(rejected, 1, "exactly 1 command should be rejected");
    }

    #[test]
    fn test_rate_limiter_new_starts_at_zero() {
        let rl = RateLimiter::new();
        assert_eq!(rl.count, 0);
        assert_eq!(rl.window_start_ms, 0);
    }

    #[test]
    fn test_rate_limiter_first_check_passes() {
        let mut rl = RateLimiter::new();
        assert!(rl.check(0));
    }

    #[test]
    fn test_rate_limiter_window_boundary_exact() {
        let mut rl = RateLimiter::new();

        // First command at t=0 starts the window
        assert!(rl.check(0));

        // Fill the rest
        for _ in 1..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(0));
        }
        assert!(!rl.check(0)); // over limit

        // Exactly at window boundary (RATE_LIMIT_WINDOW_MS) should reset
        assert!(
            rl.check(RATE_LIMIT_WINDOW_MS),
            "exactly at window boundary should reset and pass"
        );
    }

    #[test]
    fn test_rate_limiter_window_just_before_boundary_does_not_reset() {
        let mut rl = RateLimiter::new();

        // First command at t=0
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(0));
        }
        assert!(!rl.check(0)); // over limit

        // Just before boundary — window NOT expired
        assert!(
            !rl.check(RATE_LIMIT_WINDOW_MS - 1),
            "one ms before window boundary should still reject"
        );
    }

    #[test]
    fn test_rate_limiter_multiple_window_cycles() {
        let mut rl = RateLimiter::new();

        // Window 1: t=0
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(0));
        }
        assert!(!rl.check(0));

        // Window 2: t=RATE_LIMIT_WINDOW_MS
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(RATE_LIMIT_WINDOW_MS));
        }
        assert!(!rl.check(RATE_LIMIT_WINDOW_MS));

        // Window 3: t=2*RATE_LIMIT_WINDOW_MS
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(2 * RATE_LIMIT_WINDOW_MS));
        }
        assert!(!rl.check(2 * RATE_LIMIT_WINDOW_MS));
    }

    #[test]
    fn test_rate_limiter_rejects_continuous_after_limit() {
        let mut rl = RateLimiter::new();

        // Fill limit
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(100));
        }

        // Multiple rejections in a row
        for i in 0..5 {
            assert!(
                !rl.check(100 + i),
                "continuous command {} should be rejected",
                i
            );
        }
    }
}
