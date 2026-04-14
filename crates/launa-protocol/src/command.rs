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
    Blower,
    Light1,
    HoldMode,
    HeatingMode,
    TemperatureRange,
}

impl ToggleItem {
    pub fn code(self) -> u8 {
        match self {
            ToggleItem::Pump1 => 0x04,
            ToggleItem::Pump2 => 0x05,
            ToggleItem::Pump3 => 0x06,
            ToggleItem::Blower => 0x0C,
            ToggleItem::Light1 => 0x11,
            ToggleItem::HoldMode => 0x3C,
            ToggleItem::HeatingMode => 0x51,
            ToggleItem::TemperatureRange => 0x50,
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
    pub fn encode(&self) -> ([u8; 2], Vec<u8>) {
        match self {
            Command::ConfigurationRequest => ([0x0A, 0xBF], Vec::new()),
            Command::ToggleItem(item) => ([0x0A, 0xBF], vec![item.code(), 0x00]),
            Command::SetTemperature(temp) => ([0x0A, 0xBF], vec![*temp]),
            Command::SetTime { hour, minute, is_24h } => {
                let h = if *is_24h { hour | 0x80 } else { *hour };
                ([0x0A, 0xBF], vec![h, *minute])
            }
            Command::SetTemperatureScale(celsius) => {
                ([0x0A, 0xBF], vec![0x01, if *celsius { 0x01 } else { 0x00 }])
            }
            Command::SettingsRequest(SettingsType::Panel) => ([0x0A, 0xBF], vec![0x00, 0x00, 0x01]),
            Command::SettingsRequest(SettingsType::FilterCycles) | Command::FilterCyclesRequest => {
                ([0x0A, 0xBF], vec![0x01, 0x00, 0x00])
            }
            Command::SettingsRequest(SettingsType::Information) | Command::InformationRequest => {
                ([0x0A, 0xBF], vec![0x02, 0x00, 0x00])
            }
            Command::SettingsRequest(SettingsType::Preferences) => ([0x0A, 0xBF], vec![0x08, 0x00, 0x00]),
            Command::FaultLogRequest { entry } => ([0x0A, 0xBF], vec![0x20, *entry, 0x00]),
            Command::NothingToSend { client_id } => ([*client_id, 0xBF], Vec::new()),
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
    fn test_toggle_pump1() {
        let (mt, payload) = Command::ToggleItem(ToggleItem::Pump1).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x04, 0x00]);
    }

    #[test]
    fn test_set_temp() {
        let (mt, payload) = Command::SetTemperature(104).encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![104]);
    }

    #[test]
    fn test_set_time_24h() {
        let (mt, payload) = Command::SetTime { hour: 14, minute: 30, is_24h: true }.encode();
        assert_eq!(mt, [0x0A, 0xBF]);
        assert_eq!(payload, vec![0x80 | 14, 30]);
    }
}
