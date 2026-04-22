//! Pump timers and hold mode safety timer.
//!
//! Provides automatic pump shutoff timers and a hold mode safety
//! timeout that automatically releases hold mode after a configurable period.

use alloc::vec::Vec;
use launa_hal::Timestamp;
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::PumpState;

use crate::types::{DEFAULT_HOLD_MODE_TIMEOUT_MS, DEFAULT_PUMP_DURATION_MS};

/// Timer for a single pump. Fires a toggle-off command after a configurable duration.
pub struct PumpTimer {
    pump: ToggleItem,
    started_at: Option<Timestamp>,
    duration_ms: u64,
}

impl PumpTimer {
    pub fn new(pump: ToggleItem) -> Self {
        PumpTimer {
            pump,
            started_at: None,
            duration_ms: DEFAULT_PUMP_DURATION_MS,
        }
    }

    pub fn start(&mut self, now: Timestamp) -> Command {
        self.started_at = Some(now);
        Command::ToggleItem(self.pump)
    }

    pub fn start_with_minutes(&mut self, minutes: u32, now: Timestamp) -> Command {
        self.duration_ms = minutes as u64 * 60 * 1000;
        self.start(now)
    }

    pub fn cancel(&mut self) {
        self.started_at = None;
    }

    pub fn tick(&mut self, now: Timestamp, current_state: PumpState) -> Option<Command> {
        if let Some(started_at) = self.started_at {
            let is_on = matches!(current_state, PumpState::Low | PumpState::High);
            if !is_on {
                self.started_at = None;
                return None;
            }
            if now.elapsed_since(started_at) >= self.duration_ms {
                self.started_at = None;
                return Some(Command::ToggleItem(self.pump));
            }
        }
        None
    }

    pub fn remaining_ms(&self, now: Timestamp) -> u64 {
        if let Some(started_at) = self.started_at {
            self.duration_ms
                .saturating_sub(now.elapsed_since(started_at))
        } else {
            0
        }
    }

    pub fn is_running(&self) -> bool {
        self.started_at.is_some()
    }
}

/// Manages pump timers for all 6 pumps.
pub struct PumpTimerManager {
    timers: [PumpTimer; 6],
}

impl Default for PumpTimerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PumpTimerManager {
    pub fn new() -> Self {
        PumpTimerManager {
            timers: [
                PumpTimer::new(ToggleItem::Pump1),
                PumpTimer::new(ToggleItem::Pump2),
                PumpTimer::new(ToggleItem::Pump3),
                PumpTimer::new(ToggleItem::Pump4),
                PumpTimer::new(ToggleItem::Pump5),
                PumpTimer::new(ToggleItem::Pump6),
            ],
        }
    }

    pub fn tick_all(&mut self, now: Timestamp, pumps: &[PumpState; 6]) -> Vec<Command> {
        let mut cmds = Vec::new();
        for (i, timer) in self.timers.iter_mut().enumerate() {
            if let Some(c) = timer.tick(now, pumps[i]) {
                cmds.push(c);
            }
        }
        cmds
    }

    pub fn start_timer(&mut self, pump_index: u8, minutes: u32, now: Timestamp) -> Option<Command> {
        let i = (pump_index as usize).checked_sub(1)?;
        if i >= self.timers.len() {
            return None;
        }
        Some(self.timers[i].start_with_minutes(minutes, now))
    }

    /// Cancel all running pump timers (e.g., on bus reset).
    pub fn cancel_all(&mut self) {
        for timer in &mut self.timers {
            timer.cancel();
        }
    }
}

/// Hold mode safety timer. Automatically releases hold mode after a timeout period.
pub struct HoldModeTimer {
    entered_at: Option<Timestamp>,
    timeout_ms: u64,
    /// True after timer fires; prevents re-arming until hold mode is released.
    fired: bool,
}

impl Default for HoldModeTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl HoldModeTimer {
    pub fn new() -> Self {
        HoldModeTimer {
            entered_at: None,
            timeout_ms: DEFAULT_HOLD_MODE_TIMEOUT_MS,
            fired: false,
        }
    }

