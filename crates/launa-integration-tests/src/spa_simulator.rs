//! Mock Balboa spa mainboard simulator for integration testing.
//!
//! Simulates what the real BP6013G1 would send over RS-485 so we can
//! test the full pipeline on desktop.

use launa_protocol::frame::{Frame, FrameEncoder};

/// Simulated spa state.
#[derive(Debug, Clone)]
pub struct SpaState {
    pub current_temp: u8,      // raw value (Fahrenheit direct, Celsius halved)
    pub set_temp: u8,
    pub heating_mode: u8,      // 0=Ready, 1=Rest, 3=Ready-in-Rest
    pub temp_scale_celsius: bool,
    pub is_heating: bool,
    pub temp_range_high: bool,
    pub pump1: u8,             // 0=off, 1=low, 2=high
    pub pump2: u8,
    pub pump3: u8,
    pub circ_pump: bool,
    pub blower: bool,
    pub light1: bool,
    pub mister: bool,
    pub hour: u8,
    pub minute: u8,
    pub priming: bool,
    pub hold: bool,
}

impl Default for SpaState {
    fn default() -> Self {
        SpaState {
            current_temp: 100,     // 100°F
            set_temp: 104,         // 104°F
            heating_mode: 0,       // Ready
            temp_scale_celsius: false,
            is_heating: true,
            temp_range_high: true,
            pump1: 0,
            pump2: 0,
            pump3: 0,
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

/// Mock Balboa BP6013G1 spa simulator.
pub struct SpaSimulator {
    pub state: SpaState,
    pub client_id: Option<u8>,
    pub next_client_id: u8,
}

impl SpaSimulator {
    /// Create a new simulator with realistic defaults.
    pub fn new() -> Self {
        SpaSimulator {
            state: SpaState::default(),
            client_id: None,
            next_client_id: 0x02,
        }
    }

    /// Generate a complete framed status update (message type `FF AF 13`).
    ///
    /// The status payload is 24 bytes with layout:
    /// ```text
    ///  0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15 16 17 18 19 20 21 22 23
    /// F0 F1 CT HH MM F2 -- -- -- F3 F4 PP -- F5 LF F6 -- -- -- -- ST -- -- --
    /// ```
    pub fn generate_status_frame(&self) -> Vec<u8> {
        let mut payload = [0u8; 24];

        // Offset 0: F0 marker (unused flag byte)
        payload[0] = 0xF0;
        // Offset 1: F1 marker
        payload[1] = 0xF1;

        // Offset 2: current temperature
        payload[2] = self.state.current_temp;

        // Offset 3: hour
        payload[3] = self.state.hour;
        // Offset 4: minute
        payload[4] = self.state.minute;

        // Offset 5: hold flag
        if self.state.hold {
            payload[5] |= 0x05;
        }

        // Offset 6: priming flag
        if self.state.priming {
            payload[6] |= 0x01;
        }

        // Offset 7: heating mode (bits 0-1)
        payload[7] |= self.state.heating_mode & 0x03;

        // Offset 8: temperature scale, time format, filter mode
        if self.state.temp_scale_celsius {
            payload[8] |= 0x01; // bit 0: Celsius
        }
        // bit 1: 24h format (set)
        payload[8] |= 0x02;
        // bits 2-3: filter mode = 0

        // Offset 9: heating state, temp range
        if self.state.is_heating {
            payload[9] |= 0x30; // bits 4-5: heating active
        }
        if self.state.temp_range_high {
            payload[9] |= 0x04; // bit 2: temp range high
        }

        // Offset 10: pump status (PP byte)
        // pump1 bits 0-1, pump2 bits 2-3, pump3 bits 4-5
        payload[10] = (self.state.pump1 & 0x03)
            | ((self.state.pump2 & 0x03) << 2)
            | ((self.state.pump3 & 0x03) << 4);

        // Offset 11: circ pump, blower flags
        if self.state.circ_pump {
            payload[11] |= 0x02;
        }
        if self.state.blower {
            payload[11] |= 0x0C;
        }

        // Offset 12: mister
        if self.state.mister {
            payload[12] |= 0x01;
        }

        // Offset 13: light1
        if self.state.light1 {
            payload[13] |= 0x03;
        }

        // Offset 20: set temperature
        payload[20] = self.state.set_temp;

        FrameEncoder::encode([0xFF, 0xAF], &payload)
    }

    /// Generate a configuration response for BP6013G1 with 2 pumps, circ pump, 1 light, 240V heater.
    ///
    /// Message type: `0A BF`, sub-type byte `0x2E`, followed by config payload.
    pub fn generate_config_response(&self) -> Vec<u8> {
        let mut config_payload = [0u8; 10];

        // Byte 0-1: general config
        config_payload[0] = 0x02; // some setup value
        config_payload[1] = 0x02;

        // Byte 2: temperature scale
        if self.state.temp_scale_celsius {
            config_payload[3] |= 0x01;
        }

        // Byte 5: pump configs (6 pumps packed 2 bits each)
        // Pump1 = TwoSpeed (0x02) bits 0-1
        // Pump2 = TwoSpeed (0x02) bits 2-3
        config_payload[5] = 0x02 | (0x02 << 2); // Pump1=TwoSpeed, Pump2=TwoSpeed

        // Byte 7: light1 present
        config_payload[7] |= 0x01; // Light1 present

        // Byte 8: circ pump (bit 7), blower (bits 0-1)
        config_payload[8] |= 0x80; // Circ pump present
        config_payload[8] |= 0x01; // Blower present

        // Build the full payload: sub-type 0x2E + config data
        let mut full_payload = vec![0x2E];
        full_payload.extend_from_slice(&config_payload);

        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate an information response.
    ///
    /// Message type: `0A BF`, sub-type byte `0x24`, followed by 21 bytes of info data.
    pub fn generate_information_response(&self) -> Vec<u8> {
        let mut info_data = [0u8; 21];

        // Bytes 0-1: Software ID
        info_data[0] = 0x64;
        info_data[1] = 0xDC;

        // Bytes 2-3: Software Version
        info_data[2] = 0x11;
        info_data[3] = 0x00;

        // Bytes 4-11: System Model "BFBP20  " (8 ASCII bytes)
        let model: &[u8] = b"BFBP20  ";
        info_data[4..12].copy_from_slice(model);

        // Byte 12: Current Setup
        info_data[12] = 0x01;

        // Bytes 13-16: Config Signature
        info_data[13] = 0x3D;
        info_data[14] = 0x12;
        info_data[15] = 0x38;
        info_data[16] = 0x2E;

        // Bytes 17-18: Heater Voltage (0x01=240V), Heater Type (0x0A=Standard)
        info_data[17] = 0x01;
        info_data[18] = 0x0A;

        // Bytes 19-20: DIP Switch Settings
        info_data[19] = 0x04;
        info_data[20] = 0x00;

        // Build the full payload: sub-type 0x24 + info data
        let mut full_payload = vec![0x24];
        full_payload.extend_from_slice(&info_data);

        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate a fault log response.
    ///
    /// Message type: `0A BF`, sub-type byte `0x28`.
    pub fn generate_fault_log_response(&self) -> Vec<u8> {
        let fault_data: [u8; 10] = [
            0x03, // fault count: 3
            0x01, // entry number: 1
            0x1B, // message code: 27 = HeaterDry
            0x02, // days ago: 2
            0x0E, // hour: 14
            0x1E, // minute: 30
            0x04, // flags
            0x68, // set temperature: 104
            0x68, // sensor A temp: 104
            0x66, // sensor B temp: 102
        ];

        // Build the full payload: sub-type 0x28 + fault data
        let mut full_payload = vec![0x28];
        full_payload.extend_from_slice(&fault_data);

        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate a filter cycles response.
    ///
    /// Message type: `0A BF`, sub-type byte `0x23`.
    pub fn generate_filter_cycles_response(&self) -> Vec<u8> {
        let filter_data: [u8; 8] = [
            0x08, 0x00, 0x04, 0x00, // Filter 1: start 08:00, dur 4h00m
            0x90, 0x00, 0x02, 0x00, // Filter 2: start 16:00, dur 2h00m, enabled
        ];

        // Build the full payload: sub-type 0x23 + filter data
        let mut full_payload = vec![0x23];
        full_payload.extend_from_slice(&filter_data);

        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate a registration query (`FE BF 00`).
    pub fn generate_registration_query(&self) -> Vec<u8> {
        FrameEncoder::encode([0xFE, 0xBF], &[0x00])
    }

    /// Generate a client ID assignment response (`FE BF 02 <ID>`).
    fn generate_client_id_assignment(&self, id: u8) -> Vec<u8> {
        FrameEncoder::encode([0xFE, 0xBF], &[0x02, id])
    }

    /// Process a command frame from the client, optionally return a response frame.
    ///
    /// Handles:
    /// - Configuration requests → generates config response
    /// - Toggle commands → updates internal state
    /// - Set temperature → updates target temp
    /// - Information requests → generates info response
    /// - Fault log requests → generates fault response
    /// - Registration request → assigns client ID
    /// - Client ID ack → records client ID
    pub fn process_incoming(&mut self, frame: &Frame) -> Option<Vec<u8>> {
        match frame.message_type {
            [0x0A, 0xBF] => {
                if frame.payload.is_empty() {
                    return None;
                }
                match frame.payload[0] {
                    // Configuration request
                    0x04 => Some(self.generate_config_response()),

                    // Toggle item
                    0x11 => {
                        if frame.payload.len() >= 2 {
                            self.handle_toggle(frame.payload[1]);
                        }
                        None
                    }

                    // Set temperature
                    0x20 => {
                        if frame.payload.len() >= 2 {
                            self.state.set_temp = frame.payload[1];
                        }
                        None
                    }

                    // Settings request (sub-types)
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

            // Registration: FE BF 01 → client requesting ID
            [0xFE, 0xBF] => {
                if frame.payload.len() >= 1 && frame.payload[0] == 0x01 {
                    // Assign a client ID
                    let id = self.next_client_id;
                    self.next_client_id += 1;
                    Some(self.generate_client_id_assignment(id))
                } else {
                    None
                }
            }

            // Client ID ack: <ID> BF 03
            _ => {
                if frame.payload.len() >= 1
                    && frame.message_type[1] == 0xBF
                    && frame.payload[0] == 0x03
                {
                    self.client_id = Some(frame.message_type[0]);
                }
                None
            }
        }
    }

    /// Handle a toggle item command.
    fn handle_toggle(&mut self, item_code: u8) {
        match item_code {
            0x04 => {
                // Pump1 toggle: off→low→high→off
                self.state.pump1 = (self.state.pump1 + 1) % 3;
            }
            0x05 => {
                self.state.pump2 = (self.state.pump2 + 1) % 3;
            }
            0x06 => {
                self.state.pump3 = (self.state.pump3 + 1) % 3;
            }
            0x0C => {
                self.state.blower = !self.state.blower;
            }
            0x11 => {
                self.state.light1 = !self.state.light1;
            }
            0x3C => {
                self.state.hold = !self.state.hold;
            }
            0x51 => {
                // Cycle heating mode: Ready→Rest→ReadyInRest→Ready
                self.state.heating_mode = match self.state.heating_mode {
                    0 => 1,
                    1 => 3,
                    3 => 0,
                    _ => 0,
                };
            }
            0x50 => {
                self.state.temp_range_high = !self.state.temp_range_high;
            }
            _ => {}
        }
    }

    /// Simulate 1 second passing (update temps, etc.).
    pub fn tick(&mut self) {
        // Simulate temperature approaching set point
        if self.state.current_temp < self.state.set_temp && self.state.is_heating {
            self.state.current_temp = self.state.current_temp.saturating_add(1).min(self.state.set_temp);
        } else if self.state.current_temp > self.state.set_temp {
            self.state.current_temp = self.state.current_temp.saturating_sub(1).max(self.state.set_temp);
        }

        // Advance time
        self.state.minute += 1;
        if self.state.minute >= 60 {
            self.state.minute = 0;
            self.state.hour = (self.state.hour + 1) % 24;
        }
    }
}
