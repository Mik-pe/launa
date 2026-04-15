//! Real-time clock backed by `embassy_time`.
//!
//! `EmbassyClock` implements [`launa_hal::Clock`] by wrapping
//! `embassy_time::Instant::now()`. Use this in the ESP32 firmware; use
//! `VirtualClock` in simulation and tests.

use launa_hal::Clock;

/// Real-time clock using `embassy_time::Instant::now()`.
///
/// This is the production clock for the ESP32 firmware. It delegates to
/// the embassy runtime's monotonic timer.
pub struct EmbassyClock;

impl EmbassyClock {
    /// Create a new embassy-backed clock.
    pub const fn new() -> Self {
        EmbassyClock
    }
}

impl Default for EmbassyClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for EmbassyClock {
    fn now_ms(&self) -> u64 {
        embassy_time::Instant::now().as_millis()
    }
}
