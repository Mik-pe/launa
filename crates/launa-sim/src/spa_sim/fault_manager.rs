//! Fault state management for the spa simulator.
//!
//! Manages fault state, transient fault countdowns, and fault log configuration.

use alloc::vec::Vec;

use launa_protocol::fault::FaultCode;

use super::config::FaultLogConfig;

/// Fault state management subsystem.
///
/// Tracks the active fault flag, transient fault auto-clear countdown,
/// and multi-entry fault log configuration.
pub struct FaultManager {
    /// Whether a fault is currently active (init_mode=0x02 in status frames).
    pub(crate) fault_active: bool,
    /// If > 0, the fault will auto-clear after this many ticks.
    pub(crate) transient_fault_remaining_ticks: u64,
    /// Custom fault log configuration. Defaults to the hardcoded fault log data.
    pub(crate) fault_log_config: FaultLogConfig,
    /// Ordered list of fault log entries for multi-entry fault logs.
    /// Each entry is a FaultLogConfig. Index 0 = entry_number 1.
    pub(crate) fault_log_entries: Vec<FaultLogConfig>,
}

impl FaultManager {
    pub fn new() -> Self {
        FaultManager {
            fault_active: false,
            transient_fault_remaining_ticks: 0,
            fault_log_config: FaultLogConfig::default(),
            fault_log_entries: Vec::new(),
        }
    }

    /// Simulate a fault state with the given fault code.
    pub fn simulate_fault_state(&mut self, code: FaultCode) {
        self.fault_active = true;
        self.fault_log_config.message_code = code;
        self.transient_fault_remaining_ticks = 0; // not transient
    }

    /// Clear the active fault state.
    pub fn clear_fault_state(&mut self) {
        self.fault_active = false;
        self.transient_fault_remaining_ticks = 0;
    }

    /// Simulate a transient fault that auto-clears after `ticks` ticks.
    pub fn simulate_transient_fault(&mut self, code: FaultCode, ticks: u64) {
        if ticks == 0 {
            self.fault_active = false;
            self.transient_fault_remaining_ticks = 0;
            return;
        }
        self.fault_active = true;
        self.fault_log_config.message_code = code;
        self.transient_fault_remaining_ticks = ticks;
    }

    /// Set a custom fault log configuration.
    pub fn set_fault_log_config(&mut self, config: FaultLogConfig) {
        self.fault_log_config = config;
    }

    /// Set a multi-entry fault log.
    pub fn set_fault_log_entries(&mut self, entries: Vec<FaultLogConfig>) {
        self.fault_log_entries = entries;
    }

    /// Decrement the transient fault countdown, clearing the fault when it reaches zero.
    pub fn tick_transient_fault_countdown(&mut self) {
        if self.transient_fault_remaining_ticks > 0 {
            self.transient_fault_remaining_ticks -= 1;
            if self.transient_fault_remaining_ticks == 0 {
                self.fault_active = false;
            }
        }
    }
}
