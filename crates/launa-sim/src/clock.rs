//! Virtual clock for deterministic time in simulation and tests.
//!
//! `VirtualClock` implements [`launa_hal::Clock`] with a manually-advanceable
//! tick counter. Call [`VirtualClock::advance_ms`] to move time forward
//! deterministically.

use launa_hal::Clock;

/// A manually-advanceable clock for simulation and testing.
///
/// Time starts at 0 and only advances when you call [`advance_ms`](VirtualClock::advance_ms).
///
/// # Example
///
/// ```
/// use launa_hal::Clock;
/// use launa_sim::VirtualClock;
///
/// let clock = VirtualClock::new();
/// assert_eq!(clock.now_ms(), 0);
///
/// let start = clock.now_ms();
/// clock.advance_ms(5000);
/// assert_eq!(clock.elapsed_ms(start), 5000);
/// ```
pub struct VirtualClock {
    /// Current virtual time in milliseconds.
    now: core::cell::Cell<u64>,
}

impl VirtualClock {
    /// Create a new virtual clock starting at time 0.
    pub fn new() -> Self {
        VirtualClock {
            now: core::cell::Cell::new(0),
        }
    }

    /// Create a virtual clock starting at a specific time.
    pub fn starting_at(ms: u64) -> Self {
        VirtualClock {
            now: core::cell::Cell::new(ms),
        }
    }

    /// Advance virtual time by `ms` milliseconds.
    pub fn advance_ms(&self, ms: u64) {
        self.now.set(self.now.get().saturating_add(ms));
    }
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for VirtualClock {
    fn now_ms(&self) -> u64 {
        self.now.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launa_hal::Clock;

    #[test]
    fn test_virtual_clock_starts_at_zero() {
        let clock = VirtualClock::new();
        assert_eq!(clock.now_ms(), 0);
    }

    #[test]
    fn test_virtual_clock_default() {
        let clock = VirtualClock::default();
        assert_eq!(clock.now_ms(), 0);
    }

    #[test]
    fn test_virtual_clock_starting_at() {
        let clock = VirtualClock::starting_at(10_000);
        assert_eq!(clock.now_ms(), 10_000);
    }

    #[test]
    fn test_virtual_clock_advance() {
        let clock = VirtualClock::new();
        clock.advance_ms(1000);
        assert_eq!(clock.now_ms(), 1000);
        clock.advance_ms(500);
        assert_eq!(clock.now_ms(), 1500);
    }

    #[test]
    fn test_virtual_clock_elapsed() {
        let clock = VirtualClock::new();
        let start = clock.now_ms();

        clock.advance_ms(2500);
        assert_eq!(clock.elapsed_ms(start), 2500);
    }

    #[test]
    fn test_virtual_clock_elapsed_multiple_intervals() {
        let clock = VirtualClock::new();

        let t0 = clock.now_ms();
        clock.advance_ms(100);
        let t1 = clock.now_ms();
        assert_eq!(clock.elapsed_ms(t0), 100);

        clock.advance_ms(200);
        let t2 = clock.now_ms();
        assert_eq!(clock.elapsed_ms(t0), 300);
        assert_eq!(clock.elapsed_ms(t1), 200);
        assert_eq!(clock.elapsed_ms(t2), 0);
    }

    #[test]
    fn test_virtual_clock_elapsed_saturates() {
        let clock = VirtualClock::new();
        // Asking elapsed from a future time should return 0 (saturating)
        assert_eq!(clock.elapsed_ms(9999), 0);
    }

    #[test]
    fn test_virtual_clock_advance_overflow_saturates() {
        let clock = VirtualClock::starting_at(u64::MAX - 10);
        clock.advance_ms(100);
        // Should saturate at u64::MAX
        assert_eq!(clock.now_ms(), u64::MAX);
    }

    #[test]
    fn test_virtual_clock_implements_trait() {
        fn use_clock<C: Clock>(c: &C) -> u64 {
            c.now_ms()
        }
        let clock = VirtualClock::new();
        assert_eq!(use_clock(&clock), 0);
    }
}
