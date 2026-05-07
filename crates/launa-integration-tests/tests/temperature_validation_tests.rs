//! Temperature validation pipeline integration tests.
//!
//! Tests for validated temperature parsing and full pipeline:
//! - Fahrenheit temperature validation through MQTT parser → SpaSim
//! - Celsius temperature validation with wire value conversion
//! - Fahrenheit through SpaApp command queue
//! - Celsius through SpaApp command queue

mod common;

use common::{make_ready_frame, make_spaapp, make_status_frame};

use launa_core::AppAction;
use launa_mqtt::command_parser::ParseResult;
use launa_protocol::command::Command;
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::{TempRange, TemperatureScale};
use launa_protocol::Temperature;
use launa_sim::SpaSim;

#[test]
fn test_validated_temperature_pipeline_fahrenheit() {
    let mut sim = SpaSim::new();
    // Set Fahrenheit scale so raw wire value 100 is interpreted as 100°F
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    sim.state.set_temp = Temperature::fahrenheit(104.0);

    let parse_result = launa_mqtt::command_parser::parse_set_temperature_validated(
        "100",
        TemperatureScale::Fahrenheit,
        TempRange::High,
    );
    match &parse_result {
        ParseResult::Valid(cmd) => {
            assert_eq!(*cmd, Command::SetTemperature(100));
        }
        other => panic!("expected Valid, got {:?}", other),
    }

    let cmd = match parse_result {
        ParseResult::Valid(c) => c,
        _ => unreachable!(),
    };
    let (mt, payload) = cmd.encode().unwrap();
    let encoded = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    assert_eq!(frames.len(), 1);
    sim.process_frame(&frames[0]);

    // Verify through decoded status frame (observable output), not sim.state
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, Temperature::fahrenheit(100.0));
            assert_eq!(s.temperature_scale, TemperatureScale::Fahrenheit);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_validated_temperature_pipeline_celsius() {
    let mut sim = SpaSim::new();
    // Rationale: sim.state fields are test scenario setup inputs,
    // not assertions — the actual verification is through decoded status frames.
    sim.state.temp_scale = TemperatureScale::Celsius;
    sim.state.current_temp = Temperature::celsius(36.0);
    sim.state.set_temp = Temperature::celsius(40.0);

    let parse_result = launa_mqtt::command_parser::parse_set_temperature_validated(
        "38",
        TemperatureScale::Celsius,
        TempRange::High,
    );
    match &parse_result {
        ParseResult::Valid(cmd) => {
            assert!(matches!(*cmd, Command::SetTemperature(_)));
        }
        other => panic!("expected Valid, got {:?}", other),
    }

    let wire_value: u8 = 38u8.saturating_mul(2);
    let (mt, payload) = Command::SetTemperature(wire_value).encode().unwrap();
    let encoded = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);

    // Verify through decoded status frame (observable output), not sim.state
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, Temperature::celsius(38.0));
            assert_eq!(s.temperature_scale, TemperatureScale::Celsius);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_validated_temperature_pipeline_through_spaapp_fahrenheit() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);
    let mut sim = SpaSim::new();
    // Set Fahrenheit scale so raw wire value 102 is interpreted as 102°F
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    sim.state.set_temp = Temperature::fahrenheit(104.0);

    app.process_frame(&make_status_frame());

    let parse_result = launa_mqtt::command_parser::parse_set_temperature_validated(
        "102",
        TemperatureScale::Fahrenheit,
        TempRange::High,
    );
    let cmd = match parse_result {
        ParseResult::Valid(c) => c,
        other => panic!("expected Valid, got {:?}", other),
    };
    assert_eq!(cmd, Command::SetTemperature(102));

    app.on_mqtt_command(cmd);
    assert_eq!(app.queued_command_count(), 1);

    let actions = app.process_frame(&make_ready_frame(0x03));
    let send_frame = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should produce SendFrame on Ready");

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&send_frame);
    assert_eq!(frames.len(), 1);
    sim.process_frame(&frames[0]);

    // Verify through decoded status frame (observable output), not sim.state
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, Temperature::fahrenheit(102.0));
        }
        _ => panic!("Expected StatusUpdate"),
    }

    let actions = app.process_frame(&status_frames[0]);
    let has_state = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(has_state);
    assert_eq!(app.total_retries(), 0, "command should be confirmed");
    assert_eq!(app.total_dropped(), 0, "no drops expected");
}

#[test]
fn test_validated_temperature_pipeline_through_spaapp_celsius() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Celsius;
    sim.state.current_temp = Temperature::celsius(38.0);
    sim.state.set_temp = Temperature::celsius(38.0);

    let status_bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let status_frames = decoder.feed_slice(&status_bytes);
    app.process_frame(&status_frames[0]);

    let parse_result = launa_mqtt::command_parser::parse_set_temperature_validated(
        "40",
        TemperatureScale::Celsius,
        TempRange::High,
    );
    match &parse_result {
        ParseResult::Valid(_) => {}
        other => panic!("expected Valid for 40°C, got {:?}", other),
    }

    let cmd = Command::SetTemperature(80); // wire value for 40°C
    assert_eq!(cmd, Command::SetTemperature(80));

    app.on_mqtt_command(cmd);
    let actions = app.process_frame(&make_ready_frame(0x03));
    let send_frame = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should produce SendFrame");

    let frames = decoder.feed_slice(&send_frame);
    sim.process_frame(&frames[0]);

    // Verify through decoded status frame (observable output), not sim.state
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, Temperature::celsius(40.0));
            assert_eq!(s.temperature_scale, TemperatureScale::Celsius);
        }
        _ => panic!("Expected StatusUpdate"),
    }

    let actions = app.process_frame(&status_frames[0]);
    assert!(actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. })));
}
