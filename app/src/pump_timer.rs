//! Timed pump toggle (P1 mode) using embassy time.
//!
//! Also includes hold mode safety timeout: auto-clears hold mode after
//! a configurable period to prevent forgetting the spa in hold mode.

extern crate alloc;

use alloc::vec::Vec;
use embassy_time::{Duration, Instant};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::PumpState;
use log::info;

const DEFAULT_PUMP_DURATION_SECS: u64 = 20 * 60;
const DEFAULT_HOLD_MODE_TIMEOUT_SECS: u64 = 60 * 60; // 60 minutes

pub struct PumpTimer {
    pump: ToggleItem,
    started_at: Option<Instant>,
    duration: Duration,
}

impl PumpTimer {
    pub fn new(pump: ToggleItem) -> Self {
        PumpTimer {
            pump,
            started_at: None,
            duration: Duration::from_secs(DEFAULT_PUMP_DURATION_SECS),
        }
    }

    pub fn with_duration(pump: ToggleItem, duration: Duration) -> Self {
        PumpTimer {
            pump,
            started_at: None,
            duration,
        }
    }

    pub fn start(&mut self) -> Command {
        self.started_at = Some(Instant::now());
        Command::ToggleItem(self.pump)
    }

    pub fn start_with_minutes(&mut self, minutes: u32) -> Command {
        self.duration = Duration::from_secs(minutes as u64 * 60);
        self.start()
    }

    pub fn cancel(&mut self) {
        if self.started_at.is_some() {
            info!("Pump timer for {:?} cancelled", self.pump);
        }
        self.started_at = None;
    }

    /// Tick the timer. Returns a toggle command if expired.
    pub fn tick(&mut self, current_state: PumpState) -> Option<Command> {
        if let Some(started_at) = self.started_at {
            let is_on = matches!(current_state, PumpState::Low | PumpState::High);
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

    pub fn remaining_secs(&self) -> u64 {
        if let Some(started_at) = self.started_at {
            self.duration.as_secs().saturating_sub(started_at.elapsed().as_secs())
        } else {
            0
        }
    }

    pub fn is_running(&self) -> bool {
        self.started_at.is_some()
    }
}

pub struct PumpTimerManager {
    pump1: PumpTimer,
    pump2: PumpTimer,
    pump3: PumpTimer,
}

impl PumpTimerManager {
    pub fn new() -> Self {
        PumpTimerManager {
            pump1: PumpTimer::new(ToggleItem::Pump1),
            pump2: PumpTimer::new(ToggleItem::Pump2),
            pump3: PumpTimer::new(ToggleItem::Pump3),
        }
    }

    pub fn tick_all(
        &mut self,
        p1: PumpState,
        p2: PumpState,
        p3: PumpState,
    ) -> Vec<Command> {
        let mut cmds = Vec::new();
        if let Some(c) = self.pump1.tick(p1) { cmds.push(c); }
        if let Some(c) = self.pump2.tick(p2) { cmds.push(c); }
        if let Some(c) = self.pump3.tick(p3) { cmds.push(c); }
        cmds
    }

    /// Start a pump timer by pump index (1-3) for the given duration in minutes.
    /// Returns the toggle command to turn the pump on, or None for invalid index.
    pub fn start_timer(&mut self, pump_index: u8, minutes: u32) -> Option<Command> {
        match pump_index {
            1 => Some(self.pump1.start_with_minutes(minutes)),
            2 => Some(self.pump2.start_with_minutes(minutes)),
            3 => Some(self.pump3.start_with_minutes(minutes)),
            _ => None,
        }
    }
}

/// Hold mode safety timer. If the spa enters hold mode, auto-clears it
/// after a configurable timeout (default 60 minutes) to prevent cold/unsafe water.
pub struct HoldModeTimer {
    entered_at: Option<Instant>,
    timeout: Duration,
}

impl HoldModeTimer {
    pub fn new() -> Self {
        HoldModeTimer {
            entered_at: None,
            timeout: Duration::from_secs(DEFAULT_HOLD_MODE_TIMEOUT_SECS),
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        HoldModeTimer {
            entered_at: None,
            timeout,
        }
    }

    /// Tick the timer with the current hold mode state.
    /// Returns a ToggleItem::HoldMode command if the timeout expired.
    pub fn tick(&mut self, is_hold: bool) -> Option<Command> {
        if is_hold {
            if self.entered_at.is_none() {
                info!("Hold mode detected, starting {}min safety timer", self.timeout.as_secs() / 60);
                self.entered_at = Some(Instant::now());
            } else if self.entered_at.unwrap().elapsed() >= self.timeout {
                info!("Hold mode safety timeout expired, auto-clearing");
                self.entered_at = None;
                return Some(Command::ToggleItem(ToggleItem::HoldMode));
            }
        } else {
            if self.entered_at.is_some() {
                info!("Hold mode cleared externally");
            }
            self.entered_at = None;
        }
        None
    }

    pub fn is_active(&self) -> bool {
        self.entered_at.is_some()
    }

    pub fn remaining_secs(&self) -> u64 {
        if let Some(entered_at) = self.entered_at {
            self.timeout.as_secs().saturating_sub(entered_at.elapsed().as_secs())
        } else {
            0
        }
    }
}
