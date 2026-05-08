use crate::status::{TempRange, TemperatureScale};

/// Hard upper limit that can never be exceeded regardless of range (108°F / 42°C).
pub const ABSOLUTE_MAX_TEMP_F: u8 = 108;
pub const ABSOLUTE_MAX_TEMP_C: u8 = 42;

/// Temperature validation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempError {
    BelowMin,
    AboveMax,
    AboveAbsoluteLimit,
}

/// Command encoding error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandError {
    /// SetTime hour exceeds 23 or has bit 7 set.
    InvalidHour(u8),
    /// SetTime minute exceeds 59.
    InvalidMinute(u8),
    /// SetTemperature value exceeds absolute safe limit.
    InvalidTemperature(u8),
}

/// Validate a set-temperature value against the spa's safe operating range.
///
/// Per Balboa protocol:
/// - Fahrenheit high range: 80-104°F
/// - Fahrenheit low range: 50-80°F
/// - Celsius high range: 26-40°C
/// - Celsius low range: 10-26°C
///
/// A hard upper limit of 108°F / 42°C is also enforced as a backstop.
///
/// Returns the clamped raw value suitable for the `SetTemperature` command,
/// or a `TempError` if the value exceeds the absolute hard limit.
pub fn validate_set_temperature(
    raw_value: u8,
    scale: TemperatureScale,
    range: TempRange,
) -> Result<u8, TempError> {
    let (min, max) = match (scale, range) {
        (TemperatureScale::Fahrenheit, TempRange::High) => (80u8, 104u8),
        (TemperatureScale::Fahrenheit, TempRange::Low) => (50u8, 80u8),
        (TemperatureScale::Celsius, TempRange::High) => (26u8, 40u8),
        (TemperatureScale::Celsius, TempRange::Low) => (10u8, 26u8),
    };

    // Enforce absolute hard limit
    let abs_max = match scale {
        TemperatureScale::Fahrenheit => ABSOLUTE_MAX_TEMP_F,
        TemperatureScale::Celsius => ABSOLUTE_MAX_TEMP_C,
    };

    if raw_value > abs_max {
        return Err(TempError::AboveAbsoluteLimit);
    }

    if raw_value < min {
        return Err(TempError::BelowMin);
    }
    if raw_value > max {
        return Err(TempError::AboveMax);
    }

    Ok(raw_value)
}

/// Outgoing commands to the spa controller.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    ConfigurationRequest,
    ToggleItem(ToggleItem),
    SetTemperature(u8),
    SetTime { hour: u8, minute: u8, is_24h: bool },
    SetTemperatureScale(bool), // true = Celsius
    SettingsRequest(SettingsType),
    FilterCyclesRequest,
    InformationRequest,
    FaultLogRequest { entry: u8 },
    NothingToSend { client_id: u8 },
    Sniff(Option<u16>),
    Reboot,
}

/// Toggleable spa component — used in toggle commands and pump timers.
///
/// Each variant maps to a specific bit in the Balboa toggle command payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToggleItem {
    Pump1,
    Pump2,
    Pump3,
    Pump4,
    Pump5,
    Pump6,
    Blower,
    Light1,
    Light2,
    Light3,
    Light4,
    Mister,
    Aux1,
    Aux2,
    HoldMode,
    HeatingMode,
    TemperatureRange,
    CirculationPump,
    SoakMode,
    NormalOperation,
    ClearNotification,
}

impl ToggleItem {
    pub fn code(self) -> u8 {
        match self {
            ToggleItem::Pump1 => 0x04,
            ToggleItem::Pump2 => 0x05,
            ToggleItem::Pump3 => 0x06,
            ToggleItem::Pump4 => 0x07,
            ToggleItem::Pump5 => 0x08,
            ToggleItem::Pump6 => 0x09,
            ToggleItem::Blower => 0x0C,
            ToggleItem::Light1 => 0x11,
            ToggleItem::Light2 => 0x12,
            ToggleItem::Light3 => 0x13,
            ToggleItem::Light4 => 0x14,
            ToggleItem::Mister => 0x0E,
            ToggleItem::Aux1 => 0x16,
            ToggleItem::Aux2 => 0x17,
            ToggleItem::HoldMode => 0x3C,
            ToggleItem::HeatingMode => 0x51,
            ToggleItem::TemperatureRange => 0x50,
            ToggleItem::CirculationPump => 0x3D,
            ToggleItem::SoakMode => 0x1D,
            ToggleItem::NormalOperation => 0x01,
            ToggleItem::ClearNotification => 0x03,
        }
    }

