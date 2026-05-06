use crate::crc8;

const FRAME_MARKER: u8 = 0x7E;

/// A parsed Balboa protocol frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub message_type: [u8; 2],
    pub payload: Vec<u8>,
}

impl Frame {
    /// Parse a raw frame from bytes (excluding start/end 0x7E markers).
    /// Input should be the complete message body: [length, type_hi, type_lo, ..., checksum].
    ///
    /// Per the Balboa protocol, the Length byte counts *all* bytes between the
    /// two 0x7E delimiters (including itself and the trailing CRC).
    pub fn parse(data: &[u8]) -> Result<Self, FrameError> {
        if data.len() < 4 {
            return Err(FrameError::TooShort(data.len()));
        }

        let length = data[0] as usize;
        // Minimum valid frame: Length=4 (self + type(2) + CRC), so Length must be >= 4
        if length < 4 {
            return Err(FrameError::TooShort(length));
        }
        if data.len() < length {
            return Err(FrameError::Incomplete {
                expected: length,
                got: data.len(),
            });
        }

        // Length includes itself and the trailing CRC, so:
        //   body (CRC input) = data[0 .. length-1]  (length byte through last data byte)
        //   CRC               = data[length-1]
        let body = &data[..length - 1];
        let expected_crc = data[length - 1];
        let computed_crc = crc8::compute(body);

        if computed_crc != expected_crc {
            return Err(FrameError::CrcMismatch {
                expected: expected_crc,
                got: computed_crc,
            });
        }

        let message_type = [data[1], data[2]];
        // payload lies between message_type and the CRC
        let payload = data[3..length - 1].to_vec();

        Ok(Frame {
            message_type,
            payload,
        })
    }

    /// Encode this frame into raw bytes including start/end markers.
    ///
    /// The Balboa BP6013G1 does NOT use HDLC byte stuffing — frame body bytes
    /// are written literally between 0x7E delimiters. Frame boundaries are
    /// determined by the Length field, not by special-byte escaping.
    ///
    /// Returns `Err(FrameError::PayloadTooLarge(len))` if the payload exceeds
    /// 250 bytes (the maximum that fits in the u8 length field with overhead).
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        // Per the Balboa protocol, Length = 1(self) + 2(type) + payload + 1(CRC)
        // must fit in u8.
        if 4 + self.payload.len() > u8::MAX as usize {
            return Err(FrameError::PayloadTooLarge(self.payload.len()));
        }
        let length = (4 + self.payload.len()) as u8;

        let mut body = Vec::with_capacity(1 + 2 + self.payload.len() + 1);
        body.push(length);
        body.extend_from_slice(&self.message_type);
        body.extend_from_slice(&self.payload);

        let crc = crc8::compute(&body);
        body.push(crc);

        let mut buf = Vec::with_capacity(body.len() + 2);
        buf.push(FRAME_MARKER);
        buf.extend_from_slice(&body);
        buf.push(FRAME_MARKER);
        Ok(buf)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    TooShort(usize),
    Incomplete { expected: usize, got: usize },
    CrcMismatch { expected: u8, got: u8 },
    PayloadTooLarge(usize),
}

/// Streaming frame decoder. Feed bytes one at a time; yields complete frames.
///
/// The Balboa BP6013G1 does NOT use HDLC byte stuffing. Frame boundaries are
/// determined by the Length field: after seeing the start marker 0x7E, the
/// decoder reads the Length byte to know exactly how many bytes constitute the
/// frame body, then validates the end marker. This allows raw 0x7E bytes to
/// appear inside frame payloads without ambiguity.
pub struct FrameDecoder {
    buffer: Vec<u8>,
    in_frame: bool,
    expected_length: usize,
    frame_error_count: u32,
    max_buffer_size: usize,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub fn new() -> Self {
        FrameDecoder {
            buffer: Vec::new(),
            in_frame: false,
            expected_length: 0,
            frame_error_count: 0,
            max_buffer_size: 512,
        }
    }

    /// Builder method to set a custom max buffer size.
    pub fn with_max_buffer(mut self, size: usize) -> Self {
        self.max_buffer_size = size;
        self
    }

