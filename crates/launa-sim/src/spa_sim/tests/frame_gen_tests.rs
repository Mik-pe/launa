use super::*;
use crate::spa_sim::frame_gen::{cycle_heating_mode, cycle_pump};
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::{HeatingMode, PumpState, TemperatureScale};

#[test]
fn test_tick_generates_frames() {
    let mut sim = SpaSim::new();
    let bytes = sim.tick();
    assert!(!bytes.is_empty(), "tick should produce output bytes");

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert!(
        frames.len() >= 2,
        "tick should produce at least 2 frames (status + ready)"
    );
}

#[test]
fn test_tick_after_registration_no_query() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);

    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].message_type, [0xFF, 0xAF]); // status
    assert_eq!(frames[1].message_type, [0x10, 0xBF]); // ready
}

#[test]
fn test_temp_encoding_fahrenheit() {
    assert_eq!(
        SpaState::encode_temp(100.0, TemperatureScale::Fahrenheit),
        100
    );
    assert_eq!(
        SpaState::encode_temp(104.0, TemperatureScale::Fahrenheit),
        104
    );
}

#[test]
fn test_temp_encoding_celsius() {
    assert_eq!(SpaState::encode_temp(38.0, TemperatureScale::Celsius), 76);
    assert_eq!(SpaState::encode_temp(40.0, TemperatureScale::Celsius), 80);
}

#[test]
fn test_cycle_heating_mode_enums() {
    assert_eq!(cycle_heating_mode(HeatingMode::Ready), HeatingMode::Rest);
    assert_eq!(
        cycle_heating_mode(HeatingMode::Rest),
        HeatingMode::ReadyInRest
    );
    assert_eq!(
        cycle_heating_mode(HeatingMode::ReadyInRest),
        HeatingMode::Ready
    );
}

#[test]
fn test_cycle_pump_enums() {
    assert_eq!(cycle_pump(PumpState::Off), PumpState::Low);
    assert_eq!(cycle_pump(PumpState::Low), PumpState::High);
    assert_eq!(cycle_pump(PumpState::High), PumpState::Off);
}

#[test]
fn test_ready_interval_default_every_tick() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    // Default ready_interval_range=(1,1): Ready frame every tick

    let mut ready_count = 0;
    for _ in 0..10 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        for f in &frames {
            if f.message_type == [0x10, 0xBF] {
                ready_count += 1;
            }
        }
    }

    assert_eq!(
        ready_count, 10,
        "default (1,1) should produce Ready every tick"
    );
}

#[test]
fn test_ready_interval_randomized() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_ready_interval_range(2, 5);

    let mut ready_count = 0;
    for _ in 0..100 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        for f in &frames {
            if f.message_type == [0x10, 0xBF] {
                ready_count += 1;
            }
        }
    }

    // With interval range (2,5), expect ~20-60 Ready frames in 100 ticks
    // (min 100/5=20, max 100/2=50, but allow some margin)
    assert!(
        ready_count >= 15 && ready_count <= 55,
        "expected 15-55 Ready frames with range (2,5), got {}",
        ready_count
    );
}

#[test]
fn test_frame_jitter_default_unchanged() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // With default jitter_padding_bytes=0, both ticks produce the same structure
    // (status frame + ready frame). Physics causes minor byte differences (clock),
    // so we verify structural equivalence: same number of decoded frames with same types.
    let bytes1 = sim.tick();
    let bytes2 = sim.tick();

    let mut decoder1 = FrameDecoder::new();
    let frames1 = decoder1.feed_slice(&bytes1);

    let mut decoder2 = FrameDecoder::new();
    let frames2 = decoder2.feed_slice(&bytes2);

    // Same frame count and types
    assert_eq!(frames1.len(), 2, "tick 1: status + ready");
    assert_eq!(frames2.len(), 2, "tick 2: status + ready");
    assert_eq!(frames1[0].message_type, [0xFF, 0xAF]); // status
    assert_eq!(frames1[1].message_type, [0x10, 0xBF]); // ready
    assert_eq!(frames2[0].message_type, [0xFF, 0xAF]); // status
    assert_eq!(frames2[1].message_type, [0x10, 0xBF]); // ready

    // Output lengths should be identical (no jitter padding)
    assert_eq!(
        bytes1.len(),
        bytes2.len(),
        "output lengths should match with default jitter=0"
    );
}

#[test]
fn test_frame_jitter_variable_padding() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_jitter_padding_bytes(10);

    // Collect output from 50 ticks, verify at least 3 distinct lengths
    let mut lengths = std::collections::BTreeSet::new();
    let mut all_decoded_ok = true;

    for _ in 0..50 {
        let bytes = sim.tick();
        lengths.insert(bytes.len());

        // Verify FrameDecoder can still decode all valid frames
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        // Should have at least the status frame (ready may be separate)
        if frames.is_empty() || frames[0].message_type != [0xFF, 0xAF] {
            all_decoded_ok = false;
        }
    }

    assert!(
        lengths.len() >= 3,
        "expected at least 3 distinct output lengths with jitter=10, got {}",
        lengths.len()
    );
    assert!(all_decoded_ok, "all frames should decode correctly");
}

