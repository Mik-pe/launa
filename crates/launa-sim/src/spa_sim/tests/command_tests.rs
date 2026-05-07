use super::*;
use launa_protocol::frame::FrameEncoder;
use launa_protocol::status::{PumpState, TemperatureScale};
use launa_protocol::Temperature;

#[test]
fn test_process_toggle_via_bytes() {
    let mut sim = SpaSim::new();

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    sim.process_incoming_bytes(&encoded);
    assert_eq!(sim.state.pumps[0], PumpState::Low);
}

#[test]
fn test_set_temp_decoded_from_wire() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Fahrenheit;

    // Simulate a SetTemperature(100) command coming in
    let (mt, payload) = launa_protocol::command::Command::SetTemperature(100)
        .encode()
        .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    assert_eq!(sim.state.set_temp, Temperature::fahrenheit(100.0));
}

#[test]
fn test_set_temp_decoded_celsius() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Celsius;

    // SetTemperature sends raw value 80 (= 40°C on wire)
    let (mt, payload) = launa_protocol::command::Command::SetTemperature(80)
        .encode()
        .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    assert_eq!(sim.state.set_temp, Temperature::celsius(40.0));
}

#[test]
fn test_command_success_rate_ignores_toggle() {
    let mut sim = SpaSim::new();
    sim.set_command_success_rate(0.0); // Never accept commands

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    // Pump should still be Off (command was ignored)
    assert_eq!(sim.state.pumps[0], PumpState::Off);
}

#[test]
fn test_command_success_rate_accepts_toggle() {
    let mut sim = SpaSim::new();
    sim.set_command_success_rate(1.0); // Always accept commands

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    // Pump should be Low (command was accepted)
    assert_eq!(sim.state.pumps[0], PumpState::Low);
}

#[test]
fn test_command_success_rate_ignores_set_temp() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    sim.set_command_success_rate(0.0); // Never accept commands

    let (mt, payload) = launa_protocol::command::Command::SetTemperature(90)
        .encode()
        .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    // Set temp should remain at default (40.0°C)
    assert_eq!(sim.state.set_temp, Temperature::celsius(40.0));
}

#[test]
fn test_command_success_rate_accepts_set_temp() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    sim.set_command_success_rate(1.0); // Always accept commands

    let (mt, payload) = launa_protocol::command::Command::SetTemperature(100)
        .encode()
        .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    assert_eq!(sim.state.set_temp, Temperature::fahrenheit(100.0));
}

#[test]
fn test_command_latency_default_immediate() {
    let mut sim = SpaSim::new();
    // Default command_latency_ticks=0: commands applied immediately

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    sim.process_incoming_bytes(&encoded);
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "default latency=0 should apply toggle immediately"
    );
}

#[test]
fn test_command_latency_deferred() {
    let mut sim = SpaSim::new();
    sim.set_command_latency_ticks(3);

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    // Send command
    sim.process_incoming_bytes(&encoded);

    // State should NOT change immediately
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Off,
        "state should not change immediately with latency=3"
    );

    // Tick 1: still not applied
    sim.tick();
    assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 1: still pending");

    // Tick 2: still not applied
    sim.tick();
    assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 2: still pending");

    // Tick 3: should be applied now
    sim.tick();
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "tick 3: deferred command should be applied"
    );
}

#[test]
fn test_command_latency_multiple_commands_order() {
    let mut sim = SpaSim::new();
    sim.set_command_latency_ticks(2);

    // Send two toggle commands for different pumps
    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded1 = FrameEncoder::encode(mt, &payload).unwrap();

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump2)
            .encode()
            .unwrap();
    let encoded2 = FrameEncoder::encode(mt, &payload).unwrap();

    sim.process_incoming_bytes(&encoded1);
    sim.process_incoming_bytes(&encoded2);

    // Neither applied yet
    assert_eq!(sim.state.pumps[0], PumpState::Off);
    assert_eq!(sim.state.pumps[1], PumpState::Off);

    // Tick 1: pending
    sim.tick();
    assert_eq!(sim.state.pumps[0], PumpState::Off);
    assert_eq!(sim.state.pumps[1], PumpState::Off);

    // Tick 2: both should be applied
    sim.tick();
    assert_eq!(sim.state.pumps[0], PumpState::Low, "pump1 toggle applied");
    assert_eq!(sim.state.pumps[1], PumpState::Low, "pump2 toggle applied");
}