    /// Returns the configured max buffer size.
    pub fn max_buffer_size(&self) -> usize {
        self.max_buffer_size
    }

    /// Feed a single byte. Returns `Some(Frame)` when a complete frame is decoded.
    ///
    /// Uses length-field framing: once the first byte after the start marker is
    /// received (the Length byte), the decoder knows exactly how many more bytes
    /// to collect before checking for the end marker.
    pub fn feed(&mut self, byte: u8) -> Option<Frame> {
        if byte == FRAME_MARKER {
            if self.in_frame {
                if self.expected_length > 0 && self.buffer.len() >= self.expected_length {
                    // We have enough bytes — try to parse using length-field framing.
                    // Use only the bytes up to expected_length for the frame body.
                    let result = Frame::parse(&self.buffer[..self.expected_length]);
                    self.buffer.clear();
                    self.in_frame = false;
                    self.expected_length = 0;
                    match result {
                        Ok(frame) => Some(frame),
                        Err(_) => {
                            self.frame_error_count = self.frame_error_count.saturating_add(1);
                            None
                        }
                    }
                } else if self.expected_length > 0 && self.buffer.len() < self.expected_length {
                    // We know the expected length but haven't collected enough bytes yet.
                    // The protocol has no byte stuffing — 0x7E can appear as a literal
                    // body byte (e.g. temperature 0x7E = 126). Treat it as a data byte.
                    self.buffer.push(byte);
                    // If this was the last body byte, the real end marker comes next.
                    None
                } else if !self.buffer.is_empty() {
                    // No length known yet (only 1 byte buffered which is the length).
                    // A premature 0x7E means malformed data — treat as error.
                    self.buffer.clear();
                    self.in_frame = false;
                    self.expected_length = 0;
                    self.frame_error_count = self.frame_error_count.saturating_add(1);
                    // This 0x7E might be a new start marker, so start a new frame
                    self.in_frame = true;
                    None
                } else {
                    // Empty frame (consecutive 0x7E) — treat as new start
                    self.in_frame = true;
                    None
                }
            } else {
                // Start of a new frame
                self.in_frame = true;
                self.buffer.clear();
                self.expected_length = 0;
                None
            }
        } else if self.in_frame {
            self.buffer.push(byte);

            // Once we have the length byte, record it
            if self.buffer.len() == 1 {
                self.expected_length = byte as usize;
                // Validate: minimum frame length is 4 (length + type(2) + crc)
                if self.expected_length < 4 {
                    self.buffer.clear();
                    self.in_frame = false;
                    self.expected_length = 0;
                    self.frame_error_count = self.frame_error_count.saturating_add(1);
                    return None;
                }
                if self.expected_length > self.max_buffer_size {
                    self.buffer.clear();
                    self.in_frame = false;
                    self.expected_length = 0;
                    self.frame_error_count = self.frame_error_count.saturating_add(1);
                    return None;
                }
            }

            None
        } else {
            None
        }
    }

