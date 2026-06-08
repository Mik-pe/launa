use super::*;
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::PumpState;
use launa_protocol::Temperature;

#[test]
fn test_bus_silence_produces_no_output() {
    let mut sim = SpaSim::new();
    sim.simulate_bus_silence(3);

    let bytes1 = sim.tick();
    assert!(bytes1.is_empty(), "silenced tick should produce no bytes");

    let bytes2 = sim.tick();
    assert!(bytes2.is_empty());

    let bytes3 = sim.tick();
    assert!(bytes3.is_empty());

    // Silence over, normal output resumes
    let bytes4 = sim.tick();
    assert!(!bytes4.is_empty(), "should resume after silence");
}

#[test]
fn test_corrupt_frame_injection() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Get a normal frame for comparison
    let normal = sim.generate_status_frame();

    // Inject corruption
    sim.inject_corrupt_frame();
    let corrupt = sim.generate_status_frame();

    // Frames should differ
    assert_ne!(normal, corrupt, "corrupt frame should differ from normal");

    // Corrupt frame should still be parseable as bytes (just has bad CRC)
    assert!(!corrupt.is_empty());

    // Verify that the corrupt frame actually triggers a CRC error in the decoder
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&corrupt);
    // The corrupt byte should cause a CRC mismatch → no valid frames decoded
    assert!(
        frames.is_empty() || decoder.frame_error_count() > 0,
        "corrupt frame should cause frame error (frames={}, errors={})",
        frames.len(),
        decoder.frame_error_count()
    );
}

#[test]
fn test_jitter_and_latency_together() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_jitter_padding_bytes(5);
    sim.set_command_latency_ticks(2);

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    // Jitter should work (variable output), latency should defer command
    let mut distinct_lengths = std::collections::BTreeSet::new();
    for _ in 0..20 {
        let bytes = sim.tick();
        distinct_lengths.insert(bytes.len());
    }

    assert!(
        distinct_lengths.len() >= 2,
        "jitter should produce at least 2 distinct lengths, got {}",
        distinct_lengths.len()
    );

    // Command should be applied after 2 ticks
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "deferred command applied after latency ticks"
    );
}

#[test]
fn test_all_three_features_together() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_jitter_padding_bytes(3);
    sim.set_command_latency_ticks(1);
    sim.set_ready_interval_range(1, 3);

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    // Tick through several cycles
    let mut status_count = 0;
    let mut ready_count = 0;
    for _ in 0..20 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        for f in &frames {
            if f.message_type == [0xFF, 0xAF] {
                status_count += 1;
            }
            if f.message_type == [0x10, 0xBF] {
                ready_count += 1;
            }
        }
    }

    // Status should appear every tick
    assert_eq!(status_count, 20, "status every tick");

    // Ready should appear at interval (1,3), so fewer than 20
    assert!(
        (7..=20).contains(&ready_count),
        "ready at randomized interval, got {}",
        ready_count
    );

    // Command should be applied
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "deferred command applied"
    );
}

#[test]
fn test_combined_degraded_bus_500_ticks() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = Temperature::fahrenheit(95.0);
    sim.state.set_temp = Temperature::fahrenheit(104.0);
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Enable ALL degradation features
    sim.set_jitter_padding_bytes(5);
    sim.set_command_latency_ticks(2);
    sim.set_command_success_rate(0.7);
    sim.set_ready_interval_range(2, 4);
    sim.set_physics_overshoot(Temperature::fahrenheit(1.5));
    sim.set_physics_noise_amplitude(1.0);
    sim.set_physics_unknown_temp_ticks(5); // First 5 ticks unknown temp

    let mut decoder = FrameDecoder::new();
    let mut status_count = 0;
    let mut frame_errors = 0;
    let mut panic_detected = false;

    for _tick_num in 1..=500 {
        let bytes = sim.tick();
        if bytes.is_empty() {
            continue; // bus silence
        }

        let frames = decoder.feed_slice(&bytes);

        for f in &frames {
            if f.message_type == [0xFF, 0xAF] {
                status_count += 1;
                let msg = launa_protocol::dispatcher::dispatch_frame(f);
                match msg {
                    launa_protocol::dispatcher::IncomingMessage::StatusUpdate(_) => {}
                    launa_protocol::dispatcher::IncomingMessage::Unknown { .. } => {
                        // This should not happen — means protocol desync
                        panic_detected = true;
                    }
                    _ => {} // other messages are fine
                }
            }
        }

        frame_errors += decoder.frame_error_count() as usize;
    }

    assert!(
        !panic_detected,
        "protocol desync detected during 500 tick degraded bus test"
    );
    assert_eq!(
        frame_errors, 0,
        "should have zero frame errors during degraded bus test"
    );
    assert!(
        status_count >= 400,
        "should have decoded most status frames, got {}",
        status_count
    );
}

#[test]
fn test_combined_degraded_bus_commands_eventually_deliver() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.pumps[0] = PumpState::Off;
    sim.set_command_latency_ticks(2);
    sim.set_command_success_rate(0.7);
    sim.set_jitter_padding_bytes(3);
    sim.set_ready_interval_range(1, 2);

    // Send toggle pump1 command
    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();

    // Try sending the command multiple times
    let mut pump_changed = false;
    for _ in 0..50 {
        sim.process_incoming_bytes(&encoded);
        // Tick through latency
        for _ in 0..3 {
            sim.tick();
        }
        if sim.state.pumps[0] != PumpState::Off {
            pump_changed = true;
            break;
        }
    }

    // With 70% success rate and 50 attempts, should eventually succeed
    assert!(
        pump_changed,
        "command should eventually be accepted with rate=0.7"
    );
}
