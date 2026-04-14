//! Timed pump toggle (P1 mode).
//!
//! The spa panel's "P1" button turns on jet pumps for a fixed duration then auto-offs.
//! Since the Balboa protocol only has toggle commands, we implement the timer in firmware.

use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::PumpState;
use log::{debug, info};
use std::time::{Duration, Instant};

const DEFAULT_PUMP_DURATION: Duration = Duration::from_secs(20 * 60);

pub struct PumpTimer {
    pump: ToggleItem,
    started_at: Option<Instant>,
    duration: Duration,
    was_on: bool,
}

impl PumpTimer {
    pub fn new(pump: ToggleItem) -> Self {
        PumpTimer {
            pump,
            started_at: None,
            duration: DEFAULT_PUMP_DURATION,
            was_on: false,
        }
    }

    pub fn with_duration(pump: ToggleItem, duration: Duration) -> Self {
        PumpTimer {
            pump,
            started_at: None,
            duration,
            was_on: false,
        }
    }

    /// Start the timer. Returns a toggle command to turn the pump on.
    pub fn start(&mut self) -> Command {
        self.started_at = Some(Instant::now());
        self.was_on = true;
        Command::ToggleItem(self.pump)
    }

    /// Start the timer with a custom duration in minutes.
    pub fn start_with_minutes(&mut self, minutes: u32) -> Command {
        self.duration = Duration::from_secs(minutes as u64 * 60);
        self.start()
    }

    /// Cancel the timer. Does NOT send a command (caller should toggle off if desired).
    pub fn cancel(&mut self) {
        if self.started_at.is_some() {
            info!("Pump timer for {:?} cancelled", self.pump);
        }
        self.started_at = None;
    }

    /// Tick the timer. Returns a toggle command if the timer has expired (to turn pump off).
    /// Returns None if the timer is not running or hasn't expired yet.
    pub fn tick(&mut self, current_pump_state: PumpState) -> Option<Command> {
        if let Some(started_at) = self.started_at {
            // Detect if pump was manually turned off via the panel
            let is_on = matches!(current_pump_state, PumpState::Low | PumpState::High);
            if !is_on {
                info!("Pump timer for {:?} cancelled (pump turned off externally)", self.pump);
                self.started_at = None;
                return None;
            }

            if started_at.elapsed() >= self.duration {
                info!("Pump timer for {:?} expired, toggling off", self.pump);
                self.started_at = None;
                return Some(Command::ToggleItem(self.pump));
            }
        }
        None
    }

    /// Returns remaining time in seconds, or 0 if not running.
    pub fn remaining_secs(&self) -> u64 {
        if let Some(started_at) = self.started_at {
            let elapsed = started_at.elapsed();
            self.duration.as_secs().saturating_sub(elapsed.as_secs())
        } else {
            0
        }
    }

    pub fn is_running(&self) -> bool {
        self.started_at.is_some()
    }
}

/// Manages timers for all pumps.
pub struct PumpTimerManager {
    pub pump1: PumpTimer,
    pub pump2: PumpTimer,
    pub pump3: PumpTimer,
}

impl PumpTimerManager {
    pub fn new() -> Self {
        PumpTimerManager {
            pump1: PumpTimer::new(ToggleItem::Pump1),
            pump2: PumpTimer::new(ToggleItem::Pump2),
            pump3: PumpTimer::new(ToggleItem::Pump3),
        }
    }

    /// Tick all timers. Returns any expired toggle commands.
    pub fn tick_all(
        &mut self,
        pump1_state: PumpState,
        pump2_state: PumpState,
        pump3_state: PumpState,
    ) -> Vec<Command> {
        let mut commands = Vec::new();

        if let Some(cmd) = self.pump1.tick(pump1_state) {
            commands.push(cmd);
        }
        if let Some(cmd) = self.pump2.tick(pump2_state) {
            commands.push(cmd);
        }
        if let Some(cmd) = self.pump3.tick(pump3_state) {
            commands.push(cmd);
        }

        commands
    }
}