    /// Feed a slice of bytes, returning all decoded frames.
    ///
    /// Uses length-field framing for efficient slice processing: instead of
    /// feeding bytes one at a time, reads the length field and copies the
    /// body bytes in bulk, then validates the end marker.
    pub fn feed_slice(&mut self, data: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut i = 0;

        while i < data.len() {
            if !self.in_frame {
                // Scan for start marker
                if data[i] == FRAME_MARKER {
                    self.in_frame = true;
                    self.buffer.clear();
                    self.expected_length = 0;
                }
                i += 1;
                continue;
            }

            // We're in a frame. Need at least 1 byte for the length field.
            if self.expected_length == 0 {
                // Still waiting for the length byte
                if data[i] == FRAME_MARKER {
                    // Second marker before length — restart
                    self.buffer.clear();
                    self.expected_length = 0;
                    i += 1;
                    continue;
                }
                self.buffer.push(data[i]);
                self.expected_length = data[i] as usize;
                if self.expected_length < 4 || self.expected_length > self.max_buffer_size {
                    self.buffer.clear();
                    self.in_frame = false;
                    self.expected_length = 0;
                    self.frame_error_count = self.frame_error_count.saturating_add(1);
                }
                i += 1;
                continue;
            }

            // We know the expected length. Calculate how many body bytes we still need.
            let needed = self.expected_length - self.buffer.len();
            let available = data.len() - i;

            // Don't consume past the expected body bytes
            let to_copy = needed.min(available);
            self.buffer.extend_from_slice(&data[i..i + to_copy]);
            i += to_copy;

            // Check if we have the full body
            if self.buffer.len() < self.expected_length {
                continue;
            }

            // We have the full body. The next byte must be the end marker (0x7E).
            if i < data.len() {
                if data[i] == FRAME_MARKER {
                    let result = Frame::parse(&self.buffer[..self.expected_length]);
                    self.buffer.clear();
                    self.in_frame = false;
                    self.expected_length = 0;
                    match result {
                        Ok(frame) => frames.push(frame),
                        Err(_) => {
                            self.frame_error_count = self.frame_error_count.saturating_add(1);
                        }
                    }
                } else {
                    // Expected end marker but got something else — frame error
                    self.buffer.clear();
                    self.in_frame = false;
                    self.expected_length = 0;
                    self.frame_error_count = self.frame_error_count.saturating_add(1);
                }
                i += 1;
            }
            // If i == data.len(), we'll pick up the end marker on the next call
        }

        frames
    }

    /// Returns the total number of frames that failed parse checks (CRC, length, etc.).
    pub fn frame_error_count(&self) -> u32 {
        self.frame_error_count
    }

    /// Returns the current frame error count and resets it to zero.
    pub fn reset_frame_error_count(&mut self) -> u32 {
        let count = self.frame_error_count;
        self.frame_error_count = 0;
        count
    }
}

/// Encode a message type + payload into a complete framed byte sequence.
pub struct FrameEncoder;

impl FrameEncoder {
    /// Build a frame with the given message type and payload, returning the
    /// complete byte sequence including start/end markers.
    ///
    /// Returns `Err(FrameError::PayloadTooLarge(len))` if the payload exceeds
    /// 250 bytes (the maximum that fits in the u8 length field with overhead).
    pub fn encode(message_type: [u8; 2], payload: &[u8]) -> Result<Vec<u8>, FrameError> {
        let frame = Frame {
            message_type,
            payload: payload.to_vec(),
        };
        frame.encode()
    }
}

extern crate alloc;

