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
    pub pump1: PumpState,
    pub pump2: PumpState,
    pub pump3: PumpState,
    pub circ_pump: bool,
    pub blower: bool,
    pub mister: bool,
    pub light1: bool,
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
    /// Payload layout (24 bytes):
    /// ```text
    ///  0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15 16 17 18 19 20 21 22 23
    /// F0 F1 CT HH MM F2 -- -- -- F3 F4 PP -- F5 LF F6 -- -- -- -- ST -- -- --
    /// ```
    pub fn parse(payload: &[u8]) -> Result<Self, StatusError> {
        if payload.len() < 24 {
            return Err(StatusError::UnexpectedLength(payload.len()));
        }

        // Offset 8 (F5): temperature scale, time format, filter mode
        let scale = if payload[8] & 0x01 != 0 {
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

        // Offset 7 (F4): heating mode (0=Ready, 1=Rest, 3=Ready-in-Rest)
        let heating_mode = match payload[7] & 0x03 {
            0 => HeatingMode::Ready,
            1 => HeatingMode::Rest,
            3 => HeatingMode::ReadyInRest,
            _ => HeatingMode::Ready,
        };

        // Offset 10 (PP): pump status
        let pp = payload[10];
        let pump1 = decode_pump_state(pp & 0x03);
        let pump2 = decode_pump_state((pp >> 2) & 0x03);
        let pump3 = decode_pump_state((pp >> 4) & 0x03);

        // Offset 11 (F5): circ pump, blower
        let pump_flags = payload[11];
        let circ_pump = pump_flags & 0x02 != 0;
        let blower = pump_flags & 0x0C != 0;

        // Offset 12 (F6): mister
        let mister = payload[12] & 0x01 != 0;

        // Offset 9 (F6): heating state, temp range
        let heating_flags = payload[9];
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
            time_format: if payload[8] & 0x02 != 0 {
                TimeFormat::Hour24
            } else {
                TimeFormat::Hour12
            },
            filter_mode: (payload[8] >> 2) & 0x03,
            is_heating,
            temp_range,
            pump1,
            pump2,
            pump3,
            circ_pump,
            blower,
            mister,
            light1: payload[13] & 0x03 != 0,
            is_priming: payload[6] & 0x01 != 0,
            is_hold: payload[5] & 0x05 != 0,
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
        // Construct a plausible status payload
        let mut payload = [0u8; 24];
        payload[2] = 100;  // current temp = 100°F
        payload[3] = 14;   // hour
        payload[4] = 30;   // minute
        payload[8] = 0x30; // heating active, temp range high
        payload[9] = 0x04; // temp range high
        payload[10] = 0x03; // pump1=high (bits 0-1=03 → actually 3=off, let me use 1)
        payload[10] = 0x01; // pump1=low
        payload[13] = 0x03; // light on
        payload[20] = 104;  // set temp = 104°F

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.current_temp, Some(100.0));
        assert_eq!(status.set_temp, 104.0);
        assert_eq!(status.hour, 14);
        assert_eq!(status.minute, 30);
        assert_eq!(status.temperature_scale, TemperatureScale::Fahrenheit);
        assert_eq!(status.pump1, PumpState::Low);
        assert!(status.light1);
    }

    #[test]
    fn test_parse_status_celsius_unknown_temp() {
        let mut payload = [0u8; 24];
        payload[2] = 0xFF;  // unknown temp
        payload[8] = 0x01;  // celsius
        payload[20] = 76;   // set temp = 38°C (76/2)

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.current_temp, None);
        assert_eq!(status.set_temp, 38.0);
        assert_eq!(status.temperature_scale, TemperatureScale::Celsius);
    }
}
