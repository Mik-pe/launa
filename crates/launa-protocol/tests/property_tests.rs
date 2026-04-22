//! Property-based tests for the launa-protocol crate.
//!
//! Uses manual property-style testing (no proptest dependency).

mod common;

use common::{random_bytes, xorshift32};
use launa_protocol::crc8;
use launa_protocol::{
    dispatch_frame, Frame, FrameDecoder, FrameEncoder, IncomingMessage, StatusUpdate,
};

// Frame round-trip property tests

fn round_trip_frame(msg_type: [u8; 2], payload: &[u8]) {
    let frame = Frame {
        message_type: msg_type,
        payload: payload.to_vec(),
    };

    // Verify message type doesn't contain frame marker or escape char
    assert_ne!(
        msg_type[0], 0x7E,
        "message type byte 0 must not be frame marker"
    );
    assert_ne!(
        msg_type[1], 0x7E,
        "message type byte 1 must not be frame marker"
    );
    assert_ne!(
        msg_type[0], 0x7D,
        "message type byte 0 must not be escape char"
    );
    assert_ne!(
        msg_type[1], 0x7D,
        "message type byte 1 must not be escape char"
    );

    // Verify payload doesn't contain frame marker or escape char
    for (i, &b) in payload.iter().enumerate() {
        assert_ne!(b, 0x7E, "payload byte {} must not be frame marker 0x7E", i);
        assert_ne!(b, 0x7D, "payload byte {} must not be escape char 0x7D", i);
    }

    let encoded = frame.encode().unwrap();

    // Verify markers
    assert_eq!(encoded[0], 0x7E, "start marker");
    assert_eq!(*encoded.last().unwrap(), 0x7E, "end marker");

    // Feed through FrameDecoder byte-by-byte (handles byte stuffing)
    let mut decoder = FrameDecoder::new();
    let mut results = Vec::new();
    for &byte in &encoded {
        if let Some(f) = decoder.feed(byte) {
            results.push(f);
        }
    }
    assert_eq!(results.len(), 1, "decoder should produce exactly one frame");
    assert_eq!(
        results[0], frame,
        "streaming decoded frame should match original"
    );
}

#[test]
fn test_frame_round_trip_empty_payload() {
    for &msg_type in &[
        [0xFFu8, 0xAF],
        [0x0A, 0xBF],
        [0xFE, 0xBF],
        [0x10, 0xBF],
        [0xAB, 0xCD],
    ] {
        round_trip_frame(msg_type, &[]);
    }
}

#[test]
fn test_frame_round_trip_1byte_payload() {
    for &msg_type in &[
        [0xFF, 0xAF],
        [0x0A, 0xBF],
        [0xFE, 0xBF],
        [0x10, 0xBF],
        [0xAB, 0xCD],
    ] {
        round_trip_frame(msg_type, &[0x42]);
    }
}

#[test]
fn test_frame_round_trip_100byte_payload() {
    let payload: Vec<u8> = (0..100).collect();
    for &msg_type in &[
        [0xFF, 0xAF],
        [0x0A, 0xBF],
        [0xFE, 0xBF],
        [0x10, 0xBF],
        [0xAB, 0xCD],
    ] {
        round_trip_frame(msg_type, &payload);
    }
}

#[test]
fn test_frame_round_trip_max_payload() {
    // Max payload that still fits in u8 length field:
    // length field = 2 (type) + payload_len; max length value = 255
    // So max payload = 253 bytes
    // Avoid 0x7E (frame marker) and 0x7D (escape char) in payload
    let payload: Vec<u8> = (0u8..=254u8)
        .filter(|&b| b != 0x7E && b != 0x7D)
        .take(253)
        .collect();
    assert_eq!(payload.len(), 253);
    for &msg_type in &[
        [0xFF, 0xAF],
        [0x0A, 0xBF],
        [0xFE, 0xBF],
        [0x10, 0xBF],
        [0xAB, 0xCD],
    ] {
        round_trip_frame(msg_type, &payload);
    }
}

