//! Error injection subsystem for the spa simulator.
//!
//! Manages command acceptance rates, bus silence, and frame corruption/duplication
//! for testing error handling in the firmware.

/// Error injection subsystem.
///
/// Controls the simulated error conditions:
/// - **Command success rate**: Probabilistic command acceptance/dropping
/// - **Bus silence**: Suppresses all output for a configurable number of ticks
/// - **Corrupt frame injection**: One-shot injection of a frame with bad CRC
/// - **Duplicate frame injection**: One-shot duplication of the status frame
pub struct ErrorInjection {
    /// Probability that commands are accepted (0.0 = never, 1.0 = always).
    pub(crate) command_success_rate: f32,
    /// Counter used for deterministic PRNG in command acceptance.
    pub(crate) command_counter: u64,
    /// Remaining ticks of bus silence (0 = no silence).
    pub(crate) bus_silence_remaining: u64,
    /// Whether to inject a corrupt frame on the next status frame generation.
    pub(crate) inject_corrupt_next: bool,
    /// Whether to duplicate the next status frame.
    pub(crate) duplicate_next: bool,
}

impl Default for ErrorInjection {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorInjection {
    pub fn new() -> Self {
        ErrorInjection {
            command_success_rate: 1.0,
            command_counter: 0,
            bus_silence_remaining: 0,
            inject_corrupt_next: false,
            duplicate_next: false,
        }
    }

    /// Set the probability that commands are accepted (0.0 = never, 1.0 = always).
    ///
    /// Uses a deterministic PRNG seeded by a per-command counter for reproducibility.
    pub fn set_command_success_rate(&mut self, rate: f32) {
        self.command_success_rate = rate.clamp(0.0, 1.0);
    }

    /// Simulate bus silence: suppress all output for `duration_ticks` ticks.
    pub fn simulate_bus_silence(&mut self, duration_ticks: u64) {
        self.bus_silence_remaining = duration_ticks;
    }

    /// Inject a corrupt frame on the next `generate_status_frame()` call.
    pub fn inject_corrupt_frame(&mut self) {
        self.inject_corrupt_next = true;
    }

    /// Inject a duplicate status frame on the next `tick()` call.
    pub fn inject_duplicate_frame(&mut self) {
        self.duplicate_next = true;
    }

    /// Deterministic pseudo-random check for command acceptance.
    ///
    /// Returns `true` if the command should be accepted based on the success rate.
    pub fn should_accept_command(&mut self) -> bool {
        let rate = self.command_success_rate;
        if rate >= 1.0 {
            return true;
        }
        if rate <= 0.0 {
            return false;
        }
        // Simple LCG-based deterministic "random"
        let rand_val = (self
            .command_counter
            .wrapping_mul(1103515245)
            .wrapping_add(12345)
            >> 16) as u8;
        self.command_counter += 1;
        let threshold = (rate * 256.0) as u8;
        rand_val < threshold
    }

    /// Check if bus silence is active.
    pub fn is_silent(&self) -> bool {
        self.bus_silence_remaining > 0
    }

    /// Decrement bus silence counter. Returns true if silence was active this tick.
    pub fn tick_bus_silence(&mut self) -> bool {
        if self.bus_silence_remaining > 0 {
            self.bus_silence_remaining -= 1;
            true
        } else {
            false
        }
    }

    /// Consume the corrupt-next flag (one-shot).
    pub fn take_corrupt_next(&mut self) -> bool {
        let v = self.inject_corrupt_next;
        if v {
            self.inject_corrupt_next = false;
        }
        v
    }

    /// Consume the duplicate-next flag (one-shot).
    pub fn take_duplicate_next(&mut self) -> bool {
        let v = self.duplicate_next;
        if v {
            self.duplicate_next = false;
        }
        v
    }
}
