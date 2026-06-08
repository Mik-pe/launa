use crate::temperature::Temperature;

/// Parsed status update from the spa controller.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusUpdate {
    pub current_temp: Option<Temperature>,
    pub set_temp: Temperature,
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
    pub lights: [bool; 4],
    pub is_priming: bool,
    pub is_hold: bool,
    /// Reminder/notification type from offset 6.
    /// Common values: 0x00=None, 0x04=Clean Filter, etc.
    pub notification_type: u8,
    /// Panel lock status from offset 18 bit 0.
    pub panel_locked: bool,
    /// Settings lock status from offset 19 bit 0.
    pub settings_lock: bool,
    /// M8 cycle time (aux/timer) from offset 21.
    pub m8_cycle_time: u8,
    /// Sensor A temperature from offset 7.
    /// `Some(Temperature)` when not in Hold mode, `None` when `is_hold == true`.
    pub sensor_a_temp: Option<Temperature>,
    /// Sensor B temperature from offset 8.
    /// `Some(Temperature)` when A/B temps mode is active (`payload[0] == 0x14`), `None` otherwise.
    pub sensor_b_temp: Option<Temperature>,
    /// Hold timer remaining minutes from offset 7 when in Hold mode.
    /// `Some(u8)` when `is_hold == true`, `None` otherwise.
    /// Mutually exclusive with `sensor_a_temp` (same offset, dual interpretation).
    pub hold_timer_minutes: Option<u8>,
}

/// Spa heating mode — controls when the heater is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeatingMode {
    /// Heater runs whenever temperature is below set point.
    Ready,
    /// Heater is off; economy mode.
    Rest,
    /// Heater runs in Ready mode during scheduled hours, Rest otherwise.
    ReadyInRest,
}

/// Temperature display scale reported by the spa controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TemperatureScale {
    Fahrenheit,
    Celsius,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    Hour12,
    Hour24,
}

/// Temperature range — determines the min/max set temperature bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TempRange {
    Low,
    High,
}

