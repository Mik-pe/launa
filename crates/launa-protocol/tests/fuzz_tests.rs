//! Fuzz-like tests for the launa-protocol crate.
//!
//! Tests resilience against random/edge-case inputs. Verifies that parsers
//! never panic on arbitrary input.

use launa_protocol::config::SpaConfig;
use launa_protocol::fault::FaultLogEntry;
use launa_protocol::filter::FilterCycles;
use launa_protocol::information::InformationResponse;
use launa_protocol::{dispatch_frame, Frame, FrameDecoder, IncomingMessage, StatusUpdate};

// ── Helper: simple deterministic PRNG (xorshift32) ──────────────────────

fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

fn random_bytes(state: &mut u32, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(xorshift32(state) as u8);
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// Random input resilience tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_frame_decoder_random_input_no_panic() {
    let mut rng = 0xDEADBEEF;
    let mut decoder = FrameDecoder::new();
    let data = random_bytes(&mut rng, 10_000);

    // Feed 10000 random bytes through FrameDecoder — verify it never panics
    for &byte in &data {
        let _ = decoder.feed(byte);
    }
}

#[test]
fn test_status_parse_random_input_no_panic() {
    let mut rng = 0xCAFEBABE;

    // Feed random payloads to StatusUpdate::parse — verify it never panics
    for _ in 0..1000 {
        let len = (xorshift32(&mut rng) % 50) as usize;
        let payload = random_bytes(&mut rng, len);
        let _ = StatusUpdate::parse(&payload);
    }
}

#[test]
fn test_spa_config_parse_random_input_no_panic() {
    let mut rng = 0x12345678;

    // Feed random payloads to SpaConfig::parse — verify it never panics
    for _ in 0..1000 {
        let len = (xorshift32(&mut rng) % 30) as usize;
        let payload = random_bytes(&mut rng, len);
        let _ = SpaConfig::parse(&payload);
    }
}

#[test]
fn test_dispatch_frame_random_input_no_panic() {
    let mut rng = 0xABCDEF01;

    // Feed random payloads to dispatch_frame — verify it never panics
    for _ in 0..1000 {
        let payload_len = (xorshift32(&mut rng) % 50) as usize;
        let payload = random_bytes(&mut rng, payload_len);
        let msg_type = [
            (xorshift32(&mut rng) % 256) as u8,
            (xorshift32(&mut rng) % 256) as u8,
        ];
        let frame = Frame {
            message_type: msg_type,
            payload,
        };
        let _msg = dispatch_frame(&frame);
    }
}

#[test]
fn test_information_response_random_input_no_panic() {
    let mut rng = 0x55555555;

    for _ in 0..1000 {
        let len = (xorshift32(&mut rng) % 50) as usize;
        let payload = random_bytes(&mut rng, len);
        let _ = InformationResponse::parse(&payload);
    }
}

#[test]
fn test_fault_log_random_input_no_panic() {
    let mut rng = 0xAAAAAAAA;

    for _ in 0..1000 {
        let len = (xorshift32(&mut rng) % 30) as usize;
        let payload = random_bytes(&mut rng, len);
        let _ = FaultLogEntry::parse(&payload);
    }
}

#[test]
fn test_filter_cycles_random_input_no_panic() {
    let mut rng = 0x33333333;

    for _ in 0..1000 {
        let len = (xorshift32(&mut rng) % 20) as usize;
        let payload = random_bytes(&mut rng, len);
        let _ = FilterCycles::parse(&payload);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Edge case tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_frame_with_length_zero() {
    // Frame with length=0: only type bytes, no payload beyond the type
    // length field = 0 means payload is empty and type bytes aren't included
    // Actually: length field = type(2) + payload_len, so length=0 is impossible for valid frames
    // But we still test it doesn't panic
    let data = [0x00, 0xFF, 0xAF, 0x00]; // length=0, type, crc
    let result = Frame::parse(&data);
    assert!(result.is_err());
}

#[test]
fn test_frame_with_length_255_but_short_data() {
    // Frame with length=255 but only 3 bytes of data total
    let data = [0xFF, 0x0A, 0xBF];
    let result = Frame::parse(&data);
    assert!(result.is_err());
}

#[test]
fn test_frame_valid_crc_garbage_data() {
    // Frame with valid CRC but garbage data
    use launa_protocol::crc8;

    let length = 5u8; // type(2) + payload(3)
    let msg_type = [0xFF, 0xAF];
    let payload = [0xDE, 0xAD, 0xBE];
    let mut body = vec![length];
    body.extend_from_slice(&msg_type);
    body.extend_from_slice(&payload);
    let crc = crc8::compute(&body);
    body.push(crc);

    let frame = Frame::parse(&body).unwrap();
    assert_eq!(frame.message_type, [0xFF, 0xAF]);
    assert_eq!(frame.payload, vec![0xDE, 0xAD, 0xBE]);
}

#[test]
fn test_status_all_bytes_0xff() {
    let payload = [0xFFu8; 24];
    let status = StatusUpdate::parse(&payload).unwrap();
    assert_eq!(status.current_temp, None); // 0xFF = unknown
                                           // Other fields just shouldn't panic
}

#[test]
fn test_status_all_bytes_0x00() {
    let payload = [0x00u8; 24];
    let status = StatusUpdate::parse(&payload).unwrap();
    assert_eq!(status.current_temp, Some(0.0));
    assert_eq!(status.set_temp, 0.0);
    assert_eq!(status.hour, 0);
    assert_eq!(status.minute, 0);
}

#[test]
fn test_very_long_payload_frame() {
    // Create a frame with a large payload (but still fitting in u8 length)
    // Max length = 255, so max payload = 253 (255 - 2 type bytes)
    let payload: Vec<u8> = (0..253u8).collect();
    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload,
    };
    let encoded = frame.encode().unwrap();

    // Use FrameDecoder to handle byte stuffing
    let mut decoder = FrameDecoder::new();
    let decoded = decoder.feed_slice(&encoded);
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0], frame);
}