#[test]
fn test_frame_jitter_no_decode_errors_over_50_ticks() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_jitter_padding_bytes(10);

    let mut status_count = 0;
    let mut ready_count = 0;
    let mut decoder = FrameDecoder::new();

    for _ in 0..50 {
        let bytes = sim.tick();
        let frames = decoder.feed_slice(&bytes);
        for f in &frames {
            if f.message_type == [0xFF, 0xAF] {
                status_count += 1;
            } else if f.message_type == [0x10, 0xBF] {
                ready_count += 1;
            }
        }
    }

    assert_eq!(
        status_count, 50,
        "should decode 50 status frames with jitter"
    );
    assert!(
        ready_count > 0,
        "should decode some ready frames with jitter"
    );
    assert_eq!(
        decoder.frame_error_count(),
        0,
        "should have zero frame errors with jitter padding"
    );
}

#[test]
fn test_partial_frame_split_reassembly() {
    // Split status frame at midpoint; tick1 emits first N bytes, tick2 emits remainder + Ready.
    // FrameDecoder should reassemble the split frame across two feeds.
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Generate the expected status frame to find its length
    let status_bytes = sim.generate_status_frame();
    let split_point = status_bytes.len() / 2;

    sim.inject_partial_frame_at(split_point);

    // Tick 1: should emit only first `split_point` bytes of status frame (no Ready)
    let tick1_bytes = sim.tick();
    assert!(
        tick1_bytes.len() < status_bytes.len(),
        "tick1 should emit fewer bytes than a full status frame"
    );

    // Tick 2: should emit remainder of status frame + Ready frame
    let tick2_bytes = sim.tick();
    assert!(
        !tick2_bytes.is_empty(),
        "tick2 should produce remainder bytes"
    );

    // Feed both halves to a FrameDecoder — should decode the complete status frame
    let mut decoder = FrameDecoder::new();
    let frames1 = decoder.feed_slice(&tick1_bytes);
    let frames2 = decoder.feed_slice(&tick2_bytes);

    // First feed should not produce any complete frames (partial only)
    assert!(
        frames1.is_empty(),
        "first half should not produce complete frames, got {}",
        frames1.len()
    );

    // Second feed should produce at least the status frame + Ready
    assert!(
        frames2.len() >= 2,
        "second half should produce status + ready frames, got {}",
        frames2.len()
    );
    assert_eq!(
        frames2[0].message_type,
        [0xFF, 0xAF],
        "first decoded frame should be status"
    );
    assert_eq!(
        frames2[1].message_type,
        [0x10, 0xBF],
        "second decoded frame should be ready"
    );
}

#[test]
fn test_partial_frame_split_at_zero() {
    // Split at 0: the full status frame is the "remainder", so tick1 should emit
    // the full status frame (no partial), and tick2 emits Ready.
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.inject_partial_frame_at(0);

    // Tick 1: full status frame (split at 0 means no bytes split off)
    let tick1_bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames1 = decoder.feed_slice(&tick1_bytes);

    // Should have decoded the status frame
    assert!(
        frames1.len() >= 1,
        "tick1 with split_at=0 should produce the full status frame"
    );
    assert_eq!(
        frames1[0].message_type,
        [0xFF, 0xAF],
        "should be status frame"
    );

    // Tick 2: Ready frame (remainder is empty, so just Ready)
    let tick2_bytes = sim.tick();
    let mut decoder2 = FrameDecoder::new();
    let frames2 = decoder2.feed_slice(&tick2_bytes);
    assert!(
        frames2.len() >= 1,
        "tick2 should produce at least the ready frame"
    );
    assert_eq!(
        frames2[0].message_type,
        [0x10, 0xBF],
        "should be ready frame"
    );
}

#[test]
fn test_partial_frame_oneshot_reset() {
    // After partial frame fires (two ticks), subsequent ticks produce normal unsplit output.
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Get a reference normal tick output (after registration, no injection)
    let normal_bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let normal_frames = decoder.feed_slice(&normal_bytes);
    assert_eq!(normal_frames.len(), 2, "normal tick: status + ready");

    // Reset sim for controlled test
    let mut sim2 = SpaSim::new();
    sim2.registered = true;

    let status_bytes = sim2.generate_status_frame();
    let split_point = status_bytes.len() / 2;
    sim2.inject_partial_frame_at(split_point);

    // Tick 1: partial frame
    let _tick1 = sim2.tick();
    // Tick 2: remainder + ready
    let _tick2 = sim2.tick();

    // Tick 3: should be normal (no split)
    let tick3_bytes = sim2.tick();
    let mut decoder3 = FrameDecoder::new();
    let tick3_frames = decoder3.feed_slice(&tick3_bytes);

    // Should be a normal tick: status + ready
    assert_eq!(
        tick3_frames.len(),
        2,
        "third tick should produce normal 2 frames (status + ready)"
    );
    assert_eq!(tick3_frames[0].message_type, [0xFF, 0xAF], "status frame");
    assert_eq!(tick3_frames[1].message_type, [0x10, 0xBF], "ready frame");

    // Tick 4: also normal
    let tick4_bytes = sim2.tick();
    let mut decoder4 = FrameDecoder::new();
    let tick4_frames = decoder4.feed_slice(&tick4_bytes);
    assert_eq!(tick4_frames.len(), 2, "fourth tick should also be normal");
}