/// Pump speed state as reported by the spa controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PumpState {
    /// Pump is off.
    Off,
    /// Pump running at low speed.
    Low,
    /// Pump running at high speed.
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
    /// - LF = Lights (bits 0-1=Light1, bits 2-3=Light2, bits 4-5=Light3, bits 6-7=Light4), MR = Mister (0=off, 1=on)
    /// - ST = Set Temperature (÷2 if Celsius)
    pub fn parse(payload: &[u8]) -> Result<Self, StatusError> {
        if payload.len() < 24 {
            return Err(StatusError::UnexpectedLength(payload.len()));
        }

        let scale = if payload[9] & 0x01 != 0 {
            TemperatureScale::Celsius
        } else {
            TemperatureScale::Fahrenheit
        };

        let current_temp = if payload[2] == 0xFF {
            None
        } else {
            Some(Temperature::from_wire(payload[2], scale))
        };

        let set_temp = Temperature::from_wire(payload[20], scale);

        let heating_mode = match payload[5] & 0x03 {
            0 => HeatingMode::Ready,
            1 => HeatingMode::Rest,
            3 => HeatingMode::ReadyInRest,
            _ => HeatingMode::Ready,
        };

        let pumps = crate::pump_bits::decode_pumps(payload[11], payload[12], decode_pump_state);

        let circ_blower = payload[13];
        let circ_pump = circ_blower & 0x02 != 0;
        let blower = circ_blower & 0x0C != 0;

        let mister = payload[15] != 0;

        let heating_flags = payload[10];
        let is_heating = heating_flags & 0x30 != 0;
        let temp_range = if heating_flags & 0x04 != 0 {
            TempRange::High
        } else {
            TempRange::Low
        };

        let notification_type = payload[6];

        let panel_locked = payload[18] & 0x01 != 0;

        let settings_lock = payload[19] & 0x01 != 0;

        let m8_cycle_time = payload[21];

        let is_hold = payload[0] == 0x05;
        let is_ab_temps = payload[0] == 0x14;

        let (sensor_a_temp, hold_timer_minutes) = if is_hold {
            (None, Some(payload[7]))
        } else {
            (Some(Temperature::from_wire(payload[7], scale)), None)
        };

        let sensor_b_temp = if is_ab_temps {
            Some(Temperature::from_wire(payload[8], scale))
        } else {
            None
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
                payload[14] & 0x03 != 0, // light1 bits 0-1
                payload[14] & 0x0C != 0, // light2 bits 2-3
                payload[14] & 0x30 != 0, // light3 bits 4-5
                payload[14] & 0xC0 != 0, // light4 bits 6-7
            ],
            is_priming: payload[1] == 0x01,
            is_hold,
            notification_type,
            panel_locked,
            settings_lock,
            m8_cycle_time,
            sensor_a_temp,
            sensor_b_temp,
            hold_timer_minutes,
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
        payload[0] = 0x00; // spa state: running
        payload[1] = 0x00; // init mode: idle
        payload[2] = 100; // current temp = 100°F
        payload[3] = 14; // hour
        payload[4] = 30; // minute
        payload[5] = 0x00; // heating mode: Ready
        payload[9] = 0x02; // 24h time format
        payload[10] = 0x34; // heating active (bits 4-5=0x30) + temp range high (bit 2)
        payload[11] = 0x01; // pump1=low
        payload[14] = 0x03; // light1 on
        payload[20] = 104; // set temp = 104°F

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.current_temp, Some(Temperature::fahrenheit(100.0)));
        assert_eq!(status.set_temp, Temperature::fahrenheit(104.0));
        assert_eq!(status.hour, 14);
        assert_eq!(status.minute, 30);
        assert_eq!(status.temperature_scale, TemperatureScale::Fahrenheit);
        assert_eq!(status.pumps[0], PumpState::Low);
        assert!(status.lights[0]);
        assert!(status.is_heating);
        assert_eq!(status.temp_range, TempRange::High);
    }

    #[test]
    fn test_parse_status_all_4_lights_on() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[9] = 0x02;
        payload[14] = 0xFF; // all light bits set
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.lights, [true, true, true, true]);
    }

    #[test]
    fn test_parse_status_light3_on_only() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[9] = 0x02;
        payload[14] = 0x30; // bits 4-5 set (Light3 only)
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.lights, [false, false, true, false]);
    }

    #[test]
    fn test_parse_status_light4_on_only() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[9] = 0x02;
        payload[14] = 0xC0; // bits 6-7 set (Light4 only)
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.lights, [false, false, false, true]);
    }

    #[test]
    fn test_parse_status_individual_lights() {
        // Test each light individually
        let cases: [(u8, [bool; 4]); 4] = [
            (0x03, [true, false, false, false]), // Light1
            (0x0C, [false, true, false, false]), // Light2
            (0x30, [false, false, true, false]), // Light3
            (0xC0, [false, false, false, true]), // Light4
        ];
        for (byte, expected) in cases {
            let mut payload = [0u8; 24];
            payload[2] = 100;
            payload[9] = 0x02;
            payload[14] = byte;
            payload[20] = 104;
            let status = StatusUpdate::parse(&payload).unwrap();
            assert_eq!(
                status.lights, expected,
                "Failed for payload[14]=0x{:02X}",
                byte
            );
        }
    }

    #[test]
    fn test_parse_status_celsius_unknown_temp() {
        let mut payload = [0u8; 24];
        payload[2] = 0xFF; // unknown temp
        payload[9] = 0x01; // celsius (bit 0)
        payload[20] = 76; // set temp = 38°C (76/2)

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.current_temp, None);
        assert_eq!(status.set_temp, Temperature::celsius(38.0));
        assert_eq!(status.temperature_scale, TemperatureScale::Celsius);
    }

    #[test]
    fn test_parse_status_hold_and_priming() {
        let mut payload = [0u8; 24];
        payload[0] = 0x05; // spa state: hold mode
        payload[1] = 0x01; // init mode: priming
        payload[2] = 100; // temp
        payload[9] = 0x02; // 24h time
        payload[20] = 104; // set temp

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
            payload[5] = val; // heating mode at offset 5
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
        payload[11] = (1 | (2 << 4)) as u8; // pump1=low, pump2=off, pump3=high
        payload[13] = 0x0E; // circ pump (bit 1) + blower (bits 2-3)
        payload[15] = 0x01; // mister on
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.pumps[0], PumpState::Low);
        assert_eq!(status.pumps[1], PumpState::Off);
        assert_eq!(status.pumps[2], PumpState::High);
        assert!(status.circ_pump);
        assert!(status.blower);
        assert!(status.mister);
    }

    #[test]
    fn test_parse_status_new_fields_default() {
        // Default payload (all zeros for new fields)
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.notification_type, 0);
        assert!(!status.panel_locked);
        assert!(!status.settings_lock);
        assert_eq!(status.m8_cycle_time, 0);
    }

    #[test]
    fn test_parse_status_notification_type() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[6] = 0x04; // Clean filter reminder
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.notification_type, 0x04);
    }

    #[test]
    fn test_parse_status_panel_locked() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[18] = 0x01; // panel locked (bit 0)
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert!(status.panel_locked);
        assert!(!status.settings_lock); // should be separate
    }

    #[test]
    fn test_parse_status_settings_lock() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[19] = 0x01; // settings lock (bit 0)
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert!(status.settings_lock);
        assert!(!status.panel_locked); // should be separate
    }

    #[test]
    fn test_parse_status_m8_cycle_time() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[21] = 30; // M8 cycle time = 30 minutes
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.m8_cycle_time, 30);
    }

    #[test]
    fn test_parse_status_all_new_fields_together() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[6] = 0x04; // notification: clean filter
        payload[18] = 0x01; // panel locked
        payload[19] = 0x01; // settings lock
        payload[21] = 45; // m8 cycle time
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.notification_type, 0x04);
        assert!(status.panel_locked);
        assert!(status.settings_lock);
        assert_eq!(status.m8_cycle_time, 45);
    }

    // Tests for sensor_a_temp, sensor_b_temp, hold_timer_minutes

    #[test]
    fn test_sensor_a_temp_fahrenheit_normal_mode() {
        // Normal mode (payload[0] == 0x00): offset 7 is sensor A temperature
        let mut payload = [0u8; 24];
        payload[0] = 0x00; // running mode
        payload[2] = 100; // current temp
        payload[7] = 98; // sensor A temp = 98°F
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_a_temp, Some(Temperature::fahrenheit(98.0)));
        assert_eq!(status.hold_timer_minutes, None); // not in hold mode
    }

    #[test]
    fn test_sensor_a_temp_celsius() {
        // Celsius mode: sensor A temp divided by 2
        let mut payload = [0u8; 24];
        payload[0] = 0x00; // running mode
        payload[2] = 76; // current temp (38°C)
        payload[7] = 74; // sensor A temp raw = 74 → 37.0°C
        payload[9] = 0x01; // Celsius
        payload[20] = 80;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_a_temp, Some(Temperature::celsius(37.0)));
    }

    #[test]
    fn test_sensor_a_temp_none_when_hold_mode() {
        // Hold mode: offset 7 is hold timer minutes, not sensor A temp
        let mut payload = [0u8; 24];
        payload[0] = 0x05; // hold mode
        payload[2] = 100;
        payload[7] = 45; // hold timer minutes, not sensor A temp
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert!(status.is_hold);
        assert_eq!(status.sensor_a_temp, None);
        assert_eq!(status.hold_timer_minutes, Some(45));
    }

    #[test]
    fn test_sensor_b_temp_ab_temps_mode() {
        // A/B temps mode (payload[0] == 0x14): sensor B temp at offset 8
        let mut payload = [0u8; 24];
        payload[0] = 0x14; // A/B temps mode
        payload[2] = 100;
        payload[7] = 98; // sensor A temp
        payload[8] = 96; // sensor B temp = 96°F
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_b_temp, Some(Temperature::fahrenheit(96.0)));
        assert_eq!(status.sensor_a_temp, Some(Temperature::fahrenheit(98.0))); // sensor A still present
    }

    #[test]
    fn test_sensor_b_temp_celsius_ab_mode() {
        // A/B temps mode with Celsius: sensor B divided by 2
        let mut payload = [0u8; 24];
        payload[0] = 0x14; // A/B temps mode
        payload[2] = 76;
        payload[7] = 74; // sensor A raw = 74 → 37.0°C
        payload[8] = 72; // sensor B raw = 72 → 36.0°C
        payload[9] = 0x01; // Celsius
        payload[20] = 80;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_b_temp, Some(Temperature::celsius(36.0)));
        assert_eq!(status.sensor_a_temp, Some(Temperature::celsius(37.0)));
    }

    #[test]
    fn test_sensor_b_temp_none_normal_mode() {
        // Normal running mode: sensor B temp is None
        let mut payload = [0u8; 24];
        payload[0] = 0x00; // running mode
        payload[2] = 100;
        payload[8] = 96; // offset 8 exists but not in A/B mode
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_b_temp, None);
    }

    #[test]
    fn test_hold_timer_minutes_none_normal_mode() {
        // Normal mode: hold_timer_minutes should be None
        let mut payload = [0u8; 24];
        payload[0] = 0x00; // running mode
        payload[2] = 100;
        payload[7] = 98; // sensor A temp, not hold timer
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert!(!status.is_hold);
        assert_eq!(status.hold_timer_minutes, None);
    }

    #[test]
    fn test_hold_timer_minutes_hold_mode() {
        // Hold mode: hold_timer_minutes from offset 7
        let mut payload = [0u8; 24];
        payload[0] = 0x05; // hold mode
        payload[2] = 100;
        payload[7] = 60; // 60 minutes remaining
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert!(status.is_hold);
        assert_eq!(status.hold_timer_minutes, Some(60));
    }

    #[test]
    fn test_hold_timer_zero_minutes() {
        // Hold mode with 0 minutes remaining (about to expire)
        let mut payload = [0u8; 24];
        payload[0] = 0x05; // hold mode
        payload[2] = 100;
        payload[7] = 0; // 0 minutes
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.hold_timer_minutes, Some(0));
        assert_eq!(status.sensor_a_temp, None);
    }

    #[test]
    fn test_mutual_exclusivity_hold_mode() {
        // VAL-PROTO-004: Hold mode → hold_timer_minutes = Some(N) + sensor_a_temp = None
        let mut payload = [0u8; 24];
        payload[0] = 0x05; // hold mode
        payload[2] = 100;
        payload[7] = 30;
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.hold_timer_minutes, Some(30));
        assert_eq!(status.sensor_a_temp, None);
    }

    #[test]
    fn test_mutual_exclusivity_normal_mode() {
        // VAL-PROTO-004: Normal mode → sensor_a_temp = Some(T) + hold_timer_minutes = None
        let mut payload = [0u8; 24];
        payload[0] = 0x00; // running mode
        payload[2] = 100;
        payload[7] = 98;
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_a_temp, Some(Temperature::fahrenheit(98.0)));
        assert_eq!(status.hold_timer_minutes, None);
    }

    #[test]
    fn test_sensor_a_temp_zero_value() {
        // sensor_a_temp of 0 is still valid (Some(0.0))
        let mut payload = [0u8; 24];
        payload[0] = 0x00;
        payload[2] = 0;
        payload[7] = 0; // 0°F
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_a_temp, Some(Temperature::fahrenheit(0.0)));
    }

    #[test]
    fn test_sensor_b_temp_zero_ab_mode() {
        // sensor_b_temp of 0 is still valid in A/B mode
        let mut payload = [0u8; 24];
        payload[0] = 0x14; // A/B temps
        payload[2] = 100;
        payload[7] = 98;
        payload[8] = 0; // 0°F sensor B
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.sensor_b_temp, Some(Temperature::fahrenheit(0.0)));
    }

    #[test]
    fn test_hold_mode_ab_temps_combined() {
        // Hold mode + A/B temps: payload[0] == 0x05 (hold takes precedence)
        // offset 7 = hold timer, sensor_b_temp should be None
        let mut payload = [0u8; 24];
        payload[0] = 0x05; // hold mode (not A/B)
        payload[2] = 100;
        payload[7] = 45;
        payload[8] = 96;
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.hold_timer_minutes, Some(45));
        assert_eq!(status.sensor_a_temp, None);
        assert_eq!(status.sensor_b_temp, None); // not in A/B mode (0x14)
    }

    // --- Edge case tests: undefined values ---

    #[test]
    fn test_undefined_heating_mode_2_falls_back_to_ready() {
        // Heating mode value 2 is undefined (valid: 0=Ready, 1=Rest, 3=ReadyInRest)
        // The catch-all `_` arm maps it to Ready
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[5] = 0x02; // undefined heating mode
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.heating_mode, HeatingMode::Ready);
    }

    #[test]
    fn test_undefined_pump_state_3_falls_back_to_off() {
        // Pump raw value 3 is undefined (valid: 0=Off, 1=Low, 2=High)
        // decode_pump_state maps _ => Off
        let mut payload = [0u8; 24];
        payload[2] = 100;
        // Pack pump1 bits 0-1 = 3 (undefined): 0b00000011
        payload[11] = 0x03;
        payload[20] = 104;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert_eq!(status.pumps[0], PumpState::Off);
    }

    #[test]
    fn test_all_pumps_undefined_state_3() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[20] = 104;
        // All 2-bit pump fields set to 3: 0b11111111 for both bytes
        payload[11] = 0xFF; // pumps 0-3 all = 3
        payload[12] = 0xFF; // pumps 4-5 all = 3

        let status = StatusUpdate::parse(&payload).unwrap();
        for (i, &pump) in status.pumps.iter().enumerate() {
            assert_eq!(
                pump,
                PumpState::Off,
                "pump {} should be Off for undefined state 3",
                i
            );
        }
    }

    #[test]
    fn test_parse_status_too_short_returns_error() {
        let payload = [0u8; 10];
        let result = StatusUpdate::parse(&payload);
        assert!(result.is_err());
        match result {
            Err(StatusError::UnexpectedLength(len)) => assert_eq!(len, 10),
            Ok(_) => panic!("expected error for short payload"),
        }
    }

    #[test]
    fn test_parse_status_exactly_24_bytes_succeeds() {
        let mut payload = [0u8; 24];
        payload[2] = 100;
        payload[20] = 104;
        assert!(StatusUpdate::parse(&payload).is_ok());
    }

    #[test]
    fn test_parse_status_longer_payload_succeeds() {
        // Extra bytes beyond 24 should be ignored
        let mut payload = [0u8; 30];
        payload[2] = 100;
        payload[20] = 104;
        assert!(StatusUpdate::parse(&payload).is_ok());
    }
}
