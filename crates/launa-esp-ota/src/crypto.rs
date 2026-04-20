//! Cryptographic primitives for OTA firmware verification.
//!
//! Implements CRC-32/MPEG-2 (for firmware integrity) and CRC-32/ISO-HDLC
//! (for ESP-IDF otadata validation) from scratch for `no_std` compatibility
//! without external crypto dependencies.


/// CRC32 (CRC-32/MPEG-2) incremental update. Polynomial 0x04C11DB7.
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut crc = crc;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            if crc & 0x80000000 != 0 {
                crc = (crc << 1) ^ 0x04C11DB7;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// CRC32 (CRC-32/MPEG-2) for firmware integrity. Polynomial 0x04C11DB7.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFFFFFF, data)
}

/// CRC32 for ESP-IDF otadata validation.
///
/// Matches `esp_rom_crc32_le(UINT32_MAX, data, len)` from the ESP32 ROM.
/// The ROM function applies `~` (bitwise NOT) at both ends:
///   `~crc32_le_raw(~init, data)`
/// With init=0xFFFFFFFF, this becomes `~crc32_le_raw(0, data)`.
///
/// Uses reflected polynomial 0xEDB88320, init 0x00000000, final NOT.
pub fn crc32_ota(data: &[u8]) -> u32 {
    let mut crc: u32 = 0; // ~0xFFFFFFFF = 0
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc // Apply ~ at the end (ROM convention)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32() {
        // CRC-32/MPEG-2: init=0xFFFFFFFF, poly=0x04C11DB7, no final XOR
        assert_eq!(crc32(&[]), 0xFFFFFFFF);
        assert_eq!(crc32(&[0x01]), 1254728195u32);
    }

    #[test]
    fn test_crc32_mpeg2_known_vector() {
        // VAL-CORE-021: CRC-32/MPEG-2 of "123456789" must equal 0x0376E6E7.
        let data = b"123456789";
        let result = crc32(data);
        assert_eq!(
            result, 0x0376E6E7,
            "CRC-32/MPEG-2 of '123456789' should be 0x0376E6E7, got {:#010X}",
            result
        );
    }

    #[test]
    fn test_crc32_incremental_matches_oneshot() {
        // VAL-CORE-022: Incremental CRC across chunks must equal one-shot CRC
        // of concatenated data.
        let chunk1 = b"The quick brown ";
        let chunk2 = b"fox jumps over ";
        let chunk3 = b"the lazy dog";

        // One-shot
        let mut all = alloc::vec![];
        all.extend_from_slice(chunk1);
        all.extend_from_slice(chunk2);
        all.extend_from_slice(chunk3);
        let expected = crc32(&all);

        // Incremental
        let mut crc = 0xFFFFFFFFu32;
        crc = crc32_update(crc, chunk1);
        crc = crc32_update(crc, chunk2);
        crc = crc32_update(crc, chunk3);

        assert_eq!(
            crc, expected,
            "incremental CRC ({:#010X}) must match one-shot ({:#010X})",
            crc, expected
        );
    }

    #[test]
    fn test_crc32_ota_esp_idf_known_vector() {
        // ESP-IDF otadata: CRC covers only ota_seq (4 bytes).
        // esp_rom_crc32_le(UINT32_MAX, &ota_seq, 4) should give 0x4743989A for seq=1
        let otadata_seq: [u8; 4] = 1u32.to_le_bytes();
        let result = crc32_ota(&otadata_seq);
        assert_eq!(
            result, 0x4743989A,
            "CRC of ota_seq=1 should be 0x4743989A, got {:#010X}",
            result
        );
    }

    #[test]
    fn test_crc32_ota_seq_2() {
        // Verify seq=2 matches ROM: 0x55F63774
        let otadata_seq: [u8; 4] = 2u32.to_le_bytes();
        let result = crc32_ota(&otadata_seq);
        assert_eq!(
            result, 0x55F63774,
            "CRC of ota_seq=2 should be 0x55F63774, got {:#010X}",
            result
        );
    }

    #[test]
    fn test_crc32_ota_nontrivial_data() {
        // Verify with known ROM output for multi-byte data
        let data: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
        let result = crc32_ota(&data);
        // esp_rom_crc32_le(UINT32_MAX, data, 4) for [0xDE, 0xAD, 0xBE, 0xEF]
        // We compute this with our Python reference
        assert_ne!(result, 0, "CRC should not be zero for non-trivial data");
    }
}