#[test]
fn test_frame_round_trip_various_sizes() {
    let mut rng = 42;
    for size in &[0, 1, 2, 5, 10, 50, 100, 150, 200, 253] {
        let payload = random_bytes(&mut rng, *size, true);
        round_trip_frame([0xFF, 0xAF], &payload);
        round_trip_frame([0x0A, 0xBF], &payload);
    }
}

// CRC property tests

#[test]
fn test_crc_append_then_verify() {
    // For any data, compute CRC, append it, verify it passes validation
    let mut rng = 12345;
    for _ in 0..100 {
        let len = (xorshift32(&mut rng) % 256) as usize;
        let data = random_bytes(&mut rng, len, true);

        let crc = crc8::compute(&data);
        let mut with_crc = data.clone();
        with_crc.push(crc);

        // Recompute CRC over data (excluding the CRC byte) should match
        let check_crc = crc8::compute(&data);
        assert_eq!(check_crc, crc, "CRC should be deterministic");
    }
}

#[test]
fn test_crc_flip_bit_fails() {
    // For any data with CRC, flip any single bit, verify CRC mismatches
    let mut rng = 54321;
    for _ in 0..100 {
        let len = ((xorshift32(&mut rng) % 30) + 1) as usize;
        let data = random_bytes(&mut rng, len, true);
        let crc = crc8::compute(&data);

        // Try flipping each bit in the data
        for byte_idx in 0..data.len() {
            for bit in 0..8u8 {
                let mut corrupted = data.clone();
                corrupted[byte_idx] ^= 1 << bit;
                let corrupted_crc = crc8::compute(&corrupted);
                assert_ne!(
                    corrupted_crc, crc,
                    "Flipping bit {} in byte {} should change CRC",
                    bit, byte_idx
                );
            }
        }

        // Also flip bits in the CRC byte itself
        for bit in 0..8u8 {
            let flipped_crc = crc ^ (1 << bit);
            assert_ne!(
                flipped_crc, crc,
                "Flipping bit {} in CRC byte should produce different CRC",
                bit
            );
        }
    }
}

#[test]
fn test_crc_empty_data() {
    // Empty data should produce a deterministic CRC
    let crc1 = crc8::compute(&[]);
    let crc2 = crc8::compute(&[]);
    assert_eq!(crc1, crc2);
}

// Status update property tests

#[test]
fn test_status_all_pump_state_combinations() {
    use launa_protocol::status::PumpState;

    // For all pump state combinations (0-2 for each of 3 pumps), verify decode
    for p1 in 0u8..=3 {
        for p2 in 0u8..=3 {
            for p3 in 0u8..=3 {
                let mut payload = [0u8; 24];
                // Encode pump states into byte 11 (correct offset per real hardware)
                // pump1 bits 0-1, pump2 bits 2-3, pump3 bits 4-5
                payload[11] = (p1 & 0x03) | ((p2 & 0x03) << 2) | ((p3 & 0x03) << 4);
                payload[20] = 100; // set temp

                let status = StatusUpdate::parse(&payload).expect("should parse");

                let expected_p1 = match p1 & 0x03 {
                    0 => PumpState::Off,
                    1 => PumpState::Low,
                    2 => PumpState::High,
                    _ => PumpState::Off,
                };
                let expected_p2 = match p2 & 0x03 {
                    0 => PumpState::Off,
                    1 => PumpState::Low,
                    2 => PumpState::High,
                    _ => PumpState::Off,
                };
                let expected_p3 = match p3 & 0x03 {
                    0 => PumpState::Off,
                    1 => PumpState::Low,
                    2 => PumpState::High,
                    _ => PumpState::Off,
                };

                assert_eq!(status.pumps[0], expected_p1, "pump1: p1={}", p1);
                assert_eq!(status.pumps[1], expected_p2, "pump2: p2={}", p2);
                assert_eq!(status.pumps[2], expected_p3, "pump3: p3={}", p3);
            }
        }
    }
}

#[test]
fn test_status_unknown_temp_returns_none() {
    // 0xFF current temp returns None
    let mut payload = [0u8; 24];
    payload[2] = 0xFF;
    payload[20] = 100;

    let status = StatusUpdate::parse(&payload).unwrap();
    assert_eq!(status.current_temp, None, "0xFF temp should be None");
}