    pub fn with_timeout_ms(timeout_ms: u64) -> Self {
        HoldModeTimer {
            entered_at: None,
            timeout_ms,
            fired: false,
        }
    }

    pub fn tick(&mut self, now: Timestamp, is_hold: bool) -> Option<Command> {
        if is_hold {
            if self.fired {
                // Already fired — wait for hold mode to be released before re-arming.
                return None;
            }
            if let Some(entered_at) = self.entered_at {
                if now.elapsed_since(entered_at) >= self.timeout_ms {
                    self.entered_at = None;
                    self.fired = true;
                    return Some(Command::ToggleItem(ToggleItem::HoldMode));
                }
            } else {
                self.entered_at = Some(now);
            }
        } else {
            self.entered_at = None;
            self.fired = false;
        }
        None
    }

    pub fn is_active(&self) -> bool {
        self.entered_at.is_some()
    }

    pub fn remaining_ms(&self, now: Timestamp) -> u64 {
        if let Some(entered_at) = self.entered_at {
            self.timeout_ms
                .saturating_sub(now.elapsed_since(entered_at))
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use launa_hal::Clock;
    use launa_sim::VirtualClock;

    /// VAL-BM-009: PumpTimer::tick() with !is_on cancels timer and doesn't re-fire.
    /// Start the timer, feed an Off status (pump not running), verify:
    /// 1. Timer returns None on the tick where !is_on is detected
    /// 2. Timer is no longer running after cancellation
    /// 3. Subsequent ticks with past-duration elapsed time do NOT fire
    #[test]
    fn test_pump_timer_cancel_on_pump_off() {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let mut timer = PumpTimer::new(ToggleItem::Pump1);

        // Start timer for 1 minute
        let now = clock.now();
        let cmd = timer.start(now);
        assert!(matches!(cmd, Command::ToggleItem(ToggleItem::Pump1)));
        assert!(timer.is_running());

        // Advance past the timer duration
        clock.advance_ms(61_000);
        let later = clock.now();

        // Pump is OFF — timer should cancel and return None
        let result = timer.tick(later, PumpState::Off);
        assert!(
            result.is_none(),
            "tick with !is_on should return None, not a toggle command"
        );
        assert!(
            !timer.is_running(),
            "timer should not be running after cancellation"
        );

        // Advance well past the duration — should NOT re-fire
        clock.advance_ms(120_000);
        let much_later = clock.now();
        let result2 = timer.tick(much_later, PumpState::Off);
        assert!(
            result2.is_none(),
            "cancelled timer should never re-fire, even after duration passes"
        );
        assert!(!timer.is_running());
    }

    /// VAL-BM-009 extended: After cancellation, restarting the timer works correctly
    /// and fires at the new duration.
    #[test]
    fn test_pump_timer_cancel_then_restart() {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let mut timer = PumpTimer::new(ToggleItem::Pump1);

        // Start timer for 1 minute
        let now = clock.now();
        timer.start_with_minutes(1, now);
        assert!(timer.is_running());

        // Cancel by feeding Off
        clock.advance_ms(30_000);
        let result = timer.tick(clock.now(), PumpState::Off);
        assert!(result.is_none());
        assert!(!timer.is_running());

        // Restart the timer for 1 minute
        clock.advance_ms(10_000);
        let restart_time = clock.now();
        timer.start_with_minutes(1, restart_time);
        assert!(timer.is_running());

        // Feed pump running status — timer is running but not expired yet
        let result = timer.tick(clock.now(), PumpState::Low);
        assert!(result.is_none(), "timer should not fire before duration");

        // Advance past 1 minute duration
        clock.advance_ms(61_000);
        let result = timer.tick(clock.now(), PumpState::Low);
        assert!(
            result.is_some(),
            "restarted timer should fire at new duration"
        );
        assert!(matches!(
            result,
            Some(Command::ToggleItem(ToggleItem::Pump1))
        ));
        assert!(!timer.is_running());
    }
}
