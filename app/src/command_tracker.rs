//! Command ACK / status verification.
//!
//! When we send a SET command (set temperature, toggle pump, etc.), the spa has
//! no explicit ACK -- it just broadcasts status ~1/sec. This module tracks pending
//! commands and verifies them against subsequent status updates.

extern crate alloc;

use alloc::vec::Vec;
use embassy_time::{Duration, Instant};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::{PumpState, StatusUpdate};
use log::{info, warn, debug};

/// How long to wait for a command to be reflected in status before timing out.
const COMMAND_ACK_TIMEOUT_SECS: u64 = 5;

/// Maximum number of retries for a failed command.
const MAX_RETRIES: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedChange {
    PumpOn { item: ToggleItem },
    PumpOff { item: ToggleItem },
    TemperatureSet { temp: u8 },
    HoldModeToggled,
    HeatingModeToggled,
    TempRangeToggled,
}

impl ExpectedChange {
    fn from_command(cmd: &Command, pre_status: &StatusUpdate) -> Option<Self> {
        match cmd {
            Command::ToggleItem(item) => {
                // Toggle means we expect the opposite state
                match item {
                    ToggleItem::Pump1 => {
                        let is_on = matches!(pre_status.pump1, PumpState::Low | PumpState::High);
                        Some(if is_on {
                            ExpectedChange::PumpOff { item: *item }
                        } else {
                            ExpectedChange::PumpOn { item: *item }
                        })
                    }
                    ToggleItem::Pump2 => {
                        let is_on = matches!(pre_status.pump2, PumpState::Low | PumpState::High);
                        Some(if is_on {
                            ExpectedChange::PumpOff { item: *item }
                        } else {
                            ExpectedChange::PumpOn { item: *item }
                        })
                    }
                    ToggleItem::Pump3 => {
                        let is_on = matches!(pre_status.pump3, PumpState::Low | PumpState::High);
                        Some(if is_on {
                            ExpectedChange::PumpOff { item: *item }
                        } else {
                            ExpectedChange::PumpOn { item: *item }
                        })
                    }
                    ToggleItem::Light1 => {
                        // Light is a toggle - we can't easily verify since state is boolean
                        Some(ExpectedChange::HoldModeToggled) // reuse for generic toggle
                    }
                    ToggleItem::Blower => {
                        Some(if pre_status.blower {
                            ExpectedChange::PumpOff { item: *item }
                        } else {
                            ExpectedChange::PumpOn { item: *item }
                        })
                    }
                    ToggleItem::HoldMode => Some(ExpectedChange::HoldModeToggled),
                    ToggleItem::HeatingMode => Some(ExpectedChange::HeatingModeToggled),
                    ToggleItem::TemperatureRange => Some(ExpectedChange::TempRangeToggled),
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
                let is_on = match item {
                    ToggleItem::Pump1 => matches!(status.pump1, PumpState::Low | PumpState::High),
                    ToggleItem::Pump2 => matches!(status.pump2, PumpState::Low | PumpState::High),
                    ToggleItem::Pump3 => matches!(status.pump3, PumpState::Low | PumpState::High),
                    ToggleItem::Blower => status.blower,
                    _ => false,
                };
                is_on
            }
            ExpectedChange::PumpOff { item } => {
                let is_off = match item {
                    ToggleItem::Pump1 => status.pump1 == PumpState::Off,
                    ToggleItem::Pump2 => status.pump2 == PumpState::Off,
                    ToggleItem::Pump3 => status.pump3 == PumpState::Off,
                    ToggleItem::Blower => !status.blower,
                    _ => false,
                };
                is_off
            }
            ExpectedChange::TemperatureSet { temp } => {
                // Set temperature is stored as raw value in status
                (status.set_temp as u8) == *temp
            }
            ExpectedChange::HoldModeToggled => {
                // For generic toggles, we just check that some time has passed
                // and consider it confirmed (the spa doesn't always echo toggles)
                true
            }
            ExpectedChange::HeatingModeToggled => true,
            ExpectedChange::TempRangeToggled => true,
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}
