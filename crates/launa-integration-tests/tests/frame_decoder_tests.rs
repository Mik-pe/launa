//! Frame decoder integration tests.
//!
//! Tests for FrameDecoder edge cases: byte-at-a-time feeding, concatenated frames,
//! noise bytes between frames, bus idle, split boundaries, corrupt-then-valid
//! recovery, all-escape payloads, and frame encoding round-trips.

use launa_protocol::frame::{Frame, FrameDecoder};

#[test]
fn test_feed_bytes_one_at_a_time() {
    let mut sim = launa_sim::SpaSim::new();
    let encoded = sim.generate_status_frame();

    let mut decoder = FrameDecoder::new();
    let mut results = Vec::new();
    for &byte in &encoded {
        if let Some(frame) = decoder.feed(byte) {
            results.push(frame);
        }
    }

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].message_type, [0xFF, 0xAF]);
}

#[test]
fn test_multiple_concatenated_frames() {
    let mut sim = launa_sim::SpaSim::new();

    let status1 = sim.generate_status_frame();
    let _tick_bytes = sim.tick();
    let status2 = sim.generate_status_frame();
    let config = sim.generate_config_response();

    let mut all_bytes = Vec::new();
    all_bytes.extend_from_slice(&status1);
    all_bytes.extend_from_slice(&status2);
    all_bytes.extend_from_slice(&config);

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&all_bytes);

    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].message_type, [0xFF, 0xAF]);
    assert_eq!(frames[1].message_type, [0xFF, 0xAF]);
    assert_eq!(frames[2].message_type, [0x0A, 0xBF]);
}

#[test]
fn test_frames_with_noise_bytes_between() {
    let mut sim = launa_sim::SpaSim::new();

    let status = sim.generate_status_frame();
    let config = sim.generate_config_response();

    let mut all_bytes = Vec::new();
    all_bytes.extend_from_slice(&status);
    all_bytes.extend_from_slice(&[0x00, 0x00, 0x00]); // noise
    all_bytes.extend_from_slice(&config);
    all_bytes.extend_from_slice(&[0xAA, 0xBB]); // noise

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&all_bytes);

    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].message_type, [0xFF, 0xAF]);
    assert_eq!(frames[1].message_type, [0x0A, 0xBF]);
}

#[test]
fn test_frame_round_trip_encoding() {
    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0x42; 24],
    };
    let encoded = frame.encode().unwrap();

    assert_eq!(encoded.first(), Some(&0x7E));
    assert_eq!(encoded.last(), Some(&0x7E));

    let inner = &encoded[1..encoded.len() - 1];
    let decoded = Frame::parse(inner).unwrap();
    assert_eq!(decoded, frame);
}

#[test]
fn test_frame_decoder_bus_idle_0x7e() {
    let mut decoder = FrameDecoder::new();

    let idle_bytes = vec![0x7Eu8; 1000];
    let frames = decoder.feed_slice(&idle_bytes);

    assert_eq!(
        frames.len(),
        0,
        "1000 idle 0x7E bytes should not produce any frames"
    );

    assert_eq!(
        decoder.frame_error_count(),
        0,
        "idle 0x7E bytes should not cause frame errors"
    );

    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0x42; 24],
    };
    let encoded = frame.encode().unwrap();
    let valid_frames = decoder.feed_slice(&encoded);

    assert_eq!(
        valid_frames.len(),
        1,
        "valid frame after idle should decode"
    );
    assert_eq!(valid_frames[0].message_type, [0xFF, 0xAF]);
    assert_eq!(valid_frames[0].payload, vec![0x42; 24]);
}

#[test]
fn test_frame_decoder_split_every_boundary() {
    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
    };
    let encoded = frame.encode().unwrap();

    for split_at in 0..encoded.len() {
        let mut decoder = FrameDecoder::new();

        let first_part = &encoded[..split_at];
        let frames1 = decoder.feed_slice(first_part);
        assert!(
            frames1.is_empty(),
            "split_at={}: first part should not yield complete frames",
            split_at
        );

        let second_part = &encoded[split_at..];
        let frames2 = decoder.feed_slice(second_part);

        assert_eq!(
            frames2.len(),
            1,
            "split_at={}: second part should yield exactly one frame",
            split_at
        );
        assert_eq!(
            frames2[0].message_type,
            [0xFF, 0xAF],
            "split_at={}: message type should match",
            split_at
        );
        assert_eq!(
            frames2[0].payload,
            vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            "split_at={}: payload should match",
            split_at
        );
    }
}

#[test]
fn test_frame_decoder_corrupt_then_valid() {
    let mut decoder = FrameDecoder::new();

    let frame = Frame {
        message_type: [0x0A, 0xBF],
        payload: vec![0x01, 0x02, 0x03],
    };
    let mut corrupt_encoded = frame.encode().unwrap();
    let crc_idx = corrupt_encoded.len() - 2;
    corrupt_encoded[crc_idx] ^= 0xFF;

    let corrupt_frames = decoder.feed_slice(&corrupt_encoded);
    assert_eq!(
        corrupt_frames.len(),
        0,
        "corrupt frame should not produce a valid frame"
    );
    assert_eq!(
        decoder.frame_error_count(),
        1,
        "corrupt frame should increment frame error count"
    );

    let valid_frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0xAA, 0xBB, 0xCC],
    };
    let valid_encoded = valid_frame.encode().unwrap();
    let valid_frames = decoder.feed_slice(&valid_encoded);

    assert_eq!(
        valid_frames.len(),
        1,
        "valid frame after corrupt should decode"
    );
    assert_eq!(valid_frames[0].message_type, [0xFF, 0xAF]);
    assert_eq!(valid_frames[0].payload, vec![0xAA, 0xBB, 0xCC]);

    assert_eq!(
        decoder.frame_error_count(),
        1,
        "frame error count should still be 1 after valid frame"
    );
}

#[test]
fn test_frame_decoder_all_escape_payload() {
    let payload: Vec<u8> = vec![
        0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E,
        0x7D,
    ];

    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: payload.clone(),
    };
    let encoded = frame.encode().unwrap();

    assert!(
        encoded.iter().filter(|&&b| b == 0x7D).count() > 0,
        "encoded frame should contain escape sequences"
    );

    let inner = &encoded[1..encoded.len() - 1];
    let original_inner_len = 1 + 2 + payload.len() + 1;
    assert!(
        inner.len() > original_inner_len,
        "escaped inner content ({}) should be longer than original ({})",
        inner.len(),
        original_inner_len
    );

    let mut decoder = FrameDecoder::new();
    let decoded_frames = decoder.feed_slice(&encoded);

    assert_eq!(decoded_frames.len(), 1, "should decode exactly one frame");
    assert_eq!(
        decoded_frames[0].message_type,
        [0xFF, 0xAF],
        "message type should match"
    );
    assert_eq!(
        decoded_frames[0].payload, payload,
        "payload should match original (all escape bytes unescaped correctly)"
    );

    assert_eq!(decoder.frame_error_count(), 0);
}
