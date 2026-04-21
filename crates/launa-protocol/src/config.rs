/// Spa configuration response parser.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaConfig {
    pub pump_configs: [PumpConfig; 6],
    pub lights: [bool; 2],
    pub circ_pump: bool,
    pub blower: bool,
    pub mister: bool,
    pub aux1: bool,
    pub aux2: bool,
    pub temperature_scale_celsius: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpConfig {
    None,
    SingleSpeed,
    TwoSpeed,
}

impl SpaConfig {
    /// Parse configuration from a `0A BF 2E` response payload.
    /// Expected payload layout (starting after message type):
    /// ```text
    /// 0  1  2  3  4  5  6  7  8  9 ...
    /// ```
    pub fn parse(payload: &[u8]) -> Result<Self, ConfigError> {
        if payload.len() < 10 {
            return Err(ConfigError::UnexpectedLength(payload.len()));
        }

        let decode_pump = |bits: u8| match bits {
            0 => PumpConfig::None,
            1 => PumpConfig::SingleSpeed,
            2 => PumpConfig::TwoSpeed,
            _ => PumpConfig::None,
        };

        let raw_pumps = crate::pump_bits::decode_pump_raw(payload[5], payload[6]);
        let mut pump_configs = [PumpConfig::None; 6];
        for (i, &raw) in raw_pumps.iter().enumerate() {
            pump_configs[i] = decode_pump(raw);
        }

        Ok(SpaConfig {
            pump_configs,
            lights: [
                (payload[7] & 0x03) != 0, // light1
                (payload[7] & 0x0C) != 0, // light2
            ],
            circ_pump: (payload[8] & 0x80) != 0,
            blower: (payload[8] & 0x03) != 0,
            mister: (payload[9] & 0x30) != 0,
            aux1: payload[9] & 0x01 != 0,
            aux2: payload[9] & 0x02 != 0,
            temperature_scale_celsius: payload[3] & 0x01 != 0,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    UnexpectedLength(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config_two_pumps() {
        let mut payload = vec![0u8; 10];
        // Pump1=2-speed (bits 0-1 = 10), Pump2=1-speed (bits 2-3 = 01)
        payload[5] = 0b00_00_01_10;
        // Circ pump bit set, blower = single speed
        payload[8] = 0x80 | 0x01;
        // Light1 present
        payload[7] = 0x01;

        let config = SpaConfig::parse(&payload).unwrap();
        assert_eq!(config.pump_configs[0], PumpConfig::TwoSpeed);
        assert_eq!(config.pump_configs[1], PumpConfig::SingleSpeed);
        assert!(config.circ_pump);
        assert!(config.blower);
        assert!(config.lights[0]);
    }

    #[test]
    fn test_parse_config_pump5_correct_bits() {
        // Pump5 is encoded in bits 2-3 of payload[6] (not bits 6-7).
        // The sequential 2-bit packing pattern: pumps 0-3 in payload[5],
        // pump4 in bits 0-1 of payload[6], pump5 in bits 2-3 of payload[6].
        let mut payload = vec![0u8; 10];

        // Set pump5 = TwoSpeed (value 2) in bits 2-3 of payload[6]
        payload[6] = 0b00_00_10_00; // bits 2-3 = 10 (TwoSpeed)

        let config = SpaConfig::parse(&payload).unwrap();

        // pump4 (bits 0-1) should be None (00)
        assert_eq!(config.pump_configs[4], PumpConfig::None);
        // pump5 (bits 2-3) should be TwoSpeed (10)
        assert_eq!(config.pump_configs[5], PumpConfig::TwoSpeed);

        // Also verify SingleSpeed in bits 2-3
        payload[6] = 0b00_00_01_00; // bits 2-3 = 01 (SingleSpeed)
        let config = SpaConfig::parse(&payload).unwrap();
        assert_eq!(config.pump_configs[5], PumpConfig::SingleSpeed);

        // And None in bits 2-3
        payload[6] = 0b00_00_00_00;
        let config = SpaConfig::parse(&payload).unwrap();
        assert_eq!(config.pump_configs[5], PumpConfig::None);
    }
}