use alloc::vec::Vec;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        let original = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x04],
        };
        let encoded = original.encode().unwrap();
        // Strip markers for parse
        let inner = &encoded[1..encoded.len() - 1];
        let decoded = Frame::parse(inner).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_decoder_streaming() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![],
        };
        let encoded = frame.encode().unwrap();

        let mut decoder = FrameDecoder::new();
        let mut results = Vec::new();
        for &byte in &encoded {
            if let Some(f) = decoder.feed(byte) {
                results.push(f);
            }
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message_type, [0x0A, 0xBF]);
    }

    #[test]
    fn test_frame_error_count_on_bad_crc() {
        // Build a valid frame, then corrupt the CRC byte
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01, 0x02],
        };
        let mut encoded = frame.encode().unwrap();
        // Corrupt the byte before the final 0x7E marker (the CRC)
        let crc_idx = encoded.len() - 2;
        encoded[crc_idx] ^= 0xFF;

        let mut decoder = FrameDecoder::new();
        for &byte in &encoded {
            decoder.feed(byte);
        }
        assert_eq!(decoder.frame_error_count(), 1);
    }

    #[test]
    fn test_frame_error_count_stays_zero_on_valid_frame() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x04],
        };
        let encoded = frame.encode().unwrap();

        let mut decoder = FrameDecoder::new();
        for &byte in &encoded {
            decoder.feed(byte);
        }
        assert_eq!(decoder.frame_error_count(), 0);
    }

    #[test]
    fn test_frame_error_count_accumulates() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01],
        };
        let encoded = frame.encode().unwrap();

        let mut decoder = FrameDecoder::new();

        // Feed 3 bad frames
        for _ in 0..3 {
            let mut bad = encoded.clone();
            let crc_idx = bad.len() - 2;
            bad[crc_idx] ^= 0xFF;
            for &byte in &bad {
                decoder.feed(byte);
            }
        }
        assert_eq!(decoder.frame_error_count(), 3);
    }

    #[test]
    fn test_reset_frame_error_count() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01],
        };
        let encoded = frame.encode().unwrap();

        let mut decoder = FrameDecoder::new();

        // Feed 2 bad frames
        for _ in 0..2 {
            let mut bad = encoded.clone();
            let crc_idx = bad.len() - 2;
            bad[crc_idx] ^= 0xFF;
            for &byte in &bad {
                decoder.feed(byte);
            }
        }
        assert_eq!(decoder.frame_error_count(), 2);

        // Reset and verify return value + counter reset
        let returned = decoder.reset_frame_error_count();
        assert_eq!(returned, 2);
        assert_eq!(decoder.frame_error_count(), 0);
    }

    #[test]
    fn test_decoder_default_max_buffer() {
        let decoder = FrameDecoder::new();
        assert_eq!(decoder.max_buffer_size(), 512);
    }

    #[test]
    fn test_decoder_custom_max_buffer() {
        let decoder = FrameDecoder::new().with_max_buffer(256);
        assert_eq!(decoder.max_buffer_size(), 256);
    }

    #[test]
    fn test_decoder_buffer_overflow_resets() {
        let mut decoder = FrameDecoder::new().with_max_buffer(10);

        // Start a frame with 0x7E, then feed garbage data beyond the limit
        decoder.feed(FRAME_MARKER);
        for i in 0..12 {
            decoder.feed(0x40 + (i % 10) as u8); // non-marker, non-escape bytes
        }

        assert_eq!(decoder.frame_error_count(), 1);
    }

    #[test]
    fn test_decoder_overflow_then_valid_frame() {
        let mut decoder = FrameDecoder::new().with_max_buffer(10);

        // Trigger overflow with garbage data
        decoder.feed(FRAME_MARKER);
        for i in 0..12 {
            decoder.feed(0x40 + (i % 10) as u8);
        }
        assert_eq!(decoder.frame_error_count(), 1);

        // Now feed a valid frame — should decode successfully
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x04],
        };
        let encoded = frame.encode().unwrap();

        let mut results = Vec::new();
        for &byte in &encoded {
            if let Some(f) = decoder.feed(byte) {
                results.push(f);
            }
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message_type, [0x0A, 0xBF]);
        assert_eq!(results[0].payload, vec![0x04]);
    }

    #[test]
    fn test_decoder_exact_buffer_fill_works() {
        // Create a frame whose inner content (between markers) is exactly 8 bytes
        // Frame inner: [length, type_hi, type_lo, payload..., crc] = 8 bytes
        // length = 4 + 4 = 8, payload = 4 bytes
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01, 0x02, 0x03, 0x04],
        };
        let encoded = frame.encode().unwrap();

        // Set max buffer to exactly the inner frame size
        // Count inner bytes (between markers)
        let inner_len = encoded.len() - 2; // strip start and end markers
        let mut decoder = FrameDecoder::new().with_max_buffer(inner_len);

        // Feed the encoded frame — should decode fine since buffer.len() == max (not >)
        let mut results = Vec::new();
        for &byte in &encoded {
            if let Some(f) = decoder.feed(byte) {
                results.push(f);
            }
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], frame);
        assert_eq!(decoder.frame_error_count(), 0);
    }

    #[test]
    fn test_encode_payload_too_large() {
        // 251-byte payload: length = 4 + 251 = 255 (fits), but 252: length = 256 > u8::MAX
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x00; 252],
        };
        let result = frame.encode();
        assert!(result.is_err());
        match result {
            Err(FrameError::PayloadTooLarge(len)) => assert_eq!(len, 252),
            other => panic!("expected PayloadTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn test_encode_max_payload_succeeds() {
        // 251-byte payload: length = 4 + 251 = 255 = u8::MAX (valid)
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x42; 251],
        };
        let result = frame.encode();
        assert!(
            result.is_ok(),
            "251-byte payload should fit in u8 length field"
        );
    }

    // --- CRC and parse edge cases ---

    #[test]
    fn test_parse_empty_data_returns_too_short() {
        let result = Frame::parse(&[]);
        assert!(result.is_err());
        match result {
            Err(FrameError::TooShort(0)) => {}
            other => panic!("expected TooShort(0), got {:?}", other),
        }
    }

    #[test]
    fn test_parse_single_byte_returns_too_short() {
        let result = Frame::parse(&[0x05]);
        assert!(result.is_err());
        match result {
            Err(FrameError::TooShort(1)) => {}
            other => panic!("expected TooShort(1), got {:?}", other),
        }
    }

    #[test]
    fn test_parse_three_bytes_returns_too_short() {
        // Minimum is 4 bytes: length + type_hi + type_lo + crc
        // Length = 4 means 4 total inner bytes (length + type(2) + crc)
        let result = Frame::parse(&[0x04, 0x0A, 0xBF]);
        assert!(result.is_err());
        match result {
            Err(FrameError::TooShort(3)) => {}
            other => panic!("expected TooShort(3), got {:?}", other),
        }
    }

    #[test]
    fn test_parse_incomplete_payload_returns_incomplete() {
        // Length byte says 7 total bytes, but only provide 5
        let data = [0x07, 0x0A, 0xBF, 0x01, 0x02];
        let result = Frame::parse(&data);
        assert!(result.is_err());
        match result {
            Err(FrameError::Incomplete {
                expected: 7,
                got: 5,
            }) => {}
            other => panic!("expected Incomplete(7, 5), got {:?}", other),
        }
    }

    #[test]
    fn test_parse_bad_crc_returns_mismatch() {
        // Build a valid frame then corrupt the CRC
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x42],
        };
        let encoded = frame.encode().unwrap();
        let inner = &encoded[1..encoded.len() - 1];

        let mut corrupted = inner.to_vec();
        // Corrupt last byte (CRC)
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xFF;

        let result = Frame::parse(&corrupted);
        assert!(result.is_err());
        match result {
            Err(FrameError::CrcMismatch { .. }) => {}
            other => panic!("expected CrcMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_empty_payload() {
        // Frame with no payload (empty payload)
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![],
        };
        let encoded = frame.encode().unwrap();
        let inner = &encoded[1..encoded.len() - 1];
        let decoded = Frame::parse(inner).unwrap();
        assert_eq!(decoded, frame);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn test_round_trip_single_byte_payload() {
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x99],
        };
        let encoded = frame.encode().unwrap();
        let inner = &encoded[1..encoded.len() - 1];
        let decoded = Frame::parse(inner).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_round_trip_large_payload() {
        // 200-byte payload (near max)
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: (0u8..200).collect(),
        };
        let encoded = frame.encode().unwrap();
        // Decode through the streaming decoder
        let mut decoder = FrameDecoder::new();
        let decoded = decoder.feed_slice(&encoded);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0], frame);
    }

    #[test]
    fn test_round_trip_with_special_bytes_no_escaping() {
        // Payload containing 0x7D (the old ESCAPE_CHAR).
        // With byte stuffing removed, 0x7D must be written literally — NOT as
        // the HDLC escape sequence 0x7D 0x5D.
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x7D, 0x00, 0x42],
        };
        let encoded = frame.encode().unwrap();
        // Verify markers exist at start and end
        assert_eq!(*encoded.first().unwrap(), FRAME_MARKER);
        assert_eq!(*encoded.last().unwrap(), FRAME_MARKER);

        // The body between markers must contain the raw 0x7D byte literally.
        // In the old HDLC encoding, 0x7D would have been encoded as [0x7D, 0x5D]
        // (2 bytes). Now it must be a single 0x7D byte.
        let inner = &encoded[1..encoded.len() - 1];
        // Verify no HDLC escape sequences present (0x7D followed by 0x5D or 0x5E)
        for window in inner.windows(2) {
            if window[0] == 0x7D {
                assert!(
                    window[1] != 0x5D && window[1] != 0x5E,
                    "HDLC escape sequence found in encoded output"
                );
            }
        }

        // Decode through the streaming decoder using feed_slice (length-field aware)
        let mut decoder = FrameDecoder::new();
        let results = decoder.feed_slice(&encoded);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], frame);
    }

    #[test]
    fn test_encoder_convenience_function() {
        let encoded = FrameEncoder::encode([0x0A, 0xBF], &[0x04]).unwrap();
        assert_eq!(*encoded.first().unwrap(), FRAME_MARKER);
        assert_eq!(*encoded.last().unwrap(), FRAME_MARKER);

        let inner = &encoded[1..encoded.len() - 1];
        let decoded = Frame::parse(inner).unwrap();
        assert_eq!(decoded.message_type, [0x0A, 0xBF]);
        assert_eq!(decoded.payload, vec![0x04]);
    }

    #[test]
    fn test_feed_slice_multiple_frames() {
        let frame1 = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01],
        };
        let frame2 = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x02],
        };
        let mut combined = frame1.encode().unwrap();
        combined.extend_from_slice(&frame2.encode().unwrap());

        let mut decoder = FrameDecoder::new();
        let results = decoder.feed_slice(&combined);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], frame1);
        assert_eq!(results[1], frame2);
    }

    #[test]
    fn test_decoder_inter_byte_gap_then_new_frame() {
        // Feed bytes between two frames — non-marker bytes outside a frame are ignored
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![],
        };
        let encoded = frame.encode().unwrap();

        let mut decoder = FrameDecoder::new();

        // Feed noise bytes (not in a frame)
        for &b in &[0x00, 0x01, 0x02] {
            assert!(decoder.feed(b).is_none());
        }

        // Feed the frame
        let mut results = Vec::new();
        for &byte in &encoded {
            if let Some(f) = decoder.feed(byte) {
                results.push(f);
            }
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], frame);
    }

    #[test]
    fn test_real_balboa_status_frame_decodes() {
        // Known-good real Balboa BP6013G1 status frame captured from the wire.
        // Full frame with CRC validated against launa-protocol crc8 implementation.
        // Inner body: length=29, type=[0xFF,0xAF], 25-byte payload, CRC=0xC2
        let raw: Vec<u8> = vec![
            0x7E, 0x1D, 0xFF, 0xAF, 0x13, 0x00, 0x00, 0x64, 0x07, 0x07, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00,
            0x00, 0xC2, 0x7E,
        ];

        let mut decoder = FrameDecoder::new();
        let results = decoder.feed_slice(&raw);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message_type, [0xFF, 0xAF]);
        // Length field = 0x1D = 29 = 1(length) + 2(type) + payload + 1(crc)
        // So payload = 29 - 4 = 25 bytes
        assert_eq!(results[0].payload.len(), 25);
        // First byte of payload is 0x13 (status subtype)
        assert_eq!(results[0].payload[0], 0x13);
        assert_eq!(decoder.frame_error_count(), 0);
    }

    #[test]
    fn test_encode_no_escape_for_0x7d_payload() {
        // Verify that 0x7D (the old ESCAPE_CHAR) is written literally in the output
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x7D],
        };
        let encoded = frame.encode().unwrap();
        let inner = &encoded[1..encoded.len() - 1];
        // The inner body must contain 0x7D as a literal byte
        assert!(
            inner.contains(&0x7D),
            "0x7D must appear literally in encoded body"
        );
    }

    #[test]
    fn test_encode_raw_body_matches_parse_input() {
        // Verify that encoded body bytes (between markers) are exactly what Frame::parse expects
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x13, 0x00, 0x00, 0x64, 0x07],
        };
        let encoded = frame.encode().unwrap();
        // The inner bytes should be directly parseable
        let inner = &encoded[1..encoded.len() - 1];
        let decoded = Frame::parse(inner).unwrap();
        assert_eq!(decoded, frame);
    }
}
