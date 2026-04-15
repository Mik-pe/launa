/// Parsed status update from the spa controller.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusUpdate {
    pub current_temp: Option<f32>,
    pub set_temp: f32,
    pub hour: u8,
    pub minute: u8,
    pub heating_mode: HeatingMode,
    pub temperature_scale: TemperatureScale,
    pub time_format: TimeFormat,
    pub filter_mode: u8,
    pub is_heating: bool,
    pub temp_range: TempRange,
    pub pumps: [PumpState; 6],
    pub circ_pump: bool,
    pub blower: bool,
    pub mister: bool,
    pub lights: [bool; 2],
    pub is_priming: bool,
    pub is_hold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeatingMode {
    Ready,
    Rest,
    ReadyInRest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemperatureScale {
    Fahrenheit,
    Celsius,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    Hour12,
    Hour24,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempRange {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpState {
    Off,
    Low,
    High,
}

impl StatusUpdate {
    /// Parse a status update from the frame payload.
    /// Message type is `FF AF 13`.
    ///
    /// Verified against real Balboa BP6013G1 hardware (see NorthernMan54/esp32_balboa_spa).
    /// Payload layout (24 bytes):
    /// ```text
    ///  0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15 16 17 18 19 20 21 22 23
    /// ST IM CT HH MM HM RT SA SB F9 FA P1 P2 CB LF MR -- -- -- -- ST -- -- --
    /// ```
    ///
    /// - ST = Spa State (0x00=Running, 0x05=Hold, 0x14=A/B Temps, 0x17=Test)
    /// - IM = Init Mode (0x00=Idle, 0x01=Priming)
    /// - CT = Current Temperature (÷2 if Celsius; 0xFF = unknown)
    /// - HM = Heating Mode (0=Ready, 1=Rest, 3=Ready-in-Rest)
    /// - F9 = Flags: bit 0=Temp Scale, bit 1=24h Time, bits 2-3=Filter Mode
    /// - FA = Flags: bit 2=Temp Range, bits 4-5=Heating State
    /// - P1 = Pumps 1-4 (2 bits each), P2 = Pumps 5-6
    /// - CB = Circ pump (bit 1), Blower (bits 2-3)
    /// - LF = Lights (bits 0-1=Light1), MR = Mister (0=off, 1=on)
    /// - ST = Set Temperature (÷2 if Celsius)
    pub fn parse(payload: &[u8]) -> Result<Self, StatusError> {
        if payload.len() < 24 {
            return Err(StatusError::UnexpectedLength(payload.len()));
        }

        // Offset 9 (F9): temperature scale, time format, filter mode
        let scale = if payload[9] & 0x01 != 0 {
            TemperatureScale::Celsius
        } else {
            TemperatureScale::Fahrenheit
        };

        let temp_divisor: f32 = match scale {
            TemperatureScale::Celsius => 2.0,
            TemperatureScale::Fahrenheit => 1.0,
        };

        // Offset 2: current temperature
        let current_temp = if payload[2] == 0xFF {
            None
        } else {
            Some(payload[2] as f32 / temp_divisor)
        };

        // Offset 20: set temperature
        let set_temp = payload[20] as f32 / temp_divisor;

        // Offset 5 (HM): heating mode (0=Ready, 1=Rest, 3=Ready-in-Rest)
        let heating_mode = match payload[5] & 0x03 {
            0 => HeatingMode::Ready,
            1 => HeatingMode::Rest,
            3 => HeatingMode::ReadyInRest,
            _ => HeatingMode::Ready,
        };

        // Offset 11 (P1): pump status (pumps 1-4, 2 bits each)
        let pp = payload[11];

        // Offset 12 (P2): pump5 bits 0-1, pump6 bits 2-3
        let p2 = payload[12];
        let pumps = [
            decode_pump_state(pp & 0x03),           // pump1
            decode_pump_state((pp >> 2) & 0x03),    // pump2
            decode_pump_state((pp >> 4) & 0x03),    // pump3
            decode_pump_state((pp >> 6) & 0x03),    // pump4
            decode_pump_state(p2 & 0x03),           // pump5
            decode_pump_state((p2 >> 2) & 0x03),    // pump6
        ];

        // Offset 13 (CB): circ pump, blower
        let circ_blower = payload[13];
        let circ_pump = circ_blower & 0x02 != 0;
        let blower = circ_blower & 0x0C != 0;

        // Offset 15 (MR): mister
        let mister = payload[15] != 0;

        // Offset 10 (FA): heating state, temp range
        let heating_flags = payload[10];
        let is_heating = heating_flags & 0x30 != 0;
        let temp_range = if heating_flags & 0x04 != 0 {
            TempRange::High
        } else {
            TempRange::Low
        };

        Ok(StatusUpdate {
            current_temp,
            set_temp,
            hour: payload[3],
            minute: payload[4],
            heating_mode,
            temperature_scale: scale,
            time_format: if payload[9] & 0x02 != 0 {
                TimeFormat::Hour24
            } else {
                TimeFormat::Hour12
            },
            filter_mode: (payload[9] >> 2) & 0x03,
            is_heating,
            temp_range,
            pumps,
            circ_pump,
            blower,
            mister,
            lights: [
                payload[14] & 0x03 != 0,   // light1
                payload[14] & 0x0C != 0,   // light2
            ],
            is_priming: payload[1] == 0x01,
            is_hold: payload[0] == 0x05,
        })
    }
}

fn decode_pump_state(bits: u8) -> PumpState {
    match bits {
        0 => PumpState::Off,
        1 => PumpState::Low,
        2 => PumpState::High,
        _ => PumpState::Off,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusError {
    UnexpectedLength(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_fahrenheit() {
        // Construct a plausible status payload using correct offsets
        let mut payload = [0u8; 24];
        payload[0] = 0x00;  // spa state: running
        payload[1] = 0x00;  // init mode: idle
        payload[2] = 100;   // current temp = 100°F
        payload[3] = 14;    // hour
        payload[4] = 30;    // minute
        payload[5] = 0x00;  // heating mode: Ready
        payload[9] = 0x02;  // 24h time format
        payload[10] = 0x34; // heating active (bits 4-5=0x30) + temp range high (bit 2)
        payload[11] = 0x01; // pump1=low
        payload[14] = 0x03; // light on
        payload[20] = 104;  // set temp = 104°F

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.current_temp, Some(100.0));
        assert_eq!(status.set_temp, 104.0);
        assert_eq!(status.hour, 14);
        assert_eq!(status.minute, 30);
        assert_eq!(status.temperature_scale, TemperatureScale::Fahrenheit);
        assert_eq!(status.pumps[0], PumpState::Low);
        assert!(status.lights[0]);
        assert!(status.is_heating);
        assert_eq!(status.temp_range, TempRange::High);
    }

    #[test]
    fn test_parse_status_celsius_unknown_temp() {
        let mut payload = [0u8; 24];
        payload[2] = 0xFF;  // unknown temp
        payload[9] = 0x01;  // celsius (bit 0)
        payload[20] = 76;   // set temp = 38°C (76/2)

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.current_temp, None);
        assert_eq!(status.set_temp, 38.0);
        assert_eq!(status.temperature_scale, TemperatureScale::Celsius);
    }

    #[test]
    fn test_parse_status_hold_and_priming() {
        let mut payload = [0u8; 24];
        payload[0] = 0x05;  // spa state: hold mode
        payload[1] = 0x01;  // init mode: priming
        payload[2] = 100;   // temp
        payload[9] = 0x02;  // 24h time
        payload[20] = 104;  // set temp

        let status = StatusUpdate::parse(&payload).unwrap();
        assert!(status.is_hold);
        assert!(status.is_priming);
    }

    #[test]
    fn test_parse_status_heating_modes() {
        for (val, expected) in [
            (0u8, HeatingMode::Ready),
            (1u8, HeatingMode::Rest),
            (3u8, HeatingMode::ReadyInRest),
        ] {
            let mut payload = [0u8; 24];
            payload[2] = 100;
            payload[5] = val;  // heating mode at offset 5
            payload[9] = 0x02;
            payload[20] = 104;
            let status = StatusUpdate::parse(&payload).unwrap();
            assert_eq!(status.heating_mode, expected);
        }
    }

    #[test]
    fn test_parse_status_pumps_and_circ_blower() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[9] = 0x02;
        payload[11] = 0x09;  // pump1=1(low), pump2=0(off), pump3=2(high) → 0b10_00_01_01 = 0x09
        payload[11] = (1 | (0 << 2) | (2 << 4)) as u8; // pump1=low, pump2=off, pump3=high
        payload[13] = 0x0E;  // circ pump (bit 1) + blower (bits 2-3)
        payload[15] = 0x01;  // mister on
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.pumps[0], PumpState::Low);
        assert_eq!(status.pumps[1], PumpState::Off);
        assert_eq!(status.pumps[2], PumpState::High);
        assert!(status.circ_pump);
        assert!(status.blower);
        assert!(status.mister);
    }
}
