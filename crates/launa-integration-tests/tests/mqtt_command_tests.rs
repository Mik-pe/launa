//! MQTT command parsing integration tests.
//!
//! Tests for MQTT command parsing edge cases:
//! - Valid command parsing (pump2, light1)
//! - Invalid payloads (non-boolean toggle, non-numeric temperature)
//! - Wrong base topic and unknown subtopics
//! - Full MQTT command → frame → SpaSim pipeline
//! - MQTT set temperature pipeline

use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::{FrameDecoder, FrameEncoder};
use launa_protocol::status::PumpState;
use launa_protocol::Temperature;
use launa_sim::SpaSim;

#[test]
fn test_mqtt_command_parse_pump2() {
    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/spa/cmd",
        "launa/spa/cmd/pump2",
        b"true",
    );
    assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Pump2)));
}

#[test]
fn test_mqtt_command_parse_light1() {
    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/spa/cmd",
        "launa/spa/cmd/light1",
        b"false",
    );
    assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Light1)));
}

#[test]
fn test_mqtt_command_wrong_base() {
    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/spa_a/cmd",
        "launa/spa_b/cmd/pump1",
        b"true",
    );
    assert_eq!(cmd, None);
}

#[test]
fn test_mqtt_command_unknown_subtopic() {
    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/spa/cmd",
        "launa/spa/cmd/nonexistent",
        b"true",
    );
    assert_eq!(cmd, None);
}

#[test]
fn test_empty_toggle_payload() {
    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/test_spa/command",
        "launa/test_spa/command/pump1",
        b"",
    );
    assert_eq!(cmd, None);
}

#[test]
fn test_invalid_temperature_payload() {
    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/test_spa/command",
        "launa/test_spa/command/set_temperature",
        b"abc",
    );
    assert_eq!(cmd, None);
}

#[test]
fn test_mqtt_command_to_frame_to_simulator() {
    let mut sim = SpaSim::new();

    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/test_spa/command",
        "launa/test_spa/command/pump1",
        b"true",
    )
    .expect("should parse command");
    assert_eq!(cmd, Command::ToggleItem(ToggleItem::Pump1));

    let (mt, payload) = cmd.encode().unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);

    // Verify through decoded status frame (observable output), not sim.state
    let status_encoded = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_encoded);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.pumps[0], PumpState::Low);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_mqtt_set_temperature_pipeline() {
    let mut sim = SpaSim::new();

    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/test_spa/command",
        "launa/test_spa/command/set_temperature",
        b"102",
    )
    .expect("should parse command");
    assert_eq!(cmd, Command::SetTemperature(102));

    let (mt, payload) = cmd.encode().unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);

    // Verify through decoded status frame (observable output), not sim.state
    let status_encoded = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_encoded);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, Temperature::fahrenheit(102.0));
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_command_round_trip_pump_toggle() {
    let mut sim = SpaSim::new();

    // Verify initial state through decoded status frame
    let status_bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.pumps[0], PumpState::Off, "pump1 should start Off");
    } else {
        panic!("Expected StatusUpdate");
    }

    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/spa/command",
        "launa/spa/command/pump1",
        b"true",
    )
    .expect("should parse");

    let (mt, payload) = cmd.encode().unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);

    // Verify pump state through decoded status frame and MQTT JSON
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(
            s.pumps[0],
            PumpState::Low,
            "pump1 should be on after toggle"
        );
        let json_str = launa_mqtt::state::status_to_json(&s, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["pump1_on"], true);
    } else {
        panic!("Expected StatusUpdate");
    }
}

#[test]
fn test_command_round_trip_set_temperature() {
    let mut sim = SpaSim::new();

    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/spa/command",
        "launa/spa/command/set_temperature",
        b"100",
    )
    .expect("should parse");
    assert_eq!(cmd, Command::SetTemperature(100));

    let (mt, payload) = cmd.encode().unwrap();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);

    // Verify through decoded status frame (observable output), not sim.state
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, Temperature::fahrenheit(100.0));
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

// --- Empty MQTT command payload edge case tests (VAL-INTG-001) ---

/// Empty payload on a pump toggle topic should be rejected by the command parser.
/// The command parser should return None for empty bytes — no Command produced.
#[test]
fn test_empty_payload_pump1_command_rejected() {
    let result = launa_mqtt::command_parser::parse_command_ok(
        "launa/DEVICE/command",
        "launa/DEVICE/command/pump1",
        b"",
    );
    assert_eq!(
        result, None,
        "empty payload on pump1 should produce no command"
    );
}

/// Empty payload on set_temperature should be rejected — no panic, no Command.
#[test]
fn test_empty_payload_set_temperature_rejected() {
    let result = launa_mqtt::command_parser::parse_command_ok(
        "launa/DEVICE/command",
        "launa/DEVICE/command/set_temperature",
        b"",
    );
    assert_eq!(
        result, None,
        "empty payload on set_temperature should produce no command"
    );
}

/// Empty payload on hold_mode toggle should be rejected — no panic, no Command.
#[test]
fn test_empty_payload_hold_mode_rejected() {
    let result = launa_mqtt::command_parser::parse_command_ok(
        "launa/DEVICE/command",
        "launa/DEVICE/command/hold_mode",
        b"",
    );
    assert_eq!(
        result, None,
        "empty payload on hold_mode should produce no command"
    );
}

/// Verify that the parse_command (detailed) variant returns InvalidPayload for empty bytes,
/// not UnknownSubtopic — the topic is valid but the payload is not.
#[test]
fn test_empty_payload_returns_invalid_payload_not_unknown() {
    use launa_mqtt::command_parser::parse_command;
    use launa_mqtt::command_parser::ParseResult;

    let result = parse_command("launa/DEVICE/command", "launa/DEVICE/command/pump1", b"");
    assert!(
        matches!(result, ParseResult::InvalidPayload(_)),
        "empty payload should be InvalidPayload, got {:?}",
        result
    );

    let result = parse_command(
        "launa/DEVICE/command",
        "launa/DEVICE/command/set_temperature",
        b"",
    );
    assert!(
        matches!(result, ParseResult::InvalidPayload(_)),
        "empty set_temperature payload should be InvalidPayload, got {:?}",
        result
    );

    let result = parse_command(
        "launa/DEVICE/command",
        "launa/DEVICE/command/hold_mode",
        b"",
    );
    assert!(
        matches!(result, ParseResult::InvalidPayload(_)),
        "empty hold_mode payload should be InvalidPayload, got {:?}",
        result
    );
}

/// Integration-level test: since parse_command_ok returns None for empty payloads,
/// no command is ever queued into SpaApp, so no SendFrame action for a pump toggle
/// can be emitted. This verifies the full pipeline from empty MQTT payload → no
/// pump-related SendFrame side effect.
#[test]
fn test_empty_mqtt_command_no_send_frame_via_harness() {
    use launa_integration_tests::harness::TestHarness;

    let mut harness = TestHarness::new();
    harness.complete_registration(50);

    let empty_cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/test_spa/command",
        "launa/test_spa/command/pump1",
        b"",
    );
    assert!(
        empty_cmd.is_none(),
        "empty payload should produce no command"
    );

    // No command queued — verify tick produces no pump1 toggle SendFrame
    let actions = harness.tick_spa_with_outgoing();
    assert!(
        !TestHarness::has_toggle_for(&actions, ToggleItem::Pump1),
        "no pump1 toggle SendFrame should be emitted when empty payload produced no command"
    );
}