#[test]
fn test_empty_payload_frame() {
    let frame = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![],
    };
    let encoded = frame.encode().unwrap();
    let inner = &encoded[1..encoded.len() - 1];
    let decoded = Frame::parse(inner).unwrap();
    assert_eq!(decoded, frame);
    assert!(decoded.payload.is_empty());
}

#[test]
fn test_frame_parse_too_short_0_bytes() {
    assert!(Frame::parse(&[]).is_err());
}

#[test]
fn test_frame_parse_too_short_1_byte() {
    assert!(Frame::parse(&[0x01]).is_err());
}

#[test]
fn test_frame_parse_too_short_3_bytes() {
    assert!(Frame::parse(&[0x05, 0xFF, 0xAF]).is_err());
}

#[test]
fn test_frame_bad_crc() {
    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0x01, 0x02],
    };
    let encoded = frame.encode().unwrap();
    let inner = &encoded[1..encoded.len() - 1];
    let mut corrupted = inner.to_vec();
    // Corrupt the CRC byte (last byte)
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0xFF;
    assert!(Frame::parse(&corrupted).is_err());
}

#[test]
fn test_frame_decoder_multiple_frames() {
    let frame1 = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0x01],
    };
    let frame2 = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x02, 0x03],
    };

    let mut decoder = FrameDecoder::new();
    let mut results = Vec::new();

    for &byte in &frame1.encode().unwrap() {
        if let Some(f) = decoder.feed(byte) {
            results.push(f);
        }
    }
    for &byte in &frame2.encode().unwrap() {
        if let Some(f) = decoder.feed(byte) {
            results.push(f);
        }
    }

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].message_type, [0xFF, 0xAF]);
    assert_eq!(results[1].message_type, [0x10, 0xBF]);
}

#[test]
fn test_frame_decoder_feed_slice() {
    let frame = Frame {
        message_type: [0x0A, 0xBF],
        payload: vec![0x04],
    };
    let encoded = frame.encode().unwrap();

    let mut decoder = FrameDecoder::new();
    let results = decoder.feed_slice(&encoded);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], frame);
}

#[test]
fn test_frame_decoder_inter_frame_garbage() {
    // Feed garbage between frame markers — should be ignored
    let frame = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let encoded = frame.encode().unwrap();

    let mut data = Vec::new();
    // Some garbage before the frame
    data.extend_from_slice(&[0x00, 0x01, 0x02]);
    // The actual frame
    data.extend_from_slice(&encoded);
    // More garbage
    data.extend_from_slice(&[0x03, 0x04]);

    let mut decoder = FrameDecoder::new();
    let results = decoder.feed_slice(&data);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].message_type, [0x10, 0xBF]);
}

#[test]
fn test_dispatch_all_zeros_frame() {
    let frame = Frame {
        message_type: [0x00, 0x00],
        payload: vec![0x00],
    };
    // Should not panic, should return Unknown
    let msg = dispatch_frame(&frame);
    assert!(matches!(msg, IncomingMessage::Unknown { .. }));
}

#[test]
fn test_dispatch_all_ff_frame() {
    let frame = Frame {
        message_type: [0xFF, 0xFF],
        payload: vec![0xFF, 0xFF, 0xFF],
    };
    let msg = dispatch_frame(&frame);
    assert!(matches!(msg, IncomingMessage::Unknown { .. }));
}

#[test]
fn test_status_exactly_24_bytes() {
    // Exactly minimum length should work
    let payload = [0u8; 24];
    assert!(StatusUpdate::parse(&payload).is_ok());
}

#[test]
fn test_status_more_than_24_bytes() {
    // More than minimum is fine
    let payload = [0u8; 30];
    assert!(StatusUpdate::parse(&payload).is_ok());
}

#[test]
fn test_spa_config_exactly_10_bytes() {
    let payload = [0u8; 10];
    assert!(SpaConfig::parse(&payload).is_ok());
}

#[test]
fn test_spa_config_less_than_10_bytes() {
    let payload = [0u8; 9];
    assert!(SpaConfig::parse(&payload).is_err());
}