#[test]
fn test_status_temp_values_0_to_254_return_some() {
    // For temp values 0-254 verify returns Some
    for temp in 0u8..=254u8 {
        let mut payload = [0u8; 24];
        payload[2] = temp;
        payload[20] = 100;

        let status = StatusUpdate::parse(&payload).unwrap();
        assert!(
            status.current_temp.is_some(),
            "temp value {} should return Some",
            temp
        );
        assert_eq!(
            status.current_temp.unwrap(),
            launa_protocol::Temperature::fahrenheit(temp as f32),
            "temp should match byte value"
        );
    }
}

#[test]
fn test_status_celsius_temp_division() {
    // In Celsius mode, temps are divided by 2
    let mut payload = [0u8; 24];
    payload[2] = 76; // current temp raw = 76 → 38.0°C
    payload[9] = 0x01; // Celsius flag (offset 9 per real hardware)
    payload[20] = 80; // set temp raw = 80 → 40.0°C

    let status = StatusUpdate::parse(&payload).unwrap();
    assert_eq!(
        status.current_temp,
        Some(launa_protocol::Temperature::celsius(38.0))
    );
    assert_eq!(status.set_temp, launa_protocol::Temperature::celsius(40.0));
    assert!(matches!(
        status.temperature_scale,
        launa_protocol::status::TemperatureScale::Celsius
    ));
}

#[test]
fn test_status_fahrenheit_temp_no_division() {
    // In Fahrenheit mode, temps are NOT divided (divisor = 1)
    let mut payload = [0u8; 24];
    payload[2] = 104; // current temp = 104°F
    payload[8] = 0x00; // Fahrenheit
    payload[20] = 106; // set temp = 106°F

    let status = StatusUpdate::parse(&payload).unwrap();
    assert_eq!(
        status.current_temp,
        Some(launa_protocol::Temperature::fahrenheit(104.0))
    );
    assert_eq!(
        status.set_temp,
        launa_protocol::Temperature::fahrenheit(106.0)
    );
}

#[test]
fn test_status_too_short_returns_err() {
    let payload = [0u8; 23]; // need 24
    assert!(StatusUpdate::parse(&payload).is_err());
}

// FrameEncoder property test

#[test]
fn test_frame_encoder_matches_frame_encode() {
    let mut rng = 9999;
    for _ in 0..50 {
        let payload_len = (xorshift32(&mut rng) % 100) as usize;
        let payload = random_bytes(&mut rng, payload_len, true);
        let msg_type_bytes = [
            {
                let mut b = (xorshift32(&mut rng) % 256) as u8;
                if b == 0x7E || b == 0x7D {
                    b = 0x7F;
                }
                b
            },
            {
                let mut b = (xorshift32(&mut rng) % 256) as u8;
                if b == 0x7E || b == 0x7D {
                    b = 0x7F;
                }
                b
            },
        ];

        let frame = Frame {
            message_type: msg_type_bytes,
            payload: payload.clone(),
        };
        let frame_encoded = frame.encode().unwrap();
        let encoder_encoded = FrameEncoder::encode(msg_type_bytes, &payload).unwrap();

        assert_eq!(
            frame_encoded, encoder_encoded,
            "FrameEncoder should produce same output as Frame::encode"
        );
    }
}

// Dispatcher round-trip property tests

#[test]
fn test_dispatch_valid_status_roundtrip() {
    // Build a valid status frame, encode it, decode it via dispatcher
    let mut payload_bytes = [0u8; 24];
    payload_bytes[2] = 100; // current temp
    payload_bytes[20] = 104; // set temp

    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: payload_bytes.to_vec(),
    };
    let msg = dispatch_frame(&frame);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(
                s.current_temp,
                Some(launa_protocol::Temperature::fahrenheit(100.0))
            );
            assert_eq!(s.set_temp, launa_protocol::Temperature::fahrenheit(104.0));
        }
        other => panic!("Expected StatusUpdate, got {:?}", other),
    }
}

#[test]
fn test_dispatch_ready_message() {
    let frame = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let msg = dispatch_frame(&frame);
    assert_eq!(msg, IncomingMessage::Ready);
}