    /// Get the 0-based pump index (0-5), or None if not a pump.
    pub fn pump_index(self) -> Option<usize> {
        match self {
            ToggleItem::Pump1 => Some(0),
            ToggleItem::Pump2 => Some(1),
            ToggleItem::Pump3 => Some(2),
            ToggleItem::Pump4 => Some(3),
            ToggleItem::Pump5 => Some(4),
            ToggleItem::Pump6 => Some(5),
            _ => None,
        }
    }

    /// Get the 0-based light index (0-3), or None if not a light.
    pub fn light_index(self) -> Option<usize> {
        match self {
            ToggleItem::Light1 => Some(0),
            ToggleItem::Light2 => Some(1),
            ToggleItem::Light3 => Some(2),
            ToggleItem::Light4 => Some(3),
            _ => None,
        }
    }

    /// Create a ToggleItem from a pump index (0-5).
    pub fn from_pump_index(i: usize) -> Option<Self> {
        match i {
            0 => Some(ToggleItem::Pump1),
            1 => Some(ToggleItem::Pump2),
            2 => Some(ToggleItem::Pump3),
            3 => Some(ToggleItem::Pump4),
            4 => Some(ToggleItem::Pump5),
            5 => Some(ToggleItem::Pump6),
            _ => None,
        }
    }

    /// Create a ToggleItem from a light index (0-3).
    pub fn from_light_index(i: usize) -> Option<Self> {
        match i {
            0 => Some(ToggleItem::Light1),
            1 => Some(ToggleItem::Light2),
            2 => Some(ToggleItem::Light3),
            3 => Some(ToggleItem::Light4),
            _ => None,
        }
    }
}

/// Settings page type — determines which configuration response is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsType {
    Panel,
    FilterCycles,
    Information,
    Preferences,
}