#[test]
fn test_command_latency_defers_set_temperature() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    sim.set_command_latency_ticks(4);

    // Send SetTemperature(96)
    let (mt, payload) = launa_protocol::command::Command::SetTemperature(96)
        .encode()
        .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    // Ticks 1-3: set_temp should remain at default (40.0°C)
    for i in 1..=3 {
        sim.tick();
        assert_eq!(
            sim.state.set_temp,
            Temperature::celsius(40.0),
            "tick {}: set_temp should not change yet",
            i
        );
    }

    // Tick 4: set_temp should change to 96.0
    sim.tick();
    assert_eq!(
        sim.state.set_temp,
        Temperature::fahrenheit(96.0),
        "tick 4: deferred set_temp should be applied"
    );
}

#[test]
fn test_command_latency_set_temp_and_toggle_order() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    sim.set_command_latency_ticks(2);

    // Send set_temp and toggle in sequence
    let (mt, payload) = launa_protocol::command::Command::SetTemperature(96)
        .encode()
        .unwrap();
    let encoded1 = FrameEncoder::encode(mt, &payload).unwrap();

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded2 = FrameEncoder::encode(mt, &payload).unwrap();

    sim.process_incoming_bytes(&encoded1);
    sim.process_incoming_bytes(&encoded2);

    // Tick 1: pending
    sim.tick();
    assert_eq!(
        sim.state.set_temp,
        Temperature::celsius(40.0),
        "set_temp unchanged tick 1"
    );
    assert_eq!(sim.state.pumps[0], PumpState::Off, "pump unchanged tick 1");

    // Tick 2: both should apply
    sim.tick();
    assert_eq!(
        sim.state.set_temp,
        Temperature::fahrenheit(96.0),
        "set_temp applied tick 2"
    );
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "pump toggle applied tick 2"
    );
}

#[test]
fn test_command_success_rate_50_percent() {
    let mut sim = SpaSim::new();
    sim.set_command_success_rate(0.5);

    let total_commands = 100;
    let mut accepted = 0;

    for _ in 0..total_commands {
        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode()
        .unwrap();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();

        let before = sim.state.pumps[0];
        sim.process_incoming_bytes(&encoded);
        let after = sim.state.pumps[0];

        // If the pump state changed, the command was accepted
        if before != after {
            accepted += 1;
        }

        // Reset pump state for next iteration to avoid saturation
        sim.state.pumps[0] = PumpState::Off;
    }

    let acceptance_rate = accepted as f32 / total_commands as f32;
    assert!(
        acceptance_rate >= 0.35 && acceptance_rate <= 0.65,
        "with rate=0.5, expected ~50% acceptance (35-65%), got {:.0}% ({}/{})",
        acceptance_rate * 100.0,
        accepted,
        total_commands
    );
}

#[test]
fn test_command_success_rate_boundaries() {
    // Rate 0.0: no commands accepted
    let mut sim = SpaSim::new();
    sim.set_command_success_rate(0.0);
    for _ in 0..20 {
        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode()
        .unwrap();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);
    }
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Off,
        "rate 0.0 should reject all commands"
    );

    // Rate 1.0: all commands accepted
    let mut sim = SpaSim::new();
    sim.set_command_success_rate(1.0);
    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "rate 1.0 should accept all commands"
    );
}

// ── Registration validation tests ──────────────────────────────────

#[test]
fn test_require_registration_rejects_toggle_when_unregistered() {
    let mut sim = SpaSim::new();
    sim.set_require_registration(true);

    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    assert_eq!(
        sim.state.pumps[0],
        PumpState::Off,
        "toggle should be rejected"
    );
    assert_eq!(sim.rejected_unregistered_frames(), 1);
}

