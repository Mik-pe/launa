//! On-device self-test simulator backed by SpaSim.
//!
//! When self-test mode is enabled via MQTT (`launa_spa/command/self_test`),
//! this module wraps a `SpaSim` instance from `launa-sim`. Commands are
//! applied directly to the simulator's state where possible (toggles,
//! temperature, scale, time) to avoid the overhead of frame encode/decode
//! roundtrips on the ESP32's limited heap. Status is read back by parsing
//! the simulator's output frames through `StatusUpdate::parse()`, so all
//! state — `is_heating`, pump states, temperatures, etc. — is derived from
//! the same physics model used in integration tests.
//!
//! Physics ticking is throttled (every N loop iterations) so that heating
//! is visible over multiple publish cycles rather than converging instantly.

use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::frame::{FrameDecoder, FrameEncoder};
use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale, StatusUpdate};
use launa_sim::SpaSim;

/// Only tick the physics model every N loop iterations.
/// At ~0.2°C/tick and 1 tick/sec this gives ~1°C per 5 seconds.
const PHYSICS_TICK_DIVISOR: u64 = 1;

/// Self-test state backed by a full SpaSim instance.
///
/// Commands are sent to the simulator as wire frames; status is read
/// back by parsing the simulator's output through the protocol decoder.
pub(crate) struct SelfTestState {
    sim: SpaSim,
    cached_status: Option<StatusUpdate>,
    loop_count: u64,
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
            loop_count: 0,
        }
    }

    /// Apply a command to the simulated spa.
    ///
    /// For simple commands (toggles, set-temp, set-scale, set-time), the
    /// mutation is applied directly to `sim.state` to avoid frame encode/
    /// decode allocations. Other commands fall through to the frame roundtrip.
    /// Returns `true` if the command was handled.
    pub fn apply_command(&mut self, cmd: &Command) -> bool {
        match cmd {
            Command::SelfTest(_) | Command::Sniff(_) => return false,
            Command::ToggleItem(item) => {
                self.apply_toggle(*item);
                return true;
            }
            Command::SetTemperature(raw_temp) => {
                self.apply_set_temperature(*raw_temp);
                return true;
            }
            Command::SetTemperatureScale(celsius) => {
                self.apply_set_temperature_scale(*celsius);
                return true;
            }
            Command::SetTime { hour, minute, .. } => {
                self.sim.state.hour = *hour;
                self.sim.state.minute = *minute;
                return true;
            }
            _ => {}
        }

        // Fallthrough: frame encode/decode roundtrip for complex commands
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

    /// Advance the simulator by one loop iteration.
    ///
    /// Physics is only ticked every `PHYSICS_TICK_DIVISOR` iterations to slow
    /// heating to a viewable rate. Status is regenerated every loop so the
    /// web UI always gets fresh state.
    pub fn tick(&mut self) {
        self.loop_count += 1;

        if self.loop_count % PHYSICS_TICK_DIVISOR == 0 {
            // Full physics tick
            let output = self.sim.tick();
            self.cached_status = parse_status_from_bytes(&output).or(self.cached_status.take());
        } else {
            // Regenerate status without ticking physics (keeps web UI responsive)
            let output = self.sim.generate_status_frame();
            self.cached_status = parse_status_from_bytes(&output).or(self.cached_status.take());
        }
    }

    /// Get the last known status (from the most recent `tick()`).
    pub fn status(&self) -> &StatusUpdate {
        self.cached_status
            .as_ref()
            .expect("tick() must be called before status()")
    }

    // -- Direct state mutation helpers (avoid frame roundtrip allocations) --

    fn apply_toggle(&mut self, item: ToggleItem) {
        let code = item.code();
        match item.pump_index() {
            Some(idx) => {
                self.sim.state.pumps[idx] = cycle_pump(self.sim.state.pumps[idx]);
            }
            None => match item {
                ToggleItem::Blower => {
                    self.sim.state.blower = !self.sim.state.blower;
                }
                ToggleItem::Light1 | ToggleItem::Light2 => {
                    if let Some(idx) = item.light_index() {
                        if idx < 2 {
                            self.sim.state.lights[idx] = !self.sim.state.lights[idx];
                        }
                    }
                }
                ToggleItem::Mister => {
                    self.sim.state.mister = !self.sim.state.mister;
                }
                ToggleItem::HoldMode => {
                    self.sim.state.hold = !self.sim.state.hold;
                }
                ToggleItem::HeatingMode => {
                    self.sim.state.heating_mode = cycle_heating_mode(self.sim.state.heating_mode);
                }
                ToggleItem::TemperatureRange => {
                    // Save current set_temp to the active range before switching
                    match self.sim.state.temp_range {
                        TempRange::High => self.sim.state.set_temp_high = self.sim.state.set_temp,
                        TempRange::Low => self.sim.state.set_temp_low = self.sim.state.set_temp,
                        _ => {}
                    }
                    self.sim.state.temp_range = flip_temp_range(self.sim.state.temp_range);
                    // Restore set_temp from the new range's saved value
                    self.sim.state.set_temp = match self.sim.state.temp_range {
                        TempRange::High => self.sim.state.set_temp_high,
                        TempRange::Low => self.sim.state.set_temp_low,
                        _ => self.sim.state.set_temp,
                    };
                }
                ToggleItem::CirculationPump => {
                    self.sim.state.circ_pump = !self.sim.state.circ_pump;
                }
                // Light3, Light4, Aux1, Aux2, SoakMode, NormalOperation,
                // ClearNotification and any future items are not wired in
                // the simulator state — silently ignore.
                _ => {
                    let _ = code;
                }
            },
        }
    }

    fn apply_set_temperature(&mut self, raw_temp: u8) {
        // Decode raw wire value to real units: Fahrenheit direct, Celsius /2
        let real_temp = match self.sim.state.temp_scale {
            TemperatureScale::Celsius => raw_temp as f32 / 2.0,
            _ => raw_temp as f32,
        };
        // Directly mutate state (same as SpaState::set_target_temp)
        self.sim.state.set_temp = real_temp;
        match self.sim.state.temp_range {
            TempRange::High => self.sim.state.set_temp_high = real_temp,
            TempRange::Low => self.sim.state.set_temp_low = real_temp,
            _ => {}
        }
    }

    fn apply_set_temperature_scale(&mut self, celsius: bool) {
        self.sim.state.temp_scale = if celsius {
            TemperatureScale::Celsius
        } else {
            TemperatureScale::Fahrenheit
        };
    }
}

// -- Helper functions (mirrors frame_gen logic, but callable from app) --

fn cycle_pump(state: PumpState) -> PumpState {
    match state {
        PumpState::Off => PumpState::Low,
        PumpState::Low => PumpState::High,
        PumpState::High => PumpState::Off,
        _ => PumpState::Off,
    }
}

fn cycle_heating_mode(mode: HeatingMode) -> HeatingMode {
    match mode {
        HeatingMode::Ready => HeatingMode::Rest,
        HeatingMode::Rest => HeatingMode::ReadyInRest,
        HeatingMode::ReadyInRest => HeatingMode::Ready,
        _ => HeatingMode::Ready,
    }
}

fn flip_temp_range(range: TempRange) -> TempRange {
    match range {
        TempRange::High => TempRange::Low,
        TempRange::Low => TempRange::High,
        _ => TempRange::High,
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
