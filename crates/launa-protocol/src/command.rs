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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn encode(&self) -> ([u8; 2], Vec<u8>) {
        match self {
            Command::ConfigurationRequest => ([0x0A, 0xBF], vec![0x04]),
            Command::ToggleItem(item) => ([0x0A, 0xBF], vec![0x11, item.code(), 0x00]),
            Command::SetTemperature(temp) => ([0x0A, 0xBF], vec![0x20, *temp]),
            Command::SetTime {
                hour,
                minute,
                is_24h,
            } => {
                let h = if *is_24h { hour | 0x80 } else { *hour };
                ([0x0A, 0xBF], vec![0x21, h, *minute])
            }
            Command::SetTemperatureScale(celsius) => {
                let ts = if *celsius { 0x01 } else { 0x00 };
                ([0x0A, 0xBF], vec![0x27, 0x01, ts])
            }
            Command::SettingsRequest(SettingsType::Panel) => {
                ([0x0A, 0xBF], vec![0x22, 0x00, 0x00, 0x01])
            }
            Command::SettingsRequest(SettingsType::FilterCycles) | Command::FilterCyclesRequest => {
                ([0x0A, 0xBF], vec![0x22, 0x01, 0x00, 0x00])
            }
            Command::SettingsRequest(SettingsType::Information) | Command::InformationRequest => {
                ([0x0A, 0xBF], vec![0x22, 0x02, 0x00, 0x00])
            }
            Command::SettingsRequest(SettingsType::Preferences) => {
                ([0x0A, 0xBF], vec![0x22, 0x08, 0x00, 0x00])
            }
            Command::FaultLogRequest { entry } => ([0x0A, 0xBF], vec![0x22, 0x20, *entry, 0x00]),
            Command::NothingToSend { client_id } => ([*client_id, 0xBF], vec![0x07]),
        }
    }
}

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_configuration_request() {
        let (mt, payload) = Command::ConfigurationRequest.encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x04]);
    }

    #[test]
    fn test_toggle_pump1() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Pump1).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x04, 0x00]);
    }

    #[test]
    fn test_toggle_light1() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Light1).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x11, 0x00]);
    }

    #[test]
    fn test_set_temp() {
        let (mt, payload) = Command::SetTemperature(104).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x20, 104]);
    }

    #[test]
    fn test_set_time_24h() {
        let (mt, payload) = Command::SetTime {
            hour: 14,
            minute: 30,
            is_24h: true,
        }
        .encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x21, 0x80 | 14, 30]);
    }

    #[test]
    fn test_set_time_12h() {
        let (mt, payload) = Command::SetTime {
            hour: 9,
            minute: 5,
            is_24h: false,
        }
        .encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x21, 9, 5]);
    }

    #[test]
    fn test_set_temp_scale_celsius() {
        let (mt, payload) = Command::SetTemperatureScale(true).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x27, 0x01, 0x01]);
    }

    #[test]
    fn test_set_temp_scale_fahrenheit() {
        let (mt, payload) = Command::SetTemperatureScale(false).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x27, 0x01, 0x00]);
    }

    #[test]
    fn test_settings_request_panel() {
        let (mt, payload) = Command::SettingsRequest(SettingsType::Panel).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x22, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn test_settings_request_filter_cycles() {
        let (mt, payload) = Command::SettingsRequest(SettingsType::FilterCycles).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x22, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn test_filter_cycles_request_alias() {
        let (mt, payload) = Command::FilterCyclesRequest.encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x22, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn test_settings_request_information() {
        let (mt, payload) = Command::SettingsRequest(SettingsType::Information).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x22, 0x02, 0x00, 0x00]);
    }

    #[test]
    fn test_information_request_alias() {
        let (mt, payload) = Command::InformationRequest.encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x22, 0x02, 0x00, 0x00]);
    }

    #[test]
    fn test_settings_request_preferences() {
        let (mt, payload) = Command::SettingsRequest(SettingsType::Preferences).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x22, 0x08, 0x00, 0x00]);
    }

    #[test]
    fn test_fault_log_request() {
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x22, 0x20, 0xFF, 0x00]);
    }

    #[test]
    fn test_nothing_to_send() {
        let (mt, payload) = Command::NothingToSend { client_id: 0x02 }.encode();
        assert_eq!(mt, [0x02, 0xBF]);
        assert_eq!(payload, vec![0x07]);
    }

    // --- Temperature validation tests ---

    #[test]
    fn test_validate_temp_fahrenheit_high_in_range() {
        assert_eq!(
            validate_set_temperature(100, TemperatureScale::Fahrenheit, TempRange::High),
            Ok(100)
        );
    }

    #[test]
    fn test_validate_temp_fahrenheit_high_min_boundary() {
        assert_eq!(
            validate_set_temperature(80, TemperatureScale::Fahrenheit, TempRange::High),
            Ok(80)
        );
    }

    #[test]
    fn test_validate_temp_fahrenheit_high_max_boundary() {
        assert_eq!(
            validate_set_temperature(104, TemperatureScale::Fahrenheit, TempRange::High),
            Ok(104)
        );
    }

    #[test]
    fn test_validate_temp_fahrenheit_high_below_min() {
        assert_eq!(
            validate_set_temperature(79, TemperatureScale::Fahrenheit, TempRange::High),
            Err(TempError::BelowMin)
        );
    }

    #[test]
    fn test_validate_temp_fahrenheit_high_above_max() {
        assert_eq!(
            validate_set_temperature(105, TemperatureScale::Fahrenheit, TempRange::High),
            Err(TempError::AboveMax)
        );
    }

    #[test]
    fn test_validate_temp_fahrenheit_low_in_range() {
        assert_eq!(
            validate_set_temperature(65, TemperatureScale::Fahrenheit, TempRange::Low),
            Ok(65)
        );
    }

    #[test]
    fn test_validate_temp_fahrenheit_low_boundaries() {
        assert_eq!(
            validate_set_temperature(50, TemperatureScale::Fahrenheit, TempRange::Low),
            Ok(50)
        );
        assert_eq!(
            validate_set_temperature(80, TemperatureScale::Fahrenheit, TempRange::Low),
            Ok(80)
        );
    }

    #[test]
    fn test_validate_temp_celsius_high_in_range() {
        assert_eq!(
            validate_set_temperature(38, TemperatureScale::Celsius, TempRange::High),
            Ok(38)
        );
    }

    #[test]
    fn test_validate_temp_celsius_high_boundaries() {
        assert_eq!(
            validate_set_temperature(26, TemperatureScale::Celsius, TempRange::High),
            Ok(26)
        );
        assert_eq!(
            validate_set_temperature(40, TemperatureScale::Celsius, TempRange::High),
            Ok(40)
        );
    }

    #[test]
    fn test_validate_temp_celsius_low_boundaries() {
        assert_eq!(
            validate_set_temperature(10, TemperatureScale::Celsius, TempRange::Low),
            Ok(10)
        );
        assert_eq!(
            validate_set_temperature(26, TemperatureScale::Celsius, TempRange::Low),
            Ok(26)
        );
    }

    #[test]
    fn test_validate_temp_absolute_limit_fahrenheit() {
        // 108 is the absolute max - accepted in high range (80-104 won't accept it though)
        // It's above 104 (range max) but below 108 (absolute max)
        assert_eq!(
            validate_set_temperature(106, TemperatureScale::Fahrenheit, TempRange::High),
            Err(TempError::AboveMax)
        );
        // 109 exceeds absolute limit
        assert_eq!(
            validate_set_temperature(109, TemperatureScale::Fahrenheit, TempRange::High),
            Err(TempError::AboveAbsoluteLimit)
        );
    }

    #[test]
    fn test_validate_temp_absolute_limit_celsius() {
        assert_eq!(
            validate_set_temperature(43, TemperatureScale::Celsius, TempRange::High),
            Err(TempError::AboveAbsoluteLimit)
        );
    }

    #[test]
    fn test_validate_temp_zero_rejected() {
        assert_eq!(
            validate_set_temperature(0, TemperatureScale::Fahrenheit, TempRange::High),
            Err(TempError::BelowMin)
        );
    }

    // --- New ToggleItem variant code() tests ---

    #[test]
    fn test_toggle_mister_code() {
        assert_eq!(ToggleItem::Mister.code(), 0x0E);
    }

    #[test]
    fn test_toggle_circulation_pump_code() {
        assert_eq!(ToggleItem::CirculationPump.code(), 0x3D);
    }

    #[test]
    fn test_toggle_light3_code() {
        assert_eq!(ToggleItem::Light3.code(), 0x13);
    }

    #[test]
    fn test_toggle_light4_code() {
        assert_eq!(ToggleItem::Light4.code(), 0x14);
    }

    #[test]
    fn test_toggle_aux1_code() {
        assert_eq!(ToggleItem::Aux1.code(), 0x16);
    }

    #[test]
    fn test_toggle_aux2_code() {
        assert_eq!(ToggleItem::Aux2.code(), 0x17);
    }

    #[test]
    fn test_toggle_soak_mode_code() {
        assert_eq!(ToggleItem::SoakMode.code(), 0x1D);
    }

    #[test]
    fn test_toggle_normal_operation_code() {
        assert_eq!(ToggleItem::NormalOperation.code(), 0x01);
    }

    #[test]
    fn test_toggle_clear_notification_code() {
        assert_eq!(ToggleItem::ClearNotification.code(), 0x03);
    }

    // --- Wire frame encoding tests for new variants ---

    #[test]
    fn test_encode_toggle_mister() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Mister).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x0E, 0x00]);
    }

    #[test]
    fn test_encode_toggle_circulation_pump() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::CirculationPump).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x3D, 0x00]);
    }

    #[test]
    fn test_encode_toggle_light3() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Light3).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x13, 0x00]);
    }

    #[test]
    fn test_encode_toggle_light4() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Light4).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x14, 0x00]);
    }

    #[test]
    fn test_encode_toggle_aux1() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Aux1).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x16, 0x00]);
    }

    #[test]
    fn test_encode_toggle_aux2() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Aux2).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x17, 0x00]);
    }

    #[test]
    fn test_encode_toggle_soak_mode() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::SoakMode).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x1D, 0x00]);
    }

    #[test]
    fn test_encode_toggle_normal_operation() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::NormalOperation).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x01, 0x00]);
    }

    #[test]
    fn test_encode_toggle_clear_notification() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::ClearNotification).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x11, 0x03, 0x00]);
    }

    // --- light_index() extended for new lights ---

    #[test]
    fn test_light3_light_index() {
        assert_eq!(ToggleItem::Light3.light_index(), Some(2));
    }

    #[test]
    fn test_light4_light_index() {
        assert_eq!(ToggleItem::Light4.light_index(), Some(3));
    }

    #[test]
    fn test_from_light_index_extended() {
        assert_eq!(ToggleItem::from_light_index(2), Some(ToggleItem::Light3));
        assert_eq!(ToggleItem::from_light_index(3), Some(ToggleItem::Light4));
    }
}