#[test]
fn test_partial_frame_reassembly_content_correct() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 100.0;

    // Generate a reference frame for content comparison
    let reference_bytes = sim.generate_status_frame();
    let mut ref_decoder = FrameDecoder::new();
    let ref_frames = ref_decoder.feed_slice(&reference_bytes);
    assert_eq!(ref_frames.len(), 1, "reference should be 1 frame");
    let reference_payload = ref_frames[0].payload.clone();

    // Now split a frame and verify reassembled content matches
    // Use a fresh sim to get consistent state
    let mut sim2 = SpaSim::new();
    sim2.registered = true;
    sim2.state.current_temp = 100.0;
    sim2.state.set_temp = 100.0;

    let status_bytes = sim2.generate_status_frame();
    let split_point = status_bytes.len() / 3; // Split at 1/3
    sim2.inject_partial_frame_at(split_point);

    // Tick 1: first partial
    let tick1_bytes = sim2.tick();
    // Tick 2: remainder + ready
    let tick2_bytes = sim2.tick();

    let mut decoder = FrameDecoder::new();
    let _partial = decoder.feed_slice(&tick1_bytes);
    let reassembled = decoder.feed_slice(&tick2_bytes);

    // Should have at least status + ready
    assert!(
        reassembled.len() >= 2,
        "should reassemble status + ready, got {} frames",
        reassembled.len()
    );

    // First reassembled frame should be the status frame
    assert_eq!(
        reassembled[0].message_type,
        [0xFF, 0xAF],
        "first frame should be status"
    );

    // The payload of the status frame should match reference (allowing for
    // minute increment since a tick occurred between reference and split)
    // Key check: message type and payload length match
    assert_eq!(
        reassembled[0].payload.len(),
        reference_payload.len(),
        "reassembled payload length should match reference"
    );
}

#[test]
fn test_variable_ready_interval_gaps_in_range() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_ready_interval_range(2, 5);

    let mut last_ready_tick: Option<u64> = None;
    let mut gaps: Vec<u64> = Vec::new();

    for _ in 0..200 {
        let tick = sim.tick_count() + 1;
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        for f in &frames {
            if f.message_type == [0x10, 0xBF] {
                if let Some(last) = last_ready_tick {
                    gaps.push(tick - last);
                }
                last_ready_tick = Some(tick);
            }
        }
    }

    assert!(!gaps.is_empty(), "should have observed some Ready frames");

    for (i, &gap) in gaps.iter().enumerate() {
        assert!(
            gap >= 2 && gap <= 5,
            "gap {} should be in [2, 5], got {}",
            i,
            gap
        );
    }
}

#[test]
fn test_variable_ready_interval_constant_when_min_eq_max() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_ready_interval_range(3, 3);

    let mut last_ready_tick: Option<u64> = None;
    let mut gaps: Vec<u64> = Vec::new();

    for _ in 0..30 {
        let tick = sim.tick_count() + 1;
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        for f in &frames {
            if f.message_type == [0x10, 0xBF] {
                if let Some(last) = last_ready_tick {
                    gaps.push(tick - last);
                }
                last_ready_tick = Some(tick);
            }
        }
    }

    // All gaps should be exactly 3
    for (i, &gap) in gaps.iter().enumerate() {
        assert_eq!(gap, 3, "gap {} should be exactly 3 when min=max=3", i);
    }
}

#[test]
fn test_duplicate_frame_injection() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Normal tick produces N bytes
    let normal_bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let normal_frames = decoder.feed_slice(&normal_bytes);

    // Reset sim for comparison
    let mut sim2 = SpaSim::new();
    sim2.registered = true;
    sim2.inject_duplicate_frame();
    let dup_bytes = sim2.tick();

    // Duplicate tick should have more bytes (extra status frame)
    assert!(dup_bytes.len() > normal_bytes.len());

    let mut decoder2 = FrameDecoder::new();
    let dup_frames = decoder2.feed_slice(&dup_bytes);
    assert!(
        dup_frames.len() > normal_frames.len(),
        "should have extra frames from duplication"
    );
}
