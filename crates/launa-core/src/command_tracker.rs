//! Command tracking and verification.
//!
//! Tracks pending commands and verifies them against status updates
//! from the spa. Handles retry logic and timeout detection.

use alloc::vec::Vec;
use launa_hal::Timestamp;
use launa_protocol::command::Command;
use launa_protocol::command::ToggleItem;
use launa_protocol::status::{HeatingMode, PumpState, StatusUpdate, TempRange};

use crate::types::{COMMAND_ACK_TIMEOUT_MS, MAX_COMMAND_RETRIES, MAX_PENDING_COMMANDS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExpectedChange {
    PumpOn { item: ToggleItem },
    PumpOff { item: ToggleItem },
    TemperatureSet { temp: u8 },
    LightToggled { item: ToggleItem, pre_state: bool },
    HoldModeToggled { pre_state: bool },
    HeatingModeToggled { pre_mode: HeatingMode },
    TempRangeToggled { pre_range: TempRange },
}

impl ExpectedChange {
    pub(crate) fn from_command(cmd: &Command, pre_status: &StatusUpdate) -> Option<Self> {
        match cmd {
            Command::ToggleItem(item) => {
                if let Some(idx) = item.pump_index() {
                    let is_on = matches!(pre_status.pumps[idx], PumpState::Low | PumpState::High);
                    Some(if is_on {
                        ExpectedChange::PumpOff { item: *item }
                    } else {
                        ExpectedChange::PumpOn { item: *item }
                    })
                } else if let Some(idx) = item.light_index() {
                    Some(ExpectedChange::LightToggled {
                        item: *item,
                        pre_state: pre_status.lights[idx],
                    })
                } else {
                    match item {
                        ToggleItem::Blower => Some(if pre_status.blower {
                            ExpectedChange::PumpOff { item: *item }
                        } else {
                            ExpectedChange::PumpOn { item: *item }
                        }),
                        ToggleItem::HoldMode => Some(ExpectedChange::HoldModeToggled {
                            pre_state: pre_status.is_hold,
                        }),
                        ToggleItem::HeatingMode => Some(ExpectedChange::HeatingModeToggled {
                            pre_mode: pre_status.heating_mode,
                        }),
                        ToggleItem::TemperatureRange => Some(ExpectedChange::TempRangeToggled {
                            pre_range: pre_status.temp_range,
                        }),
                        _ => None,
                    }
                }
            }
            Command::SetTemperature(temp) => Some(ExpectedChange::TemperatureSet { temp: *temp }),
            _ => None,
        }
    }
}

pub(crate) struct PendingCommand {
    pub(crate) command: Command,
    pub(crate) expected: ExpectedChange,
    pub(crate) sent_at: Timestamp,
    pub(crate) retries: u8,
}

/// Tracks pending commands and verifies them against status updates.
pub struct CommandTracker {
    pub(crate) pending: Vec<PendingCommand>,
    pub(crate) dropped_count: u32,
    pub(crate) retry_count: u32,
}

/// Result of verifying pending commands against a status update.
pub struct VerifyResult {
    /// Commands that timed out and should be retried.
    pub retries: Vec<Command>,
    /// Number of commands that exceeded max retries and were dropped.
    pub dropped: u32,
}

impl Default for CommandTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandTracker {
    pub fn new() -> Self {
        CommandTracker {
            pending: Vec::new(),
            dropped_count: 0,
            retry_count: 0,
        }
    }

    pub fn track(&mut self, command: Command, pre_status: &StatusUpdate, now: Timestamp) {
        if self.pending.len() >= MAX_PENDING_COMMANDS {
            self.dropped_count += 1;
            return;
        }
        if let Some(expected) = ExpectedChange::from_command(&command, pre_status) {
            self.pending.push(PendingCommand {
                command,
                expected,
                sent_at: now,
                retries: 0,
            });
        }
    }

    pub fn verify(&mut self, status: &StatusUpdate, now: Timestamp) -> VerifyResult {
        let mut confirmed = Vec::new();
        let mut to_retry = Vec::new();
        let mut dropped_this_call: u32 = 0;

        for i in (0..self.pending.len()).rev() {
            let pending = &self.pending[i];
            let elapsed = now.elapsed_since(pending.sent_at);

            if Self::is_confirmed(&pending.expected, status) {
                confirmed.push(i);
            } else if elapsed >= COMMAND_ACK_TIMEOUT_MS {
                if pending.retries < MAX_COMMAND_RETRIES {
                    to_retry.push(i);
                } else {
                    confirmed.push(i);
                    dropped_this_call += 1;
                }
            }
        }

        let mut retries = Vec::new();
        for &i in &to_retry {
            let pending = &mut self.pending[i];
            pending.retries += 1;
            pending.sent_at = now;
            retries.push(pending.command.clone());
        }

        self.retry_count += retries.len() as u32;

        let mut to_remove = confirmed;
        to_remove.sort();
        for &i in to_remove.iter().rev() {
            self.pending.remove(i);
        }

        self.dropped_count += dropped_this_call;

        VerifyResult {
            retries,
            dropped: dropped_this_call,
        }
    }

    pub(crate) fn is_confirmed(expected: &ExpectedChange, status: &StatusUpdate) -> bool {
        match expected {
            ExpectedChange::PumpOn { item } => {
                if let Some(idx) = item.pump_index() {
                    matches!(status.pumps[idx], PumpState::Low | PumpState::High)
                } else {
                    match item {
                        ToggleItem::Blower => status.blower,
                        _ => false,
                    }
                }
            }
            ExpectedChange::PumpOff { item } => {
                if let Some(idx) = item.pump_index() {
                    status.pumps[idx] == PumpState::Off
                } else {
                    match item {
                        ToggleItem::Blower => !status.blower,
                        _ => false,
                    }
                }
            }
            ExpectedChange::TemperatureSet { temp } => {
                // set_temp is a Temperature that knows its scale.
                // to_wire() encodes back to raw byte (×2 for Celsius, direct for Fahrenheit).
                status.set_temp.to_wire() == *temp
            }
            ExpectedChange::LightToggled { item, pre_state } => {
                if let Some(idx) = item.light_index() {
                    status.lights[idx] != *pre_state
                } else {
                    status.lights[0] != *pre_state
                }
            }
            ExpectedChange::HoldModeToggled { pre_state } => status.is_hold != *pre_state,
            ExpectedChange::HeatingModeToggled { pre_mode } => status.heating_mode != *pre_mode,
            ExpectedChange::TempRangeToggled { pre_range } => status.temp_range != *pre_range,
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn total_dropped(&self) -> u32 {
        self.dropped_count
    }

    pub fn total_retries(&self) -> u32 {
        self.retry_count
    }

    /// Increment the dropped command counter (e.g. when the command queue is full).
    pub fn record_dropped(&mut self) {
        self.dropped_count += 1;
    }

    /// Reset all tracked state (e.g. on bus reset).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.dropped_count = 0;
        self.retry_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale, TimeFormat};
    use launa_protocol::Temperature;
    use launa_sim::VirtualClock;

    /// Helper: build a StatusUpdate with explicit set_temp and scale.
    fn make_status(set_temp: Temperature, scale: TemperatureScale) -> StatusUpdate {
        StatusUpdate {
            current_temp: Some(Temperature::celsius(38.0)),
            set_temp,
            hour: 0,
            minute: 0,
            heating_mode: HeatingMode::Ready,
            temperature_scale: scale,
            time_format: TimeFormat::Hour24,
            filter_mode: 0,
            is_heating: false,
            temp_range: TempRange::High,
            pumps: [PumpState::Off; 6],
            circ_pump: false,
            blower: false,
            mister: false,
            lights: [false; 4],
            is_priming: false,
            is_hold: false,
            notification_type: 0,
            panel_locked: false,
            settings_lock: false,
            m8_cycle_time: 0,
            sensor_a_temp: Some(set_temp),
            sensor_b_temp: None,
            hold_timer_minutes: None,
        }
    }

    /// Helper: build a Celsius StatusUpdate from a raw wire value.
    /// Raw 76 → set_temp=38.0, raw 77 → set_temp=38.5, etc.
    fn make_celsius_status(raw_set_temp: u8) -> StatusUpdate {
        make_status(
            Temperature::from_wire(raw_set_temp, TemperatureScale::Celsius),
            TemperatureScale::Celsius,
        )
    }

    /// Helper: build a Fahrenheit StatusUpdate from a raw wire value.
    fn make_fahrenheit_status(raw_set_temp: u8) -> StatusUpdate {
        make_status(
            Temperature::from_wire(raw_set_temp, TemperatureScale::Fahrenheit),
            TemperatureScale::Fahrenheit,
        )
    }

    /// Test is_confirmed directly for Fahrenheit.
    #[test]
    fn test_temperature_confirm_fahrenheit() {
        let expected = ExpectedChange::TemperatureSet { temp: 104 };
        let status = make_fahrenheit_status(104);
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Celsius SetTemperature(76) confirmed when status.set_temp=38.0 (raw 76/2).
    #[test]
    fn test_temperature_confirm_celsius_even_raw() {
        let expected = ExpectedChange::TemperatureSet { temp: 76 };
        let status = make_celsius_status(76); // set_temp = 38.0
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Celsius SetTemperature(77) confirmed when status.set_temp=38.5 (raw 77/2).
    #[test]
    fn test_temperature_confirm_celsius_odd_raw() {
        let expected = ExpectedChange::TemperatureSet { temp: 77 };
        let status = make_celsius_status(77); // set_temp = 38.5
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Celsius boundary odd value: SetTemperature(83) → set_temp=41.5.
    #[test]
    fn test_temperature_confirm_celsius_boundary_odd() {
        let expected = ExpectedChange::TemperatureSet { temp: 83 };
        let status = make_celsius_status(83); // set_temp = 41.5
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Temperature mismatch does NOT confirm; triggers retry in both scales.
    #[test]
    fn test_temperature_mismatch_triggers_retry_both_scales() {
        use launa_hal::Clock;

        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let mut tracker = CommandTracker::new();
        let now = clock.now();

        // Fahrenheit: SetTemperature(104), but spa reports set_temp=100
        let pre_status_f = make_fahrenheit_status(100);
        tracker.track(Command::SetTemperature(104), &pre_status_f, now);

        // Advance past ACK timeout
        clock.advance_ms(COMMAND_ACK_TIMEOUT_MS + 1);
        let now_after = clock.now();

        let status_f = make_fahrenheit_status(100);
        let result = tracker.verify(&status_f, now_after);
        assert_eq!(result.retries.len(), 1);
        assert_eq!(result.dropped, 0);

        // Celsius: SetTemperature(76), but spa reports set_temp=74/2=37.0
        let mut tracker_c = CommandTracker::new();
        clock.advance_ms(10_000); // ensure fresh timestamps
        let now2 = clock.now();

        let pre_status_c = make_celsius_status(74); // 37.0
        tracker_c.track(Command::SetTemperature(76), &pre_status_c, now2);

        clock.advance_ms(COMMAND_ACK_TIMEOUT_MS + 1);
        let now2_after = clock.now();

        let status_c = make_celsius_status(74); // 37.0 — mismatch
        let result_c = tracker_c.verify(&status_c, now2_after);
        assert_eq!(result_c.retries.len(), 1);
        assert_eq!(result_c.dropped, 0);
    }

    /// Bug 7 fix: track() increments dropped_count when pending queue is full.
    #[test]
    fn test_track_drops_when_pending_full() {
        use launa_hal::Clock;

        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let mut tracker = CommandTracker::new();
        let now = clock.now();
        let pre_status = make_celsius_status(76);

        // Fill pending up to MAX_PENDING_COMMANDS
        for i in 0..MAX_PENDING_COMMANDS {
            let temp = 76 + i as u8;
            tracker.track(Command::SetTemperature(temp), &pre_status, now);
        }
        assert_eq!(tracker.pending_count(), MAX_PENDING_COMMANDS);
        assert_eq!(tracker.total_dropped(), 0);

        // Next track() should silently drop and increment dropped_count
        tracker.track(Command::SetTemperature(100), &pre_status, now);
        assert_eq!(
            tracker.pending_count(),
            MAX_PENDING_COMMANDS,
            "pending should stay at max"
        );
        assert_eq!(tracker.total_dropped(), 1, "dropped_count should increment");

        // Another track — accumulates
        tracker.track(Command::SetTemperature(101), &pre_status, now);
        assert_eq!(tracker.pending_count(), MAX_PENDING_COMMANDS);
        assert_eq!(tracker.total_dropped(), 2);
    }
}
