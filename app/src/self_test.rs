//! On-device self-test simulator backed by SpaSim.
//!
//! When self-test mode is enabled via MQTT (`launa_spa/command/self_test`),
//! this module wraps a `SpaSim` instance from `launa-sim`. All commands
//! are fed through the simulator's existing frame processing pipeline
//! (encode → process_frame) so behaviour is identical to integration tests.

use launa_protocol::command::Command;
use launa_protocol::frame::{FrameDecoder, FrameEncoder};
use launa_protocol::status::{TemperatureScale, StatusUpdate};
use launa_protocol::Temperature;
use launa_sim::SpaSim;

/// Self-test state backed by a full SpaSim instance.
pub(crate) struct SelfTestState {
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
