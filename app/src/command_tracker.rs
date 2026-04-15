//! Command ACK / status verification.
//!
//! When we send a SET command (set temperature, toggle pump, etc.), the spa has
//! no explicit ACK -- it just broadcasts status ~1/sec. This module tracks pending
//! commands and verifies them against subsequent status updates.

extern crate alloc;

use alloc::vec::Vec;
use embassy_time::{Duration, Instant};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::{HeatingMode, PumpState, StatusUpdate, TempRange};
use log::{warn, debug};

/// How long to wait for a command to be reflected in status before timing out.
const COMMAND_ACK_TIMEOUT_SECS: u64 = 5;

/// Maximum number of retries for a failed command.
const MAX_RETRIES: u8 = 2;

/// Maximum number of pending commands to prevent heap exhaustion on the ESP32.
const MAX_PENDING_COMMANDS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedChange {
    PumpOn { item: ToggleItem },
    PumpOff { item: ToggleItem },
    TemperatureSet { temp: u8 },
    LightToggled { item: ToggleItem, pre_state: bool },
    HoldModeToggled { pre_state: bool },
    HeatingModeToggled { pre_mode: HeatingMode },
    TempRangeToggled { pre_range: TempRange },
}

impl ExpectedChange {
    fn from_command(cmd: &Command, pre_status: &StatusUpdate) -> Option<Self> {
        match cmd {
            Command::ToggleItem(item) => {
                // Toggle means we expect the opposite state
                if let Some(idx) = item.pump_index() {
                    let is_on = matches!(pre_status.pumps[idx], PumpState::Low | PumpState::High);
                    Some(if is_on {
                        ExpectedChange::PumpOff { item: *item }
                    } else {
                        ExpectedChange::PumpOn { item: *item }
                    })
                } else if let Some(idx) = item.light_index() {
                    Some(ExpectedChange::LightToggled { item: *item, pre_state: pre_status.lights[idx] })
                } else {
                    match item {
                        ToggleItem::Blower => {
                            Some(if pre_status.blower {
                                ExpectedChange::PumpOff { item: *item }
                            } else {
                                ExpectedChange::PumpOn { item: *item }
                            })
                        }
                        ToggleItem::HoldMode => Some(ExpectedChange::HoldModeToggled { pre_state: pre_status.is_hold }),
                        ToggleItem::HeatingMode => Some(ExpectedChange::HeatingModeToggled { pre_mode: pre_status.heating_mode }),
                        ToggleItem::TemperatureRange => Some(ExpectedChange::TempRangeToggled { pre_range: pre_status.temp_range }),
                        _ => None,
                    }
                }
            }
            Command::SetTemperature(temp) => Some(ExpectedChange::TemperatureSet { temp: *temp }),
            _ => None,
        }
    }
}

struct PendingCommand {
    command: Command,
    expected: ExpectedChange,
    sent_at: Instant,
    retries: u8,
}

/// Tracks pending commands and verifies them against status updates.
/// If a command is not confirmed within the timeout, it is retried up to
/// MAX_RETRIES times. After that, it is dropped and a warning is logged.
pub struct CommandTracker {
    pending: Vec<PendingCommand>,
}

impl CommandTracker {
    pub fn new() -> Self {
        CommandTracker {
            pending: Vec::new(),
        }
    }

    /// Record a command that was just sent, along with the pre-command status
    /// so we know what state change to expect.
    pub fn track(&mut self, command: Command, pre_status: &StatusUpdate) {
        if self.pending.len() >= MAX_PENDING_COMMANDS {
            warn!("CommandTracker full ({} pending), dropping command: {:?}", MAX_PENDING_COMMANDS, command);
            return;
        }
        if let Some(expected) = ExpectedChange::from_command(&command, pre_status) {
            debug!("Tracking command: {:?} expecting {:?}", command, expected);
            self.pending.push(PendingCommand {
                command,
                expected,
                sent_at: Instant::now(),
                retries: 0,
            });
        }
    }

    /// Check the current status against all pending commands.
    /// Returns a list of commands that timed out and should be retried.
    pub fn verify(&mut self, status: &StatusUpdate) -> Vec<Command> {
        let mut confirmed = alloc::vec![];
        let mut to_retry = alloc::vec![];

        for i in (0..self.pending.len()).rev() {
            let pending = &self.pending[i];
            let elapsed = pending.sent_at.elapsed();

            if Self::is_confirmed(&pending.expected, status) {
                debug!("Command confirmed: {:?}", pending.command);
                confirmed.push(i);
            } else if elapsed >= Duration::from_secs(COMMAND_ACK_TIMEOUT_SECS) {
                if pending.retries < MAX_RETRIES {
                    warn!("Command timeout, retrying ({}/{}): {:?}",
                          pending.retries + 1, MAX_RETRIES, pending.command);
                    to_retry.push(i);
                } else {
                    warn!("Command failed after {} retries: {:?}", MAX_RETRIES, pending.command);
                    confirmed.push(i); // Remove it
                }
            }
        }

        // Build retry list before removing
        let mut retries = Vec::new();
        for &i in &to_retry {
            let pending = &mut self.pending[i];
            pending.retries += 1;
            pending.sent_at = Instant::now();
            retries.push(pending.command.clone());
        }

        // Remove confirmed (in reverse order to maintain indices)
        let mut to_remove = confirmed;
        to_remove.sort();
        for &i in to_remove.iter().rev() {
            self.pending.remove(i);
        }

        retries
    }

    fn is_confirmed(expected: &ExpectedChange, status: &StatusUpdate) -> bool {
        match expected {
            ExpectedChange::PumpOn { item } => {
                let is_on = if let Some(idx) = item.pump_index() {
                    matches!(status.pumps[idx], PumpState::Low | PumpState::High)
                } else {
                    match item {
                        ToggleItem::Blower => status.blower,
                        _ => false,
                    }
                };
                is_on
            }
            ExpectedChange::PumpOff { item } => {
                let is_off = if let Some(idx) = item.pump_index() {
                    status.pumps[idx] == PumpState::Off
                } else {
                    match item {
                        ToggleItem::Blower => !status.blower,
                        _ => false,
                    }
                };
                is_off
            }
            ExpectedChange::TemperatureSet { temp } => {
                // Set temperature is stored as raw value in status
                (status.set_temp as u8) == *temp
            }
            ExpectedChange::LightToggled { item, pre_state } => {
                if let Some(idx) = item.light_index() {
                    status.lights[idx] != *pre_state
                } else {
                    status.lights[0] != *pre_state
                }
            }
            ExpectedChange::HoldModeToggled { pre_state } => {
                status.is_hold != *pre_state
            }
            ExpectedChange::HeatingModeToggled { pre_mode } => {
                status.heating_mode != *pre_mode
            }
            ExpectedChange::TempRangeToggled { pre_range } => {
                status.temp_range != *pre_range
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}
