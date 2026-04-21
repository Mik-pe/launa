//! Shared pump bit-decode helpers.
//!
//! Pumps are encoded as 2-bit fields packed into two bytes:
//! - byte1 bits 0-1: pump 1, bits 2-3: pump 2, bits 4-5: pump 3, bits 6-7: pump 4
//! - byte2 bits 0-1: pump 5, bits 2-3: pump 6
//!
//! Raw values: 0 = off/none, 1 = low/single-speed, 2 = high/two-speed.

/// Extract 6 raw pump values (each 0-3) from the two packed pump bytes.
///
/// Returns an array `[u8; 6]` where index 0 = pump 1, index 5 = pump 6.
pub fn decode_pump_raw(byte1: u8, byte2: u8) -> [u8; 6] {
    [
        byte1 & 0x03,
        (byte1 >> 2) & 0x03,
        (byte1 >> 4) & 0x03,
        (byte1 >> 6) & 0x03,
        byte2 & 0x03,
        (byte2 >> 2) & 0x03,
    ]
}
