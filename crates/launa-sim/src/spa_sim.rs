//! Simulated Balboa BP6013G1 spa mainboard.
//!
//! Generates realistic RS-485 byte streams identical to what a real spa controller
//! would send. Processes incoming commands and updates internal state accordingly.
//! Designed to be connected to a `SpaController` via a `SimTransport`.
//!
//! The sim uses Rust types natively (enums, f32 temps) and only converts to raw
//! bytes at the frame generation boundary.

use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
use launa_protocol::status::{HeatingMode, TemperatureScale, TempRange, PumpState};

/// Simulated spa state using native Rust types.
///
/// All values are in real units (f32 temperatures, proper enums). Conversion
/// to the wire format happens only in `generate_status_frame()`.
#[derive(Debug, Clone)]
pub struct SpaState {
    /// Current water temperature in real units (°F or °C).
    pub current_temp: f32,
    /// Target temperature in real units.
    pub set_temp: f32,
    /// Active heating mode.
    pub heating_mode: HeatingMode,
    /// Temperature scale (affects wire encoding).
    pub temp_scale: TemperatureScale,
    /// Whether the heater element is currently active.
    pub is_heating: bool,
    /// Temperature range (high/low).
    pub temp_range: TempRange,
    /// Pump 1 state.
    pub pump1: PumpState,
    /// Pump 2 state.
    pub pump2: PumpState,
    /// Pump 3 state.
    pub pump3: PumpState,
    /// Circulation pump on/off.
    pub circ_pump: bool,
    /// Blower on/off.
    pub blower: bool,
    /// Light on/off.
    pub light1: bool,
    /// Mister on/off.
    pub mister: bool,
    /// Clock hour (0-23).
    pub hour: u8,
    /// Clock minute (0-59).
    pub minute: u8,
    /// Whether the spa is in priming mode.
    pub priming: bool,
    /// Whether the spa is in hold mode.
    pub hold: bool,
}

impl Default for SpaState {
    fn default() -> Self {
        SpaState {
            current_temp: 100.0,
            set_temp: 104.0,
            heating_mode: HeatingMode::Ready,
            temp_scale: TemperatureScale::Fahrenheit,
            is_heating: true,
            temp_range: TempRange::High,
            pump1: PumpState::Off,
            pump2: PumpState::Off,
            pump3: PumpState::Off,
            circ_pump: false,
            blower: false,
            light1: false,
            mister: false,
            hour: 14,
            minute: 30,
            priming: false,
            hold: false,
        }
    }
}

impl SpaState {
    /// Encode a temperature to the raw wire value.
    /// Fahrenheit: direct. Celsius: multiply by 2.
    fn encode_temp(temp: f32, scale: TemperatureScale) -> u8 {
        let raw = match scale {
            TemperatureScale::Fahrenheit => temp,
            TemperatureScale::Celsius => temp * 2.0,
        };
        raw.round() as u8
    }

    /// Decode a raw wire temperature to real units.
    fn decode_temp(raw: u8, scale: TemperatureScale) -> f32 {
        match scale {
            TemperatureScale::Fahrenheit => raw as f32,
            TemperatureScale::Celsius => raw as f32 / 2.0,
        }
    }
}

/// Simulated Balboa BP6013G1 spa mainboard.
///
/// Connects to a `SimTransport` and simulates the real spa's behavior:
/// - Sends periodic status updates (~1 per second via `tick()`)
/// - Sends registration queries until a client registers
/// - Sends `Ready` messages after each status update
/// - Processes incoming command frames and updates internal state
pub struct SpaSim {
    pub state: SpaState,
    pub client_id: Option<u8>,
    next_client_id: u8,
    tick_count: u64,
    registered: bool,
}

impl SpaSim {
    pub fn new() -> Self {
        SpaSim {
            state: SpaState::default(),
            client_id: None,
            next_client_id: 0x02,
            tick_count: 0,
            registered: false,
        }
    }

    /// Advance simulated time by 1 second.
    ///
    /// Returns the raw bytes the spa would transmit this second:
    /// - If not registered: a registration query (`FE BF 00`)
    /// - Always: a status update frame (`FF AF 13` with 24-byte payload)
    /// - Always: a `Ready` frame (`10 BF 06`) indicating the bus is free
    pub fn tick(&mut self) -> Vec<u8> {
        self.tick_count += 1;
        let mut output = Vec::new();

        // Send registration query if no client is registered
        if !self.registered {
            output.extend_from_slice(&self.generate_registration_query());
        }

        // Update physical simulation
        self.simulate_physics();

        // Send status update
        output.extend_from_slice(&self.generate_status_frame());

        // Send ready indicator (bus is free for commands)
        output.extend_from_slice(&self.generate_ready_frame());

        output
    }