#[test]
fn test_require_registration_rejects_set_temp_when_unregistered() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    sim.set_require_registration(true);

    let (mt, payload) = launa_protocol::command::Command::SetTemperature(100)
        .encode()
        .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    assert_eq!(
        sim.state.set_temp,
        Temperature::celsius(40.0),
        "set_temp should be rejected"
    );
    assert_eq!(sim.rejected_unregistered_frames(), 1);
}

#[test]
fn test_require_registration_rejects_config_request_when_unregistered() {
    let mut sim = SpaSim::new();
    sim.set_require_registration(true);

    let (mt, payload) = launa_protocol::command::Command::ConfigurationRequest
        .encode()
        .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let responses = sim.process_incoming_bytes(&encoded);

    assert!(responses.is_empty(), "config response should be suppressed");
    assert_eq!(sim.rejected_unregistered_frames(), 1);
}

#[test]
fn test_require_registration_accepts_toggle_after_registration() {
    let mut sim = SpaSim::new();
    sim.set_require_registration(true);

    // Register via new-client flow: FE BF 01 → FE BF 02 <id> <hash> → <id> BF 03
    let reg_response = FrameEncoder::encode([0xFE, 0xBF], &[0x00]).unwrap();
    sim.process_incoming_bytes(&reg_response); // NewClientQuery — sim sends it, but we feed it back
                                               // Sim doesn't register from its own query. Feed a NewClientResponse instead:
    let new_client_resp = FrameEncoder::encode([0xFE, 0xBF], &[0x01, 0x02, 0xAB, 0xCD]).unwrap();
    let responses = sim.process_incoming_bytes(&new_client_resp);
    // Sim sends ClientIdAssignment — extract and feed the ack
    let mut decoder = launa_protocol::frame::FrameDecoder::new();
    let frames = decoder.feed_slice(&responses);
    let mut assigned_id = 0u8;
    for frame in &frames {
        if frame.message_type == [0xFE, 0xBF]
            && !frame.payload.is_empty()
            && frame.payload[0] == 0x02
        {
            assigned_id = frame.payload[1];
            let ack = FrameEncoder::encode([assigned_id, 0xBF], &[0x03]).unwrap();
            sim.process_incoming_bytes(&ack);
        }
    }
    assert!(sim.is_registered());

    // Now toggle should work
    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "toggle should work after registration"
    );
    assert_eq!(sim.rejected_unregistered_frames(), 0);
}

#[test]
fn test_require_registration_allows_registration_frames() {
    let mut sim = SpaSim::new();
    sim.set_require_registration(true);

    // FE BF 01 (NewClientResponse) should always be accepted
    let new_client_resp = FrameEncoder::encode([0xFE, 0xBF], &[0x01, 0x02, 0xAB, 0xCD]).unwrap();
    let responses = sim.process_incoming_bytes(&new_client_resp);
    assert!(
        !responses.is_empty(),
        "registration response should produce output"
    );
    assert_eq!(sim.rejected_unregistered_frames(), 0);

    // <ID> BF 03 (ClientIdAck) should also be accepted
    let ack = FrameEncoder::encode([0x02, 0xBF], &[0x03]).unwrap();
    sim.process_incoming_bytes(&ack);
    assert!(sim.is_registered());
    assert_eq!(sim.rejected_unregistered_frames(), 0);
}

#[test]
fn test_require_registration_counts_multiple_rejections() {
    let mut sim = SpaSim::new();
    sim.set_require_registration(true);

    // Send 3 toggles while unregistered
    for _ in 0..3 {
        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode()
        .unwrap();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);
    }

    assert_eq!(sim.rejected_unregistered_frames(), 3);
    assert_eq!(sim.state.pumps[0], PumpState::Off);
}

#[test]
fn test_require_registration_default_off() {
    let mut sim = SpaSim::new();

    // Without enabling require_registration, commands should work
    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode()
            .unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "default should allow commands"
    );
    assert_eq!(sim.rejected_unregistered_frames(), 0);
}
