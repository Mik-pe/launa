//! MQTT packet extraction from a byte buffer.
//!
//! Provides heap-efficient extraction of complete MQTT packets from a
//! reassembly buffer. Uses `Vec::drain()` instead of double `Vec::from()`
//! to avoid allocating two new `Vec`s per packet extraction — critical on a
//! 32 KiB ESP32 heap.

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Decode an MQTT remaining-length field starting at byte index 1.
///
/// Returns `Some((remaining_length, header_size))` where `header_size`
/// is the total number of bytes in the fixed header (1 byte packet type +
/// N bytes remaining-length encoding).
///
/// Returns `None` if the buffer is too short or the encoding is malformed.
pub fn decode_remaining_length(buf: &[u8]) -> Option<(usize, usize)> {
    if buf.is_empty() {
        return None;
    }
    let mut multiplier = 1usize;
    let mut value = 0usize;
    let mut idx = 1;
    loop {
        if idx >= buf.len() {
            return None;
        }
        let byte = buf[idx];
        value += ((byte & 0x7F) as usize) * multiplier;
        multiplier *= 128;
        idx += 1;
        if (byte & 0x80) == 0 {
            break;
        }
        if multiplier > 128 * 128 * 128 * 128 {
            return None;
        }
    }
    Some((value, idx))
}

