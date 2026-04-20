//! On-device self-test simulator backed by SpaSim.
//!
//! When self-test mode is enabled via MQTT (`launa_spa/command/self_test`),
//! this module wraps a `SpaSim` instance from `launa-sim`. Commands are
//! encoded to wire frames and fed to the simulator exactly as the real spa
//! would receive them. Status is read back by parsing the simulator's output
//! frames through `StatusUpdate::parse()`, so all state — `is_heating`,
//! pump states, temperatures, etc. — is derived from the same physics model
//! used in integration tests.

use launa_protocol::command::Command;
use launa_protocol::frame::{FrameDecoder, FrameEncoder};
use launa_protocol::status::{StatusUpdate, TemperatureScale};
use launa_sim::SpaSim;

/// Self-test state backed by a full SpaSim instance.
///
/// Commands are sent to the simulator as wire frames; status is read
/// back by parsing the simulator's output through the protocol decoder.
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
        // Default SpaSim state is Fahrenheit 100°F / 104°F set point.
        // Switch to Celsius for a more sensible default.
        sim.state.temp_scale = TemperatureScale::Celsius;
        sim.state.current_temp = 37.5;
        sim.state.set_temp = 38.0;
        sim.state.set_temp_high = 38.0;

        // Enable circ pump so the heater interlock is satisfied
        // (the physics model requires at least one pump running for heating).
        sim.state.circ_pump = true;

        SelfTestState {
            sim,
            cached_status: None,
        }
    }

    /// Apply a command to the simulated spa.
    ///
    /// The command is encoded to a wire frame and fed to the simulator's
    /// `process_frame()`, exactly as if it arrived over RS-485. Returns
    /// `true` if the command was forwarded to the simulator.
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

    /// Advance the simulator by one tick, run physics, and parse the status.
    pub fn tick(&mut self) {
        let output = self.sim.tick();
        self.cached_status = parse_status_from_bytes(&output).or(self.cached_status.take());
    }

    /// Get the last known status (from the most recent `tick()`).
    pub fn status(&self) -> &StatusUpdate {
        self.cached_status
            .as_ref()
            .expect("tick() must be called before status()")
    }
}

/// Parse the first valid status frame from a raw byte stream.
fn parse_status_from_bytes(bytes: &[u8]) -> Option<StatusUpdate> {
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(bytes);
    for frame in &frames {
        if frame.message_type == [0xFF, 0xAF] && frame.payload.len() == 24 {
            if let Ok(status) = StatusUpdate::parse(&frame.payload) {
                return Some(status);
            }
        }
    }
    None
}
