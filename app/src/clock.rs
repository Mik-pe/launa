//! Real-time clock backed by `embassy_time`.
//!
//! `EmbassyClock` implements [`launa_hal::Clock`] by wrapping
//! `embassy_time::Instant::now()`. Use this in the ESP32 firmware; use
//! `VirtualClock` in simulation and tests.

use launa_hal::{Clock, Timestamp};

pub struct EmbassyClock;

impl EmbassyClock {
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
    fn now(&self) -> Timestamp {
        Timestamp(embassy_time::Instant::now().as_millis())
    }
}
