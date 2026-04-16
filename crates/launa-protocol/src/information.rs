/// Information response parser for `0A BF 24` messages.
///
/// Payload layout:
/// ```text
/// Offset: 0  1  2  3  4-11     12 13-16     17-18  19-20
/// Field:  SI SI SV SV SM(8B)   SU CS(4B)    HT HT  DS DS
/// ```
extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InformationResponse {
    /// Software ID string derived from SI+SV bytes (e.g. "M100_220 V17.0")
    pub software_id: String,
    /// System model string from 8 ASCII bytes, trimmed (e.g. "BFBP20")
    pub system_model: String,
    /// Current setup byte
    pub current_setup: u8,
    /// Configuration signature as hex string (e.g. "3D12382E")
    pub config_signature: String,
    /// Heater voltage
    pub heater_voltage: HeaterVoltage,
    /// Heater type
    pub heater_type: HeaterType,
    /// DIP switch settings as binary string
    pub dip_switches: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaterVoltage {
    Unknown(u8),
    V240,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaterType {
    Unknown(u8),
    Standard,
    // Other known variants can be added
}

impl HeaterVoltage {
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0x01 => HeaterVoltage::V240,
            other => HeaterVoltage::Unknown(other),
        }
    }
}

impl HeaterType {
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0x0A => HeaterType::Standard,
            other => HeaterType::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InformationError {
    UnexpectedLength(usize),
}

impl InformationResponse {
    /// Parse an information response from the frame payload.
    /// Message type is `0A BF 24`.
    /// Payload is 21 bytes (offsets 0-20).
    pub fn parse(payload: &[u8]) -> Result<Self, InformationError> {
        if payload.len() < 21 {
            return Err(InformationError::UnexpectedLength(payload.len()));
        }

        // Bytes 0-1: Software ID (SI) - two ASCII bytes
        let si_bytes = &payload[0..2];

        // Bytes 2-3: Software Version (SV) - two bytes
        let sv_bytes = &payload[2..4];

        // Derive software_id string from SI+SV using standard Balboa format:
        // SI bytes (raw decimal): "M{si0}_{si1}"
        // SV bytes (raw decimal): "V{sv0}.{sv1}"
        // Example: 0x64,0xDC,0x11,0x00 → "M100_220 V17.0"
        let software_id = format!(
            "M{}_{} V{}.{}",
            si_bytes[0], si_bytes[1], sv_bytes[0], sv_bytes[1]
        );

        // Bytes 4-11: System Model (8 ASCII bytes)
        let model_bytes = &payload[4..12];
        let system_model = String::from_utf8_lossy(model_bytes)
            .trim_end()
            .trim_end_matches('\0')
            .to_string();

        // Byte 12: Current Setup
        let current_setup = payload[12];

        // Bytes 13-16: Configuration Signature (4 bytes as hex)
        let config_signature = format!(
            "{:02X}{:02X}{:02X}{:02X}",
            payload[13], payload[14], payload[15], payload[16]
        );

        // Bytes 17-18: Heater Voltage, Heater Type
        let heater_voltage = HeaterVoltage::from_byte(payload[17]);
        let heater_type = HeaterType::from_byte(payload[18]);

        // Bytes 19-20: DIP Switch Settings (2 bytes as binary string)
        let dip_switches = format!("{:08b}{:08b}", payload[19], payload[20]);

        Ok(InformationResponse {
            software_id,
            system_model,
            current_setup,
            config_signature,
            heater_voltage,
            heater_type,
            dip_switches,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_information_response() {
        // Example from protocol doc:
        // 64dc 1100 4246425032302020 01 3d12382e 010a 0400
        let payload: &[u8] = &[
            0x64, 0xDC, 0x11, 0x00, // SI SI SV SV
            0x42, 0x46, 0x42, 0x50, 0x32, 0x30, 0x20, 0x20, // SM: "BFBP20  "
            0x01, // SU
            0x3D, 0x12, 0x38, 0x2E, // CS
            0x01, 0x0A, // HT: voltage=240V, type=Standard
            0x04, 0x00, // DS
        ];

        let info = InformationResponse::parse(payload).unwrap();
        assert_eq!(info.system_model, "BFBP20");
        assert_eq!(info.current_setup, 0x01);
        assert_eq!(info.config_signature, "3D12382E");
        assert_eq!(info.heater_voltage, HeaterVoltage::V240);
        assert_eq!(info.heater_type, HeaterType::Standard);
        assert_eq!(info.software_id, "M100_220 V17.0");
        assert_eq!(info.dip_switches, "0000010000000000");
    }

    #[test]
    fn test_parse_information_too_short() {
        let payload = [0u8; 10];
        let result = InformationResponse::parse(&payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_heater_voltage_unknown() {
        let v = HeaterVoltage::from_byte(0x05);
        assert_eq!(v, HeaterVoltage::Unknown(0x05));
    }

    #[test]
    fn test_heater_type_unknown() {
        let t = HeaterType::from_byte(0xFF);
        assert_eq!(t, HeaterType::Unknown(0xFF));
    }
}
