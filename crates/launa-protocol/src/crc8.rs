/// Balboa CRC-8 checksum.
///
/// Parameters:
/// - init: 0x02
/// - poly: 0x07
/// - reflect_in: false
/// - reflect_out: false
/// - xor_out: 0x02
pub fn compute(data: &[u8]) -> u8 {
    let mut crc: u8 = 0x02;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc ^ 0x02
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc_known_value() {
        // From protocol docs: message 1DFFAF13000064070700000100000400000000000000000064000000
        // should have CRC 0xC2
        let data: &[u8] = &[
            0x1D, 0xFF, 0xAF, 0x13, 0x00, 0x00, 0x64, 0x07,
            0x07, 0x00, 0x00, 0x01, 0x00, 0x00, 0x04, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x64, 0x00, 0x00, 0x00,
        ];
        assert_eq!(compute(data), 0xC2);
    }

    #[test]
    fn test_crc_empty() {
        // init=0x02, no bytes processed, xor_out=0x02 → 0x02 ^ 0x02 = 0x00
        assert_eq!(compute(&[]), 0x00);
    }

    #[test]
    fn test_crc_single_byte() {
        // init=0x02 ^ 0x00 = 0x02, process 8 bits with poly 0x07, then xor 0x02
        assert_eq!(compute(&[0x00]), 0x0C);
    }
}
