/// Fault log response parser for `0A BF 28` messages.
///
/// Payload layout:
/// ```text
/// Offset: 0  1  2  3  4  5  6  7  8  9
/// Field:  FC EN MC DD HH MM FF ST TA TB
/// ```

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultLogEntry {
    /// Total number of entries in the controller's fault log.
    pub fault_count: u8,
    /// Sequential index of this entry within the log (1-based).
    pub entry_number: u8,
    /// Categorized fault code — see [`FaultCode`] for known values.
    pub message_code: FaultCode,
    pub days_ago: u8,
    pub hour: u8,
    pub minute: u8,
    /// Bitfield encoding the heating mode and temperature range at the time of fault.
    pub flags: u8,
    pub set_temperature: u8,
    pub sensor_a_temp: u8,
    pub sensor_b_temp: u8,
}

/// Known Balboa fault codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultCode {
    Sync,
    LowFlow,
    FlowFailed,
    SettingsReset,
    Priming,
    ClockFailed,
    ProgramMemory,
    SyncCallService,
    HeaterDry,
    HeaterMaybeDry,
    WaterTooHot,
    HeaterTooHot,
    SensorAFault,
    SensorBFault,
    PumpStuckOn,
    HotFault,
    GfciTestFailed,
    StandbyHold,
    /// Unknown fault code not in the known list
    Unknown(u8),
}

impl FaultCode {
    pub fn from_code(code: u8) -> Self {
        match code {
            15 => FaultCode::Sync,
            16 => FaultCode::LowFlow,
            17 => FaultCode::FlowFailed,
            18 => FaultCode::SettingsReset,
            19 => FaultCode::Priming,
            20 => FaultCode::ClockFailed,
            22 => FaultCode::ProgramMemory,
            26 => FaultCode::SyncCallService,
            27 => FaultCode::HeaterDry,
            28 => FaultCode::HeaterMaybeDry,
            29 => FaultCode::WaterTooHot,
            30 => FaultCode::HeaterTooHot,
            31 => FaultCode::SensorAFault,
            32 => FaultCode::SensorBFault,
            34 => FaultCode::PumpStuckOn,
            35 => FaultCode::HotFault,
            36 => FaultCode::GfciTestFailed,
            37 => FaultCode::StandbyHold,
            other => FaultCode::Unknown(other),
        }
    }

    /// Returns the numeric fault code value.
    pub fn code(&self) -> u8 {
        match self {
            FaultCode::Sync => 15,
            FaultCode::LowFlow => 16,
            FaultCode::FlowFailed => 17,
            FaultCode::SettingsReset => 18,
            FaultCode::Priming => 19,
            FaultCode::ClockFailed => 20,
            FaultCode::ProgramMemory => 22,
            FaultCode::SyncCallService => 26,
            FaultCode::HeaterDry => 27,
            FaultCode::HeaterMaybeDry => 28,
            FaultCode::WaterTooHot => 29,
            FaultCode::HeaterTooHot => 30,
            FaultCode::SensorAFault => 31,
            FaultCode::SensorBFault => 32,
            FaultCode::PumpStuckOn => 34,
            FaultCode::HotFault => 35,
            FaultCode::GfciTestFailed => 36,
            FaultCode::StandbyHold => 37,
            FaultCode::Unknown(code) => *code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultError {
    UnexpectedLength(usize),
}

impl FaultLogEntry {
    /// Parse a fault log entry from the frame payload.
    /// Message type is `0A BF 28`.
    /// Payload is 10 bytes (offsets 0-9).
    pub fn parse(payload: &[u8]) -> Result<Self, FaultError> {
        if payload.len() < 10 {
            return Err(FaultError::UnexpectedLength(payload.len()));
        }

        Ok(FaultLogEntry {
            fault_count: payload[0],
            entry_number: payload[1],
            message_code: FaultCode::from_code(payload[2]),
            days_ago: payload[3],
            hour: payload[4],
            minute: payload[5],
            flags: payload[6],
            set_temperature: payload[7],
            sensor_a_temp: payload[8],
            sensor_b_temp: payload[9],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fault_log_entry() {
        // Construct a realistic fault log entry
        let payload: &[u8] = &[
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

        let entry = FaultLogEntry::parse(payload).unwrap();
        assert_eq!(entry.fault_count, 3);
        assert_eq!(entry.entry_number, 1);
        assert_eq!(entry.message_code, FaultCode::HeaterDry);
        assert_eq!(entry.days_ago, 2);
        assert_eq!(entry.hour, 14);
        assert_eq!(entry.minute, 30);
        assert_eq!(entry.flags, 0x04);
        assert_eq!(entry.set_temperature, 104);
        assert_eq!(entry.sensor_a_temp, 104);
        assert_eq!(entry.sensor_b_temp, 102);
    }

    #[test]
    fn test_parse_fault_log_low_flow() {
        let payload: &[u8] = &[
            0x01, // fault count: 1
            0x01, // entry number: 1
            0x10, // message code: 16 = LowFlow
            0x00, // days ago: 0 (today)
            0x08, // hour: 8
            0x00, // minute: 0
            0x00, // flags
            0x68, // set temp: 104
            0x64, // sensor A: 100
            0x64, // sensor B: 100
        ];

        let entry = FaultLogEntry::parse(payload).unwrap();
        assert_eq!(entry.message_code, FaultCode::LowFlow);
        assert_eq!(entry.days_ago, 0);
    }

    #[test]
    fn test_parse_fault_too_short() {
        let payload = [0u8; 5];
        let result = FaultLogEntry::parse(&payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_fault_code_roundtrip() {
        let codes = [
            FaultCode::Sync,
            FaultCode::LowFlow,
            FaultCode::FlowFailed,
            FaultCode::SettingsReset,
            FaultCode::Priming,
            FaultCode::ClockFailed,
            FaultCode::ProgramMemory,
            FaultCode::SyncCallService,
            FaultCode::HeaterDry,
            FaultCode::HeaterMaybeDry,
            FaultCode::WaterTooHot,
            FaultCode::HeaterTooHot,
            FaultCode::SensorAFault,
            FaultCode::SensorBFault,
            FaultCode::PumpStuckOn,
            FaultCode::HotFault,
            FaultCode::GfciTestFailed,
            FaultCode::StandbyHold,
        ];

        for code in &codes {
            assert_eq!(FaultCode::from_code(code.code()), *code);
        }
    }

    #[test]
    fn test_unknown_fault_code() {
        let code = FaultCode::from_code(99);
        assert_eq!(code, FaultCode::Unknown(99));
        assert_eq!(code.code(), 99);
    }
}