impl Command {
    /// Returns the message type bytes and payload for this command.
    ///
    /// All outgoing commands use message type `0A BF`. The first byte of the
    /// payload acts as a sub-type discriminator per the Balboa protocol.
    ///
    /// Returns an error if `SetTime` or `SetTemperature` values are out of
    /// their valid ranges.
    pub fn encode(&self) -> Result<([u8; 2], Vec<u8>), CommandError> {
        match self {
            Command::ConfigurationRequest => Ok(([0x0A, 0xBF], vec![0x04])),
            Command::ToggleItem(item) => Ok(([0x0A, 0xBF], vec![0x11, item.code(), 0x00])),
            Command::SetTemperature(temp) => {
                if *temp > ABSOLUTE_MAX_TEMP_F {
                    return Err(CommandError::InvalidTemperature(*temp));
                }
                Ok(([0x0A, 0xBF], vec![0x20, *temp]))
            }
            Command::SetTime {
                hour,
                minute,
                is_24h,
            } => {
                if *hour > 23 || (*hour & 0x80) != 0 {
                    return Err(CommandError::InvalidHour(*hour));
                }
                if *minute > 59 {
                    return Err(CommandError::InvalidMinute(*minute));
                }
                let h = if *is_24h { *hour | 0x80 } else { *hour };
                Ok(([0x0A, 0xBF], vec![0x21, h, *minute]))
            }
            Command::SetTemperatureScale(celsius) => {
                let ts = if *celsius { 0x01 } else { 0x00 };
                Ok(([0x0A, 0xBF], vec![0x27, 0x01, ts]))
            }
            Command::SettingsRequest(SettingsType::Panel) => {
                Ok(([0x0A, 0xBF], vec![0x22, 0x00, 0x00, 0x01]))
            }
            Command::SettingsRequest(SettingsType::FilterCycles) | Command::FilterCyclesRequest => {
                Ok(([0x0A, 0xBF], vec![0x22, 0x01, 0x00, 0x00]))
            }
            Command::SettingsRequest(SettingsType::Information) | Command::InformationRequest => {
                Ok(([0x0A, 0xBF], vec![0x22, 0x02, 0x00, 0x00]))
            }
            Command::SettingsRequest(SettingsType::Preferences) => {
                Ok(([0x0A, 0xBF], vec![0x22, 0x08, 0x00, 0x00]))
            }
            Command::FaultLogRequest { entry } => {
                Ok(([0x0A, 0xBF], vec![0x22, 0x20, *entry, 0x00]))
            }
            Command::NothingToSend { client_id } => Ok(([*client_id, 0xBF], vec![0x07])),
            Command::Sniff(_) => Ok(([0x00, 0x00], Vec::new())), // not sent to spa
            Command::Reboot => Ok(([0x00, 0x00], Vec::new())),   // not sent to spa
        }
    }
}

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ToggleItem variant must map to the correct Balboa protocol code byte.
    #[test]
    fn toggle_item_code_table() {
        let cases: [(ToggleItem, u8); 21] = [
            (ToggleItem::Pump1, 0x04),
            (ToggleItem::Pump2, 0x05),
            (ToggleItem::Pump3, 0x06),
            (ToggleItem::Pump4, 0x07),
            (ToggleItem::Pump5, 0x08),
            (ToggleItem::Pump6, 0x09),
            (ToggleItem::Blower, 0x0C),
            (ToggleItem::Mister, 0x0E),
            (ToggleItem::Light1, 0x11),
            (ToggleItem::Light2, 0x12),
            (ToggleItem::Light3, 0x13),
            (ToggleItem::Light4, 0x14),
            (ToggleItem::Aux1, 0x16),
            (ToggleItem::Aux2, 0x17),
            (ToggleItem::SoakMode, 0x1D),
            (ToggleItem::NormalOperation, 0x01),
            (ToggleItem::ClearNotification, 0x03),
            (ToggleItem::HoldMode, 0x3C),
            (ToggleItem::CirculationPump, 0x3D),
            (ToggleItem::TemperatureRange, 0x50),
            (ToggleItem::HeatingMode, 0x51),
        ];
        for (i, (item, expected_code)) in cases.iter().enumerate() {
            assert_eq!(
                item.code(),
                *expected_code,
                "case {i}: {item:?}.code() mismatch"
            );
        }
    }

    /// Every Command variant must encode to the correct (message_type, payload) pair.
    #[test]
    fn command_encode_table() {
        #[derive(Debug)]
        struct Case {
            name: &'static str,
            cmd: Command,
            expected_mt: [u8; 2],
            expected_payload: Vec<u8>,
        }

        let cases = [
            Case {
                name: "ConfigurationRequest",
                cmd: Command::ConfigurationRequest,
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x04],
            },
            Case {
                name: "ToggleItem(Pump1)",
                cmd: Command::ToggleItem(ToggleItem::Pump1),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x04, 0x00],
            },
            Case {
                name: "ToggleItem(Light1)",
                cmd: Command::ToggleItem(ToggleItem::Light1),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x11, 0x00],
            },
            Case {
                name: "ToggleItem(Mister)",
                cmd: Command::ToggleItem(ToggleItem::Mister),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x0E, 0x00],
            },
            Case {
                name: "ToggleItem(CirculationPump)",
                cmd: Command::ToggleItem(ToggleItem::CirculationPump),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x3D, 0x00],
            },
            Case {
                name: "ToggleItem(Light3)",
                cmd: Command::ToggleItem(ToggleItem::Light3),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x13, 0x00],
            },
            Case {
                name: "ToggleItem(Light4)",
                cmd: Command::ToggleItem(ToggleItem::Light4),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x14, 0x00],
            },
            Case {
                name: "ToggleItem(Aux1)",
                cmd: Command::ToggleItem(ToggleItem::Aux1),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x16, 0x00],
            },
            Case {
                name: "ToggleItem(Aux2)",
                cmd: Command::ToggleItem(ToggleItem::Aux2),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x17, 0x00],
            },
            Case {
                name: "ToggleItem(SoakMode)",
                cmd: Command::ToggleItem(ToggleItem::SoakMode),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x1D, 0x00],
            },
            Case {
                name: "ToggleItem(NormalOperation)",
                cmd: Command::ToggleItem(ToggleItem::NormalOperation),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x01, 0x00],
            },
            Case {
                name: "ToggleItem(ClearNotification)",
                cmd: Command::ToggleItem(ToggleItem::ClearNotification),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x11, 0x03, 0x00],
            },
            Case {
                name: "SetTemperature(104)",
                cmd: Command::SetTemperature(104),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x20, 104],
            },
            Case {
                name: "SetTime 24h",
                cmd: Command::SetTime {
                    hour: 14,
                    minute: 30,
                    is_24h: true,
                },
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x21, 0x80 | 14, 30],
            },
            Case {
                name: "SetTime 12h",
                cmd: Command::SetTime {
                    hour: 9,
                    minute: 5,
                    is_24h: false,
                },
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x21, 9, 5],
            },
            Case {
                name: "SetTemperatureScale(celsius)",
                cmd: Command::SetTemperatureScale(true),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x27, 0x01, 0x01],
            },
            Case {
                name: "SetTemperatureScale(fahrenheit)",
                cmd: Command::SetTemperatureScale(false),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x27, 0x01, 0x00],
            },
            Case {
                name: "SettingsRequest(Panel)",
                cmd: Command::SettingsRequest(SettingsType::Panel),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x22, 0x00, 0x00, 0x01],
            },
            Case {
                name: "SettingsRequest(FilterCycles)",
                cmd: Command::SettingsRequest(SettingsType::FilterCycles),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x22, 0x01, 0x00, 0x00],
            },
            Case {
                name: "FilterCyclesRequest alias",
                cmd: Command::FilterCyclesRequest,
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x22, 0x01, 0x00, 0x00],
            },
            Case {
                name: "SettingsRequest(Information)",
                cmd: Command::SettingsRequest(SettingsType::Information),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x22, 0x02, 0x00, 0x00],
            },
            Case {
                name: "InformationRequest alias",
                cmd: Command::InformationRequest,
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x22, 0x02, 0x00, 0x00],
            },
            Case {
                name: "SettingsRequest(Preferences)",
                cmd: Command::SettingsRequest(SettingsType::Preferences),
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x22, 0x08, 0x00, 0x00],
            },
            Case {
                name: "FaultLogRequest{0xFF}",
                cmd: Command::FaultLogRequest { entry: 0xFF },
                expected_mt: [0x0A, 0xBF],
                expected_payload: vec![0x22, 0x20, 0xFF, 0x00],
            },
            Case {
                name: "NothingToSend{0x02}",
                cmd: Command::NothingToSend { client_id: 0x02 },
                expected_mt: [0x02, 0xBF],
                expected_payload: vec![0x07],
            },
        ];

        for (i, case) in cases.iter().enumerate() {
            let (mt, payload) = case.cmd.encode().expect("encode should succeed");
            assert_eq!(
                mt, case.expected_mt,
                "case {i} '{}': message_type mismatch",
                case.name
            );
            assert_eq!(
                payload, case.expected_payload,
                "case {i} '{}': payload mismatch",
                case.name
            );
        }
    }

    /// Temperature validation: in-range, boundary, out-of-range, and absolute limit cases.
    #[test]
    fn temperature_validation_table() {
        #[derive(Debug)]
        struct Case {
            name: &'static str,
            value: u8,
            scale: TemperatureScale,
            range: TempRange,
            expected: Result<u8, TempError>,
        }

        let cases = [
            // Fahrenheit high range
            Case {
                name: "F/High in-range (100)",
                value: 100,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Ok(100),
            },
            Case {
                name: "F/High min boundary (80)",
                value: 80,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Ok(80),
            },
            Case {
                name: "F/High max boundary (104)",
                value: 104,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Ok(104),
            },
            Case {
                name: "F/High below min (79)",
                value: 79,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Err(TempError::BelowMin),
            },
            Case {
                name: "F/High above max (105)",
                value: 105,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Err(TempError::AboveMax),
            },
            // Fahrenheit low range
            Case {
                name: "F/Low in-range (65)",
                value: 65,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::Low,
                expected: Ok(65),
            },
            Case {
                name: "F/Low min boundary (50)",
                value: 50,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::Low,
                expected: Ok(50),
            },
            Case {
                name: "F/Low max boundary (80)",
                value: 80,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::Low,
                expected: Ok(80),
            },
            // Celsius high range
            Case {
                name: "C/High in-range (38)",
                value: 38,
                scale: TemperatureScale::Celsius,
                range: TempRange::High,
                expected: Ok(38),
            },
            Case {
                name: "C/High min boundary (26)",
                value: 26,
                scale: TemperatureScale::Celsius,
                range: TempRange::High,
                expected: Ok(26),
            },
            Case {
                name: "C/High max boundary (40)",
                value: 40,
                scale: TemperatureScale::Celsius,
                range: TempRange::High,
                expected: Ok(40),
            },
            // Celsius low range
            Case {
                name: "C/Low min boundary (10)",
                value: 10,
                scale: TemperatureScale::Celsius,
                range: TempRange::Low,
                expected: Ok(10),
            },
            Case {
                name: "C/Low max boundary (26)",
                value: 26,
                scale: TemperatureScale::Celsius,
                range: TempRange::Low,
                expected: Ok(26),
            },
            // Absolute limit — between range max and absolute max
            Case {
                name: "F/High above max but below absolute (106)",
                value: 106,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Err(TempError::AboveMax),
            },
            // Absolute limit — exceeds hard cap
            Case {
                name: "F absolute limit exceeded (109)",
                value: 109,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Err(TempError::AboveAbsoluteLimit),
            },
            Case {
                name: "C absolute limit exceeded (43)",
                value: 43,
                scale: TemperatureScale::Celsius,
                range: TempRange::High,
                expected: Err(TempError::AboveAbsoluteLimit),
            },
            // Zero
            Case {
                name: "F zero rejected",
                value: 0,
                scale: TemperatureScale::Fahrenheit,
                range: TempRange::High,
                expected: Err(TempError::BelowMin),
            },
        ];

        for (i, case) in cases.iter().enumerate() {
            let result = validate_set_temperature(case.value, case.scale, case.range);
            assert_eq!(
                result, case.expected,
                "case {i} '{}': validate_set_temperature({}, {:?}, {:?})",
                case.name, case.value, case.scale, case.range
            );
        }
    }

    /// pump_index() / from_pump_index() and light_index() / from_light_index() round-trips.
    #[test]
    fn toggle_item_index_helpers() {
        // Pump index round-trip
        let pump_cases: [(ToggleItem, usize); 6] = [
            (ToggleItem::Pump1, 0),
            (ToggleItem::Pump2, 1),
            (ToggleItem::Pump3, 2),
            (ToggleItem::Pump4, 3),
            (ToggleItem::Pump5, 4),
            (ToggleItem::Pump6, 5),
        ];
        for (i, (item, expected_idx)) in pump_cases.iter().enumerate() {
            assert_eq!(
                item.pump_index(),
                Some(*expected_idx),
                "case {i}: {item:?}.pump_index()"
            );
            assert_eq!(
                ToggleItem::from_pump_index(*expected_idx),
                Some(*item),
                "case {i}: from_pump_index({expected_idx})"
            );
        }
        // Non-pumps return None for pump_index
        assert_eq!(ToggleItem::Light1.pump_index(), None);
        assert_eq!(ToggleItem::Blower.pump_index(), None);
        assert_eq!(ToggleItem::from_pump_index(6), None);

        // Light index round-trip
        let light_cases: [(ToggleItem, usize); 4] = [
            (ToggleItem::Light1, 0),
            (ToggleItem::Light2, 1),
            (ToggleItem::Light3, 2),
            (ToggleItem::Light4, 3),
        ];
        for (i, (item, expected_idx)) in light_cases.iter().enumerate() {
            assert_eq!(
                item.light_index(),
                Some(*expected_idx),
                "case {i}: {item:?}.light_index()"
            );
            assert_eq!(
                ToggleItem::from_light_index(*expected_idx),
                Some(*item),
                "case {i}: from_light_index({expected_idx})"
            );
        }
        // Non-lights return None for light_index
        assert_eq!(ToggleItem::Pump1.light_index(), None);
        assert_eq!(ToggleItem::Mister.light_index(), None);
        assert_eq!(ToggleItem::from_light_index(4), None);
    }

    /// SetTime validation: reject out-of-range hours, minutes, and bit-7-set hours.
    #[test]
    fn set_time_validation() {
        // Valid: boundary hour=23, minute=59
        let cmd = Command::SetTime {
            hour: 23,
            minute: 59,
            is_24h: false,
        };
        assert!(cmd.encode().is_ok());

        // Valid: hour=0, minute=0
        let cmd = Command::SetTime {
            hour: 0,
            minute: 0,
            is_24h: true,
        };
        assert!(cmd.encode().is_ok());

        // Invalid: hour=24
        let cmd = Command::SetTime {
            hour: 24,
            minute: 0,
            is_24h: false,
        };
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidHour(24)),
            "hour 24 should be rejected"
        );

        // Invalid: minute=60
        let cmd = Command::SetTime {
            hour: 12,
            minute: 60,
            is_24h: false,
        };
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidMinute(60)),
            "minute 60 should be rejected"
        );

        // Invalid: hour with bit 7 set (0x80)
        let cmd = Command::SetTime {
            hour: 0x80,
            minute: 0,
            is_24h: false,
        };
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidHour(0x80)),
            "hour with bit 7 set should be rejected"
        );

        // Invalid: hour with bit 7 set (0xFF)
        let cmd = Command::SetTime {
            hour: 0xFF,
            minute: 30,
            is_24h: true,
        };
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidHour(0xFF)),
            "hour 0xFF should be rejected"
        );

        // Invalid: extreme minute
        let cmd = Command::SetTime {
            hour: 10,
            minute: 0xFF,
            is_24h: false,
        };
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidMinute(0xFF)),
            "minute 0xFF should be rejected"
        );
    }

    /// SetTemperature validation: reject values above absolute max.
    #[test]
    fn set_temperature_validation() {
        // Valid: exactly at absolute max
        let cmd = Command::SetTemperature(ABSOLUTE_MAX_TEMP_F);
        assert!(cmd.encode().is_ok());

        // Valid: low value
        let cmd = Command::SetTemperature(80);
        assert!(cmd.encode().is_ok());

        // Invalid: one above absolute max
        let cmd = Command::SetTemperature(ABSOLUTE_MAX_TEMP_F + 1);
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidTemperature(ABSOLUTE_MAX_TEMP_F + 1)),
            "temp above absolute max should be rejected"
        );

        // Invalid: 0xFF
        let cmd = Command::SetTemperature(0xFF);
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidTemperature(0xFF)),
            "temp 0xFF should be rejected"
        );

        // Invalid: well above absolute max
        let cmd = Command::SetTemperature(200);
        assert_eq!(
            cmd.encode(),
            Err(CommandError::InvalidTemperature(200)),
            "temp 200 should be rejected"
        );
    }

    /// Verify that valid SetTime encodes the 24h flag correctly (bit 7 set on hour).
    #[test]
    fn set_time_24h_encoding() {
        let cmd = Command::SetTime {
            hour: 14,
            minute: 30,
            is_24h: true,
        };
        let (_, payload) = cmd.encode().unwrap();
        assert_eq!(payload, vec![0x21, 0x80 | 14, 30]);

        let cmd = Command::SetTime {
            hour: 14,
            minute: 30,
            is_24h: false,
        };
        let (_, payload) = cmd.encode().unwrap();
        assert_eq!(payload, vec![0x21, 14, 30]);
    }
}
