/// Known Balboa message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    // Incoming
    Ready,
    StatusUpdate,
    ConfigurationResponse,
    FilterCyclesResponse,
    InformationResponse,
    FaultLogResponse,
    ControlConfiguration,

    // Outgoing
    ConfigurationRequest,
    ToggleItem,
    SetTemperature,
    SetTime,
    SetTemperatureScale,
    SettingsRequest,
    SetWifiSettings,

    // Registration
    NewClientQuery,
    ClientIdRequest,
    ClientIdResponse,
    ClientIdAck,

    // Other
    NothingToSend,
    Unknown([u8; 2]),
}

impl MessageType {
    pub fn from_bytes(bytes: [u8; 2]) -> Self {
        match bytes {
            [0x10, 0xBF] => MessageType::Ready,
            [0xFF, 0xAF] => MessageType::StatusUpdate,
            [0x0A, 0xBF] => MessageType::Unknown([0x0A, 0xBF]), // disambiguated by context
            [0xFE, 0xBF] => MessageType::Unknown([0xFE, 0xBF]), // disambiguated by context
            _ => MessageType::Unknown(bytes),
        }
    }

    pub fn to_bytes(self) -> [u8; 2] {
        match self {
            MessageType::Ready => [0x10, 0xBF],
            MessageType::StatusUpdate => [0xFF, 0xAF],
            MessageType::ConfigurationResponse => [0x0A, 0xBF],
            MessageType::FilterCyclesResponse => [0x0A, 0xBF],
            MessageType::InformationResponse => [0x0A, 0xBF],
            MessageType::FaultLogResponse => [0x0A, 0xBF],
            MessageType::ControlConfiguration => [0x0A, 0xBF],
            MessageType::ConfigurationRequest => [0x0A, 0xBF],
            MessageType::ToggleItem => [0x0A, 0xBF],
            MessageType::SetTemperature => [0x0A, 0xBF],
            MessageType::SetTime => [0x0A, 0xBF],
            MessageType::SetTemperatureScale => [0x0A, 0xBF],
            MessageType::SettingsRequest => [0x0A, 0xBF],
            MessageType::SetWifiSettings => [0x0A, 0xBF],
            MessageType::NewClientQuery => [0xFE, 0xBF],
            MessageType::ClientIdRequest => [0xFE, 0xBF],
            MessageType::ClientIdResponse => [0xFE, 0xBF],
            MessageType::ClientIdAck => [0xFE, 0xBF],
            MessageType::NothingToSend => [0x00, 0xBF], // placeholder, uses client ID
            MessageType::Unknown(b) => b,
        }
    }
}
