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
    pub fn encode(&self) -> Vec<u8> {
        // Length field = type(2) + payload length
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
        buf
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    TooShort(usize),
    Incomplete { expected: usize, got: usize },
    CrcMismatch { expected: u8, got: u8 },
}

/// Streaming frame decoder. Feed bytes one at a time; yields complete frames.
///
/// Handles HDLC-style byte stuffing: `0x7D` is the escape character.
/// Escaped bytes are XOR'd with `0x20` to recover the original value.
pub struct FrameDecoder {
    buffer: Vec<u8>,
    in_frame: bool,
    escape_next: bool,
    crc_error_count: u32,
}

impl FrameDecoder {
    pub fn new() -> Self {
        FrameDecoder {
            buffer: Vec::new(),
            in_frame: false,
            escape_next: false,
            crc_error_count: 0,
        }
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
                        self.crc_error_count += 1;
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

    /// Returns the total number of frames that failed CRC or other parse checks.
    pub fn crc_error_count(&self) -> u32 {
        self.crc_error_count
    }

    /// Returns the current CRC error count and resets it to zero.
    pub fn reset_crc_error_count(&mut self) -> u32 {
        let count = self.crc_error_count;
        self.crc_error_count = 0;
        count
    }
}

/// Encode a message type + payload into a complete framed byte sequence.
pub struct FrameEncoder;

impl FrameEncoder {
    /// Build a frame with the given message type and payload, returning the
    /// complete byte sequence including start/end markers.
    pub fn encode(message_type: [u8; 2], payload: &[u8]) -> Vec<u8> {
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
        let encoded = original.encode();
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
        let encoded = frame.encode();

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
    fn test_crc_error_count_on_bad_crc() {
        // Build a valid frame, then corrupt the CRC byte
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01, 0x02],
        };
        let mut encoded = frame.encode();
        // Corrupt the byte before the final 0x7E marker (the CRC)
        let crc_idx = encoded.len() - 2;
        encoded[crc_idx] ^= 0xFF;

        let mut decoder = FrameDecoder::new();
        for &byte in &encoded {
            decoder.feed(byte);
        }
        assert_eq!(decoder.crc_error_count(), 1);
    }

    #[test]
    fn test_crc_error_count_stays_zero_on_valid_frame() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x04],
        };
        let encoded = frame.encode();

        let mut decoder = FrameDecoder::new();
        for &byte in &encoded {
            decoder.feed(byte);
        }
        assert_eq!(decoder.crc_error_count(), 0);
    }

    #[test]
    fn test_crc_error_count_accumulates() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01],
        };
        let encoded = frame.encode();

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
        assert_eq!(decoder.crc_error_count(), 3);
    }

    #[test]
    fn test_reset_crc_error_count() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01],
        };
        let encoded = frame.encode();

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
        assert_eq!(decoder.crc_error_count(), 2);

        // Reset and verify return value + counter reset
        let returned = decoder.reset_crc_error_count();
        assert_eq!(returned, 2);
        assert_eq!(decoder.crc_error_count(), 0);
    }
}