    /// Process all bytes the controller has written to the transport.
    ///
    /// Parses them as frames and handles each one (toggles, temp changes,
    /// registration responses, etc.). Updates internal state accordingly.
    /// Returns any response frames the spa would send back.
    pub fn process_incoming_bytes(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(bytes);
        let mut responses = Vec::new();

        for frame in &frames {
            if let Some(response) = self.process_frame(frame) {
                responses.extend_from_slice(&response);
            }
        }

        responses
    }

    /// Process a single parsed frame from the controller.
    pub fn process_frame(&mut self, frame: &Frame) -> Option<Vec<u8>> {
        match frame.message_type {
            [0x0A, 0xBF] => {
                if frame.payload.is_empty() {
                    return None;
                }
                match frame.payload[0] {
                    0x04 => Some(self.generate_config_response()),
                    0x11 => {
                        if frame.payload.len() >= 2 {
                            self.handle_toggle_by_code(frame.payload[1]);
                        }
                        None
                    }
                    0x20 => {
                        if frame.payload.len() >= 2 {
                            let raw_temp = frame.payload[1];
                            self.state.set_temp = SpaState::decode_temp(raw_temp, self.state.temp_scale);
                        }
                        None
                    }
                    0x22 => {
                        if frame.payload.len() >= 2 {
                            match frame.payload[1] {
                                0x01 => Some(self.generate_filter_cycles_response()),
                                0x02 => Some(self.generate_information_response()),
                                0x20 => Some(self.generate_fault_log_response()),
                                _ => None,
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            [0xFE, 0xBF] => {
                if frame.payload.len() >= 1 && frame.payload[0] == 0x01 {
                    let id = self.next_client_id;
                    self.next_client_id += 1;
                    Some(self.generate_client_id_assignment(id))
                } else {
                    None
                }
            }
            _ => {
                // Client ID ack: <ID> BF 03
                if frame.payload.len() >= 1
                    && frame.message_type[1] == 0xBF
                    && frame.payload[0] == 0x03
                {
                    self.client_id = Some(frame.message_type[0]);
                    self.registered = true;
                }
                None
            }
        }
    }

    /// Simulate temperature changes and time progression.
    fn simulate_physics(&mut self) {
        // Temperature approach: ±1° per tick in real units
        if self.state.current_temp < self.state.set_temp && self.state.is_heating {
            self.state.current_temp = (self.state.current_temp + 1.0).min(self.state.set_temp);
        } else if self.state.current_temp > self.state.set_temp {
            self.state.current_temp = (self.state.current_temp - 1.0).max(self.state.set_temp);
        } else if self.state.current_temp == self.state.set_temp {
            if self.state.is_heating {
                self.state.is_heating = false;
            }
        }

        // Advance clock
        self.state.minute += 1;
        if self.state.minute >= 60 {
            self.state.minute = 0;
            self.state.hour = (self.state.hour + 1) % 24;
        }
    }

    /// Generate a complete framed status update.
    ///
    /// This is the boundary where Rust types → raw wire bytes.
    pub fn generate_status_frame(&self) -> Vec<u8> {
        let mut payload = [0u8; 24];

        // Offset 0: Spa State (0x00=Running, 0x05=Hold)
        if self.state.hold {
            payload[0] = 0x05;
        }
        // Offset 1: Init Mode (0x00=Idle, 0x01=Priming)
        if self.state.priming {
            payload[1] = 0x01;
        }
        // Offset 2: Current Temperature
        payload[2] = SpaState::encode_temp(self.state.current_temp, self.state.temp_scale);
        // Offset 3: Hour, Offset 4: Minute
        payload[3] = self.state.hour;
        payload[4] = self.state.minute;

        // Offset 5: Heating Mode (0=Ready, 1=Rest, 3=Ready-in-Rest)
        payload[5] |= match self.state.heating_mode {
            HeatingMode::Ready => 0,
            HeatingMode::Rest => 1,
            HeatingMode::ReadyInRest => 3,
        };

        // Offset 9: Flags (temp scale bit 0, 24h time bit 1, filter mode bits 2-3)
        if matches!(self.state.temp_scale, TemperatureScale::Celsius) {
            payload[9] |= 0x01;
        }
        payload[9] |= 0x02; // 24h format

        // Offset 10: Flags (temp range bit 2, heating state bits 4-5)
        if self.state.is_heating {
            payload[10] |= 0x30;
        }
        if matches!(self.state.temp_range, TempRange::High) {
            payload[10] |= 0x04;
        }

        // Offset 11: Pumps 1-4 (2 bits each)
        payload[11] = pump_state_to_bits(self.state.pump1)
            | (pump_state_to_bits(self.state.pump2) << 2)
            | (pump_state_to_bits(self.state.pump3) << 4);

        // Offset 13: Circ pump (bit 1), Blower (bits 2-3)
        if self.state.circ_pump {
            payload[13] |= 0x02;
        }
        if self.state.blower {
            payload[13] |= 0x0C;
        }
        // Offset 14: Lights (bits 0-1 = Light1)
        if self.state.light1 {
            payload[14] |= 0x03;
        }
        // Offset 15: Mister (0=off, 1=on)
        if self.state.mister {
            payload[15] = 0x01;
        }

        // Offset 20: Set Temperature
        payload[20] = SpaState::encode_temp(self.state.set_temp, self.state.temp_scale);

        FrameEncoder::encode([0xFF, 0xAF], &payload)
    }

    /// Generate a `Ready` frame (`10 BF 06`).
    pub fn generate_ready_frame(&self) -> Vec<u8> {
        FrameEncoder::encode([0x10, 0xBF], &[0x06])
    }

    /// Generate a registration query (`FE BF 00`).
    pub fn generate_registration_query(&self) -> Vec<u8> {
        FrameEncoder::encode([0xFE, 0xBF], &[0x00])
    }

    /// Generate a client ID assignment (`FE BF 02 <ID>`).
    fn generate_client_id_assignment(&self, id: u8) -> Vec<u8> {
        FrameEncoder::encode([0xFE, 0xBF], &[0x02, id])
    }

    /// Generate a configuration response.
    pub fn generate_config_response(&self) -> Vec<u8> {
        let mut config_payload = [0u8; 10];
        config_payload[0] = 0x02;
        config_payload[1] = 0x02;
        if matches!(self.state.temp_scale, TemperatureScale::Celsius) {
            config_payload[3] |= 0x01;
        }
        config_payload[5] = 0x02 | (0x02 << 2);
        config_payload[7] |= 0x01;
        config_payload[8] |= 0x80;
        config_payload[8] |= 0x01;

        let mut full_payload = vec![0x2E];
        full_payload.extend_from_slice(&config_payload);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate an information response.
    pub fn generate_information_response(&self) -> Vec<u8> {
        let mut info_data = [0u8; 21];
        info_data[0] = 0x64;
        info_data[1] = 0xDC;
        info_data[2] = 0x11;
        info_data[3] = 0x00;
        let model: &[u8] = b"BFBP20  ";
        info_data[4..12].copy_from_slice(model);
        info_data[12] = 0x01;
        info_data[13] = 0x3D;
        info_data[14] = 0x12;
        info_data[15] = 0x38;
        info_data[16] = 0x2E;
        info_data[17] = 0x01;
        info_data[18] = 0x0A;
        info_data[19] = 0x04;
        info_data[20] = 0x00;

        let mut full_payload = vec![0x24];
        full_payload.extend_from_slice(&info_data);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate a fault log response.
    pub fn generate_fault_log_response(&self) -> Vec<u8> {
        let fault_data: [u8; 10] = [
            0x03, 0x01, 0x1B, 0x02, 0x0E, 0x1E, 0x04, 0x68, 0x68, 0x66,
        ];
        let mut full_payload = vec![0x28];
        full_payload.extend_from_slice(&fault_data);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate a filter cycles response.
    pub fn generate_filter_cycles_response(&self) -> Vec<u8> {
        let filter_data: [u8; 8] = [
            0x08, 0x00, 0x04, 0x00,
            0x90, 0x00, 0x02, 0x00,
        ];
        let mut full_payload = vec![0x23];
        full_payload.extend_from_slice(&filter_data);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Handle a toggle command by raw protocol item code.
    ///
    /// This is the boundary where raw bytes → Rust type mutations.
    fn handle_toggle_by_code(&mut self, item_code: u8) {
        match item_code {
            0x04 => self.state.pump1 = cycle_pump(self.state.pump1),
            0x05 => self.state.pump2 = cycle_pump(self.state.pump2),
            0x06 => self.state.pump3 = cycle_pump(self.state.pump3),
            0x0C => self.state.blower = !self.state.blower,
            0x11 => self.state.light1 = !self.state.light1,
            0x3C => self.state.hold = !self.state.hold,
            0x51 => self.state.heating_mode = cycle_heating_mode(self.state.heating_mode),
            0x50 => self.state.temp_range = flip_temp_range(self.state.temp_range),
            _ => {}
        }
    }

    /// How many ticks have elapsed.
    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }
}

// -- Helper functions for type-safe state transitions --

fn pump_state_to_bits(state: PumpState) -> u8 {
    match state {
        PumpState::Off => 0,
        PumpState::Low => 1,
        PumpState::High => 2,
    }
}

fn cycle_pump(state: PumpState) -> PumpState {
    match state {
        PumpState::Off => PumpState::Low,
        PumpState::Low => PumpState::High,
        PumpState::High => PumpState::Off,
    }
}

fn cycle_heating_mode(mode: HeatingMode) -> HeatingMode {
    match mode {
        HeatingMode::Ready => HeatingMode::Rest,
        HeatingMode::Rest => HeatingMode::ReadyInRest,
        HeatingMode::ReadyInRest => HeatingMode::Ready,
    }
}

fn flip_temp_range(range: TempRange) -> TempRange {
    match range {
        TempRange::High => TempRange::Low,
        TempRange::Low => TempRange::High,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tick_generates_frames() {
        let mut sim = SpaSim::new();
        let bytes = sim.tick();
        assert!(!bytes.is_empty(), "tick should produce output bytes");

        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert!(frames.len() >= 2, "tick should produce at least 2 frames (status + ready)");
    }

    #[test]
    fn test_tick_after_registration_no_query() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].message_type, [0xFF, 0xAF]); // status
        assert_eq!(frames[1].message_type, [0x10, 0xBF]); // ready
    }

    #[test]
    fn test_physics_heating() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 95.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = true;

        for _ in 0..5 {
            sim.simulate_physics();
        }
        assert_eq!(sim.state.current_temp, 100.0);
        assert!(sim.state.is_heating, "still heating on the tick that reaches target");

        sim.simulate_physics();
        assert!(!sim.state.is_heating, "should stop heating once at set temp");
    }

    #[test]
    fn test_physics_cooling() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 105.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;

        sim.simulate_physics();
        assert_eq!(sim.state.current_temp, 104.0);
    }

    #[test]
    fn test_process_toggle_via_bytes() {
        let mut sim = SpaSim::new();

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1
        ).encode();
        let encoded = FrameEncoder::encode(mt, &payload);

        sim.process_incoming_bytes(&encoded);
        assert_eq!(sim.state.pump1, PumpState::Low);
    }

    #[test]
    fn test_cycle_heating_mode_enums() {
        assert_eq!(cycle_heating_mode(HeatingMode::Ready), HeatingMode::Rest);
        assert_eq!(cycle_heating_mode(HeatingMode::Rest), HeatingMode::ReadyInRest);
        assert_eq!(cycle_heating_mode(HeatingMode::ReadyInRest), HeatingMode::Ready);
    }

    #[test]
    fn test_cycle_pump_enums() {
        assert_eq!(cycle_pump(PumpState::Off), PumpState::Low);
        assert_eq!(cycle_pump(PumpState::Low), PumpState::High);
        assert_eq!(cycle_pump(PumpState::High), PumpState::Off);
    }

    #[test]
    fn test_temp_encoding_fahrenheit() {
        assert_eq!(SpaState::encode_temp(100.0, TemperatureScale::Fahrenheit), 100);
        assert_eq!(SpaState::encode_temp(104.0, TemperatureScale::Fahrenheit), 104);
    }

    #[test]
    fn test_temp_encoding_celsius() {
        assert_eq!(SpaState::encode_temp(38.0, TemperatureScale::Celsius), 76);
        assert_eq!(SpaState::encode_temp(40.0, TemperatureScale::Celsius), 80);
    }

    #[test]
    fn test_set_temp_decoded_from_wire() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Fahrenheit;

        // Simulate a SetTemperature(100) command coming in
        let (mt, payload) = launa_protocol::command::Command::SetTemperature(100).encode();
        let encoded = FrameEncoder::encode(mt, &payload);
        sim.process_incoming_bytes(&encoded);

        assert_eq!(sim.state.set_temp, 100.0);
    }

    #[test]
    fn test_set_temp_decoded_celsius() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Celsius;

        // SetTemperature sends raw value 80 (= 40°C on wire)
        let (mt, payload) = launa_protocol::command::Command::SetTemperature(80).encode();
        let encoded = FrameEncoder::encode(mt, &payload);
        sim.process_incoming_bytes(&encoded);

        assert_eq!(sim.state.set_temp, 40.0);
    }
}
