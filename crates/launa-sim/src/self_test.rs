//! Self-test simulator backed by SpaSim.
//!
//! When self-test mode is enabled via MQTT (`launa_spa/command/self_test`),
//! this module wraps a `SpaSim` instance. All commands are fed through
//! the simulator's existing frame processing pipeline (encode → process_frame)
//! so behaviour is identical to integration tests.

use launa_protocol::command::Command;
use launa_protocol::frame::{FrameDecoder, FrameEncoder};
use launa_protocol::status::{StatusUpdate, TemperatureScale};
use launa_protocol::Temperature;

use crate::SpaSim;

/// Self-test state backed by a full SpaSim instance.
pub struct SelfTestState {
    sim: SpaSim,
    cached_status: Option<StatusUpdate>,
}

impl SelfTestState {
    /// Create a new self-test state.
    ///
    /// The simulator starts in Celsius mode (37.5°C water, 38°C set point)
    /// with heating active and all pumps/lights/accessories off. The circ
    /// pump is enabled to satisfy the heater interlock.
    pub fn new() -> Self {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Celsius;
        sim.state.current_temp = Temperature::celsius(37.5);
        sim.state.set_temp = Temperature::celsius(38.0);
        sim.state.set_temp_high = Temperature::celsius(38.0);
        sim.state.set_temp_low = Temperature::celsius(20.0);
        sim.state.circ_pump = true;

        SelfTestState {
            sim,
            cached_status: None,
        }
    }

    /// Apply a command by encoding it as a wire frame and feeding it
    /// through the simulator's process_frame. Returns `true` if handled.
    pub fn apply_command(&mut self, cmd: &Command) -> bool {
        match cmd {
            Command::SelfTest(_) | Command::Sniff(_) => return false,
            _ => {}
        }

        let (msg_type, payload) = cmd.encode();
        if let Ok(frame_bytes) = FrameEncoder::encode(msg_type, &payload) {
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&frame_bytes);
            if let Some(frame) = frames.first() {
                self.sim.process_frame(frame);
                return true;
            }
        }
        false
    }

    /// Advance the simulator by one physics tick and cache the resulting status.
    pub fn tick(&mut self) {
        self.cached_status = self.sim.tick_status().or(self.cached_status.take());
    }

    /// Get the last known status (from the most recent `tick()`).
    pub fn status(&self) -> &StatusUpdate {
        self.cached_status
            .as_ref()
            .expect("tick() must be called before status()")
    }
}

#[cfg(feature = "std")]
mod tests {
    use super::*;
    use launa_protocol::command::ToggleItem;
    use launa_protocol::status::PumpState;

    #[test]
    fn test_initial_state_after_tick() {
        let mut st = SelfTestState::new();
        st.tick();
        let status = st.status();

        assert_eq!(
            status.current_temp,
            Some(Temperature::celsius(37.5)),
            "initial current_temp should be 37.5°C"
        );
        assert_eq!(
            status.set_temp,
            Temperature::celsius(38.0),
            "initial set_temp should be 38.0°C"
        );
        assert!(
            status.circ_pump,
            "circ_pump should be on to satisfy heater interlock"
        );
    }

    #[test]
    fn test_apply_toggle_pump_command() {
        let mut st = SelfTestState::new();

        // Toggle pump 1 on — should be handled
        let handled = st.apply_command(&Command::ToggleItem(ToggleItem::Pump1));
        assert!(handled, "toggle pump1 should be handled");

        // After applying, tick to get updated status
        st.tick();
        let status = st.status();
        assert_ne!(
            status.pumps[0],
            PumpState::Off,
            "pump1 should be on after toggle"
        );
    }

    #[test]
    fn test_self_test_command_not_handled() {
        let mut st = SelfTestState::new();
        let handled = st.apply_command(&Command::SelfTest(true));
        assert!(!handled, "SelfTest command should not be handled");
    }

    #[test]
    fn test_sniff_command_not_handled() {
        let mut st = SelfTestState::new();
        let handled = st.apply_command(&Command::Sniff(true));
        assert!(!handled, "Sniff command should not be handled");
    }

    #[test]
    fn test_tick_produces_status() {
        let mut st = SelfTestState::new();
        st.tick();
        // After tick, status should return a valid StatusUpdate
        let status = st.status();
        assert_eq!(
            status.temperature_scale,
            TemperatureScale::Celsius,
            "status should report Celsius"
        );
    }
}
