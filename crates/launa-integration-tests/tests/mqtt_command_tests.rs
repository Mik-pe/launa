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
fn test_invalid_toggle_payload() {
    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/test_spa/command",
        "launa/test_spa/command/pump1",
        b"on",
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

    let (mt, payload) = cmd.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);
    assert_eq!(sim.state.pumps[0], PumpState::Low);

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

    let (mt, payload) = cmd.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);
    assert_eq!(sim.state.set_temp, 102.0);

    let status_encoded = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_encoded);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, 102.0);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_command_round_trip_pump_toggle() {
    let mut sim = SpaSim::new();
    assert_eq!(sim.state.pumps[0], PumpState::Off);

    let cmd = launa_mqtt::command_parser::parse_command_ok(
        "launa/spa/command",
        "launa/spa/command/pump1",
        b"true",
    )
    .expect("should parse");

    let (mt, payload) = cmd.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);

    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "pump1 should be on after toggle"
    );

    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        let json_str = launa_mqtt::state::status_to_json(&s, None, None);
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

    let (mt, payload) = cmd.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);
    assert_eq!(sim.state.set_temp, 100.0);
}
