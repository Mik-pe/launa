//! Hex encoding and decoding utilities.
//!
//! Provides `to_hex()` and `from_hex()` for converting between byte slices
//! and hexadecimal strings. Used by the ESP32 app for encrypted NVS values
//! and by OTA firmware verification.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Convert a byte slice to a lowercase hex string.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let hi = (b >> 4) & 0x0f;
        let lo = b & 0x0f;
        s.push(if hi < 10 {
            (b'0' + hi) as char
        } else {
            (b'a' + hi - 10) as char
        });
        s.push(if lo < 10 {
            (b'0' + lo) as char
        } else {
            (b'a' + lo - 10) as char
        });
    }
    s
}

/// Maximum hex string length accepted by `from_hex()`.
/// 1024 hex chars → 512 bytes decoded. More than enough for encrypted NVS
/// values (typically ~100 chars). Prevents OOM from malformed/corrupt input
/// on the 32 KiB ESP32 heap.
pub const MAX_HEX_LEN: usize = 1024;

/// Parse a hex string into a byte vector.
///
/// Returns `None` if the input has odd length, contains non-hex characters,
/// or exceeds `MAX_HEX_LEN` (prevents unbounded heap allocation).
pub fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || hex.len() > MAX_HEX_LEN {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let hi = hex.as_bytes()[i].to_ascii_lowercase();
        let lo = hex.as_bytes()[i + 1].to_ascii_lowercase();
        let h = match hi {
            b'0'..=b'9' => hi - b'0',
            b'a'..=b'f' => hi - b'a' + 10,
            _ => return None,
        };
        let l = match lo {
            b'0'..=b'9' => lo - b'0',
            b'a'..=b'f' => lo - b'a' + 10,
            _ => return None,
        };
        bytes.push(h << 4 | l);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_hex_empty() {
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn test_to_hex_single_byte() {
        assert_eq!(to_hex(&[0x00]), "00");
        assert_eq!(to_hex(&[0xff]), "ff");
        assert_eq!(to_hex(&[0x0f]), "0f");
        assert_eq!(to_hex(&[0xf0]), "f0");
    }

    #[test]
    fn test_to_hex_multiple_bytes() {
        assert_eq!(to_hex(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(
            to_hex(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]),
            "0123456789abcdef"
        );
    }

    #[test]
    fn test_from_hex_empty() {
        assert_eq!(from_hex(""), Some(alloc::vec![]));
    }

    #[test]
    fn test_from_hex_single_byte() {
        assert_eq!(from_hex("00"), Some(alloc::vec![0x00]));
        assert_eq!(from_hex("ff"), Some(alloc::vec![0xff]));
        assert_eq!(from_hex("0F"), Some(alloc::vec![0x0f]));
    }

    #[test]
    fn test_from_hex_multiple_bytes() {
        assert_eq!(
            from_hex("deadbeef"),
            Some(alloc::vec![0xde, 0xad, 0xbe, 0xef])
        );
    }

    #[test]
    fn test_round_trip() {
        let original: &[u8] = &[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let hex = to_hex(original);
        let decoded = from_hex(&hex).unwrap();
        assert_eq!(decoded.as_slice(), original);
    }

    #[test]
    fn test_round_trip_empty() {
        let hex = to_hex(&[]);
        assert_eq!(hex, "");
        let decoded = from_hex(&hex).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_from_hex_odd_length_rejected() {
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("12345"), None);
    }

    #[test]
    fn test_from_hex_invalid_chars() {
        assert_eq!(from_hex("gh"), None);
        assert_eq!(from_hex("xy"), None);
        assert_eq!(from_hex("z0"), None);
        assert_eq!(from_hex("!!"), None);
        assert_eq!(from_hex("0x"), None);
    }

    #[test]
    fn test_from_hex_uppercase_accepted() {
        assert_eq!(from_hex("DEAD"), Some(alloc::vec![0xde, 0xad]));
        assert_eq!(from_hex("AbCd"), Some(alloc::vec![0xab, 0xcd]));
    }

    #[test]
    fn test_from_hex_max_length() {
        // Exactly MAX_HEX_LEN chars should be accepted
        let input = "00".repeat(MAX_HEX_LEN / 2);
        assert!(from_hex(&input).is_some());
    }

    #[test]
    fn test_from_hex_exceeds_max_length() {
        // MAX_HEX_LEN + 2 chars should be rejected
        let input = "00".repeat(MAX_HEX_LEN / 2 + 1);
        assert!(from_hex(&input).is_none());
    }

    #[test]
    fn test_to_hex_all_byte_values() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let hex = to_hex(&bytes);
        assert_eq!(hex.len(), 512);
        let decoded = from_hex(&hex).unwrap();
        assert_eq!(decoded, bytes);
    }
}
