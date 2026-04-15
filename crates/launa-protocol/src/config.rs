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

        let mut pump_configs = [PumpConfig::None; 6];
        pump_configs[0] = decode_pump(payload[5] & 0x03);
        pump_configs[1] = decode_pump((payload[5] >> 2) & 0x03);
        pump_configs[2] = decode_pump((payload[5] >> 4) & 0x03);
        pump_configs[3] = decode_pump((payload[5] >> 6) & 0x03);
        pump_configs[4] = decode_pump(payload[6] & 0x03);
        pump_configs[5] = decode_pump((payload[6] >> 6) & 0x03);

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
}
