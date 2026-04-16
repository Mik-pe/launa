use crate::crc8;

const FRAME_MARKER: u8 = 0x7E;
const ESCAPE_CHAR: u8 = 0x7D;
const ESCAPED_MARKER: u8 = 0x5E;
const ESCAPED_ESCAPE: u8 = 0x5D;

/// A parsed Balboa protocol frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub message_type: [u8; 2],
    pub payload: Vec<u8>,
}

impl Frame {
    /// Parse a raw frame from bytes (excluding start/end 0x7E markers).
    /// Input should be the complete message body: [length, type_hi, type_lo, ..., checksum].
    pub fn parse(data: &[u8]) -> Result<Self, FrameError> {
        if data.len() < 4 {
            return Err(FrameError::TooShort(data.len()));
        }

        let length = data[0] as usize;
        if data.len() < length + 2 {
            return Err(FrameError::Incomplete {
                expected: length + 2,
                got: data.len(),
            });
        }

        // Verify checksum (covers length byte through last data byte, excludes checksum itself)
        let body = &data[..length + 1];
        let expected_crc = data[length + 1];
        let computed_crc = crc8::compute(body);

        if computed_crc != expected_crc {
            return Err(FrameError::CrcMismatch {
                expected: expected_crc,
                got: computed_crc,
            });
        }

        let message_type = [data[1], data[2]];
        let payload = data[3..length + 1].to_vec();

        Ok(Frame {
            message_type,
            payload,
        })
    }

    /// Encode this frame into raw bytes including start/end markers.
    ///
    /// Bytes inside the frame body that equal `0x7E` or `0x7D` are escaped
    /// using HDLC-style byte stuffing (`0x7D` followed by the byte XOR'd
    /// with `0x20`). The decoder un-stuffs them on the way in.
    ///
    /// Returns `Err(FrameError::PayloadTooLarge(len))` if the payload exceeds
    /// 253 bytes (the maximum that fits in the u8 length field).
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        // Length field = type(2) + payload length; must fit in u8
        if 2 + self.payload.len() > u8::MAX as usize {
            return Err(FrameError::PayloadTooLarge(self.payload.len()));
        }
        let length = (2 + self.payload.len()) as u8;

        let mut body = Vec::with_capacity(1 + 2 + self.payload.len() + 1);
        body.push(length);
        body.extend_from_slice(&self.message_type);
        body.extend_from_slice(&self.payload);

        let crc = crc8::compute(&body);
        body.push(crc);

        let mut buf = Vec::with_capacity(body.len() + 2 + body.len() / 8);
        buf.push(FRAME_MARKER);
        for &byte in &body {
            if byte == FRAME_MARKER {
                buf.push(ESCAPE_CHAR);
                buf.push(ESCAPED_MARKER);
            } else if byte == ESCAPE_CHAR {
                buf.push(ESCAPE_CHAR);
                buf.push(ESCAPED_ESCAPE);
            } else {
                buf.push(byte);
            }
        }
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
/// Handles HDLC-style byte stuffing: `0x7D` is the escape character.
/// Escaped bytes are XOR'd with `0x20` to recover the original value.
pub struct FrameDecoder {
    buffer: Vec<u8>,
    in_frame: bool,
    escape_next: bool,
    frame_error_count: u32,
    max_buffer_size: usize,
}

impl FrameDecoder {
    pub fn new() -> Self {
        FrameDecoder {
            buffer: Vec::new(),
            in_frame: false,
            escape_next: false,
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
    pub fn feed(&mut self, byte: u8) -> Option<Frame> {
        if byte == FRAME_MARKER {
            if self.in_frame && !self.buffer.is_empty() {
                let result = Frame::parse(&self.buffer);
                self.buffer.clear();
                self.in_frame = false;
                self.escape_next = false;
                match result {
                    Ok(frame) => Some(frame),
                    Err(_) => {
                        self.frame_error_count += 1;
                        None
                    }
                }
            } else {
                self.in_frame = true;
                self.buffer.clear();
                self.escape_next = false;
                None
            }
        } else if self.in_frame {
            if self.escape_next {
                // Un-stuff: XOR with 0x20
                self.buffer.push(byte ^ 0x20);
                self.escape_next = false;
            } else if byte == ESCAPE_CHAR {
                self.escape_next = true;
            } else {
                self.buffer.push(byte);
            }

            // Check buffer overflow after pushing
            if self.buffer.len() > self.max_buffer_size {
                self.buffer.clear();
                self.in_frame = false;
                self.escape_next = false;
                self.frame_error_count += 1;
            }

            None
        } else {
            None
        }
    }

    /// Feed a slice of bytes, returning all decoded frames.
    pub fn feed_slice(&mut self, data: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        for &byte in data {
            if let Some(frame) = self.feed(byte) {
                frames.push(frame);
            }
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
    /// 253 bytes (the maximum that fits in the u8 length field).
    pub fn encode(message_type: [u8; 2], payload: &[u8]) -> Result<Vec<u8>, FrameError> {
        let frame = Frame {
            message_type,
            payload: payload.to_vec(),
        };
        frame.encode()
    }
}

// Use alloc for Vec in no_std
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

        // Buffer should have been cleared on overflow, state reset, frame_error_count incremented
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
        // payload = 8 - 4 = 4 bytes
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
        // 254-byte payload: length = 2 + 254 = 256 > u8::MAX
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x00; 254],
        };
        let result = frame.encode();
        assert!(result.is_err());
        match result {
            Err(FrameError::PayloadTooLarge(len)) => assert_eq!(len, 254),
            other => panic!("expected PayloadTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn test_encode_max_payload_succeeds() {
        // 253-byte payload: length = 2 + 253 = 255 = u8::MAX (valid)
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x42; 253],
        };
        let result = frame.encode();
        assert!(
            result.is_ok(),
            "253-byte payload should fit in u8 length field"
        );
    }
}