/// Attempt to extract a single complete MQTT packet from the front of `buffer`.
///
/// If a complete packet is available, drains its bytes from `buffer` and
/// returns them in a new `Vec`.  If the buffer is incomplete (not enough
/// bytes yet), returns `None` and leaves `buffer` unchanged.
///
/// This replaces the old double-`Vec::from()` pattern:
/// ```ignore
/// // OLD (two allocations):
/// let packet = Vec::from(&self.rx_buffer[..total_size]);
/// self.rx_buffer = Vec::from(&self.rx_buffer[total_size..]);
/// ```
/// With a single `drain()` that shifts the tail in-place:
/// ```ignore
/// // NEW (one allocation, in-place shift):
/// let packet: Vec<u8> = buffer.drain(..total_size).collect();
/// ```
pub fn try_extract_packet(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buffer.len() < 2 {
        return None;
    }
    let (remaining_len, header_size) = decode_remaining_length(buffer)?;
    let total_size = header_size + remaining_len;
    if buffer.len() >= total_size {
        let packet: Vec<u8> = buffer.drain(..total_size).collect();
        Some(packet)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a fake MQTT packet with given packet type byte and payload.
    /// Encodes the MQTT remaining-length field automatically.
    fn build_packet(packet_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.push(packet_type);
        encode_remaining_length_helper(&mut packet, payload.len());
        packet.extend_from_slice(payload);
        packet
    }

    /// Helper: encode remaining length (same algorithm as the app's version).
    fn encode_remaining_length_helper(buf: &mut Vec<u8>, mut len: usize) {
        loop {
            let mut byte = (len & 0x7F) as u8;
            len >>= 7;
            if len > 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if len == 0 {
                break;
            }
        }
    }

    // ── decode_remaining_length tests ────────────────────────────────

    #[test]
    fn test_decode_remaining_length_single_byte() {
        // Single byte: value = 0x05, header_size = 2 (type byte + 1 length byte)
        let buf = [0x30, 0x05];
        let (value, header_size) = decode_remaining_length(&buf).unwrap();
        assert_eq!(value, 5);
        assert_eq!(header_size, 2);
    }

    #[test]
    fn test_decode_remaining_length_two_byte() {
        // Two-byte encoding: 0x80 0x01 = (0 * 1) + (1 * 128) = 128
        let buf = [0x30, 0x80, 0x01];
        let (value, header_size) = decode_remaining_length(&buf).unwrap();
        assert_eq!(value, 128);
        assert_eq!(header_size, 3);
    }

    #[test]
    fn test_decode_remaining_length_three_byte() {
        // Three-byte encoding: 0x80 0x80 0x01 = 0 + 0 + (1 * 128 * 128) = 16384
        let buf = [0x30, 0x80, 0x80, 0x01];
        let (value, header_size) = decode_remaining_length(&buf).unwrap();
        assert_eq!(value, 16384);
        assert_eq!(header_size, 4);
    }

    #[test]
    fn test_decode_remaining_length_empty_buffer() {
        assert!(decode_remaining_length(&[]).is_none());
    }

    #[test]
    fn test_decode_remaining_length_single_byte_buffer() {
        // Only the packet type byte, no length byte
        assert!(decode_remaining_length(&[0x30]).is_none());
    }

    #[test]
    fn test_decode_remaining_length_incomplete_multi_byte() {
        // Continuation bit set but no next byte
        assert!(decode_remaining_length(&[0x30, 0x80]).is_none());
    }

    // ── try_extract_packet tests ─────────────────────────────────────

    #[test]
    fn test_try_extract_packet_empty_buffer() {
        let mut buf: Vec<u8> = Vec::new();
        assert!(try_extract_packet(&mut buf).is_none());
        assert!(buf.is_empty());
    }

    #[test]
    fn test_try_extract_packet_single_byte() {
        let mut buf = vec![0x30];
        assert!(try_extract_packet(&mut buf).is_none());
        assert_eq!(buf, vec![0x30]); // unchanged
    }

    #[test]
    fn test_try_extract_packet_partial_packet() {
        // Packet declares 5 bytes remaining but only 3 bytes of payload available
        let mut buf = vec![0x30, 0x05, 0x01, 0x02, 0x03]; // 5 - 2 header = need 3 more payload bytes = need total 7, have 5
        assert!(try_extract_packet(&mut buf).is_none());
        assert_eq!(buf, vec![0x30, 0x05, 0x01, 0x02, 0x03]); // unchanged
    }

    #[test]
    fn test_try_extract_packet_exact_complete() {
        let payload = vec![0xAA, 0xBB, 0xCC];
        let packet = build_packet(0x30, &payload);
        let mut buf = packet.clone();
        let extracted = try_extract_packet(&mut buf).unwrap();
        assert_eq!(extracted, packet);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_try_extract_packet_two_packets_in_sequence() {
        let packet1 = build_packet(0x30, &[0x01, 0x02]);
        let packet2 = build_packet(0x40, &[0x03, 0x04, 0x05]);

        let mut buf = Vec::new();
        buf.extend_from_slice(&packet1);
        buf.extend_from_slice(&packet2);

        // Extract first packet
        let extracted1 = try_extract_packet(&mut buf).unwrap();
        assert_eq!(extracted1, packet1);

        // Extract second packet
        let extracted2 = try_extract_packet(&mut buf).unwrap();
        assert_eq!(extracted2, packet2);

        // Buffer should be empty now
        assert!(buf.is_empty());
    }

    #[test]
    fn test_try_extract_packet_drain_preserves_remainder() {
        let packet = build_packet(0x30, &[0xAA]);
        let extra = vec![0xDE, 0xAD, 0xBE, 0xEF];

        let mut buf = Vec::new();
        buf.extend_from_slice(&packet);
        buf.extend_from_slice(&extra);

        let extracted = try_extract_packet(&mut buf).unwrap();
        assert_eq!(extracted, packet);
        assert_eq!(buf, extra);
    }

    #[test]
    fn test_try_extract_packet_multibyte_remaining_length() {
        // Two-byte remaining length encoding: remaining = 128
        let mut packet = vec![0x30]; // PUBLISH
        encode_remaining_length_helper(&mut packet, 128);
        let payload = vec![0xAB; 128];
        packet.extend_from_slice(&payload);

        let mut buf = packet.clone();
        let extracted = try_extract_packet(&mut buf).unwrap();
        assert_eq!(extracted, packet);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_try_extract_packet_multibyte_remaining_length_partial() {
        // Two-byte remaining length: remaining = 128, but only 10 bytes of payload
        let mut packet = vec![0x30];
        encode_remaining_length_helper(&mut packet, 128);
        packet.extend_from_slice(&[0xAB; 10]); // Only 10 of 128 bytes

        let mut buf = packet.clone();
        assert!(try_extract_packet(&mut buf).is_none());
        assert_eq!(buf, packet); // unchanged
    }

    #[test]
    fn test_try_extract_packet_zero_remaining_length() {
        // Packet with remaining length = 0 (e.g. PINGREQ: 0xC0 0x00)
        let packet = build_packet(0xC0, &[]);
        let mut buf = packet.clone();
        let extracted = try_extract_packet(&mut buf).unwrap();
        assert_eq!(extracted, vec![0xC0, 0x00]);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_try_extract_packet_three_packets_with_remainder() {
        let p1 = build_packet(0xC0, &[]); // PINGREQ
        let p2 = build_packet(0x30, &[0x01, 0x02, 0x03]);
        let p3 = build_packet(0x82, &[0x00, 0x01]); // SUBSCRIBE
        let remainder = vec![0xFF, 0xFE];

        let mut buf = Vec::new();
        buf.extend_from_slice(&p1);
        buf.extend_from_slice(&p2);
        buf.extend_from_slice(&p3);
        buf.extend_from_slice(&remainder);

        assert_eq!(try_extract_packet(&mut buf).unwrap(), p1);
        assert_eq!(try_extract_packet(&mut buf).unwrap(), p2);
        assert_eq!(try_extract_packet(&mut buf).unwrap(), p3);
        assert!(try_extract_packet(&mut buf).is_none()); // remainder is not a complete packet
        assert_eq!(buf, remainder);
    }

    #[test]
    fn test_try_extract_packet_large_remaining_length() {
        // Three-byte encoding: remaining = 16384
        let mut packet = vec![0x30];
        encode_remaining_length_helper(&mut packet, 16384);
        // Don't include any payload — should be partial
        let mut buf = packet.clone();
        assert!(try_extract_packet(&mut buf).is_none());
        assert_eq!(buf, packet); // unchanged
    }
}
