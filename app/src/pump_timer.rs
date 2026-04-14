//! Timed pump toggle (P1 mode) using embassy time.

extern crate alloc;

use alloc::vec::Vec;
use embassy_time::{Duration, Instant};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::PumpState;
use log::info;

const DEFAULT_PUMP_DURATION_SECS: u64 = 20 * 60;

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
}
