//! Protocol round-trip integration tests.
//!
//! Tests that exercise the full encode → decode → dispatch pipeline for each
//! protocol message type: status, config, information, fault log, filter cycles.
//! Also covers edge cases like unknown message types, short payloads, and
//! temperature boundary values.

use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::config::PumpConfig;
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::fault::FaultCode;
use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
use launa_protocol::information::{HeaterType, HeaterVoltage};
use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale};
use launa_sim::SpaSim;

#[test]
fn test_status_frame_round_trip() {
    let mut sim = SpaSim::new();
    let encoded = sim.generate_status_frame();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    assert_eq!(frames.len(), 1);

    let frame = &frames[0];
    assert_eq!(frame.message_type, [0xFF, 0xAF]);

    let msg = dispatch_frame(frame);
    match msg {
        IncomingMessage::StatusUpdate(status) => {
            assert_eq!(status.current_temp, Some(100.0));
            assert_eq!(status.set_temp, 104.0);
            assert_eq!(status.hour, 14);
            assert_eq!(status.minute, 30);
            assert_eq!(status.heating_mode, HeatingMode::Ready);
            assert_eq!(status.temperature_scale, TemperatureScale::Fahrenheit);
            assert!(status.is_heating);
            assert_eq!(status.temp_range, TempRange::High);
        }
        _ => panic!("Expected StatusUpdate, got {:?}", msg),
    }
}

#[test]
fn test_config_request_response_round_trip() {
    let mut sim = SpaSim::new();

    let (mt, payload) = Command::ConfigurationRequest.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    assert_eq!(frames.len(), 1);
    let request_frame = &frames[0];

    let response = sim
        .process_frame(request_frame)
        .expect("should return config response");
    let response_frames = decoder.feed_slice(&response);
    assert_eq!(response_frames.len(), 1);

    let msg = dispatch_frame(&response_frames[0]);
    match msg {
        IncomingMessage::ControlConfiguration(config) => {
            assert_eq!(config.pump_configs[0], PumpConfig::TwoSpeed);
            assert_eq!(config.pump_configs[1], PumpConfig::TwoSpeed);
            assert!(config.circ_pump);
            assert!(config.blower);
            assert!(config.lights[0]);
        }
        _ => panic!("Expected ControlConfiguration, got {:?}", msg),
    }
}

#[test]
fn test_information_request_response_round_trip() {
    let mut sim = SpaSim::new();

    let (mt, payload) = Command::InformationRequest.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let request_frame = &frames[0];

    let response = sim
        .process_frame(request_frame)
        .expect("should return info response");
    let response_frames = decoder.feed_slice(&response);
    let msg = dispatch_frame(&response_frames[0]);
    match msg {
        IncomingMessage::InformationResponse(info) => {
            assert_eq!(info.system_model, "BFBP20");
            assert_eq!(info.config_signature, "3D12382E");
            assert_eq!(info.heater_voltage, HeaterVoltage::V240);
            assert_eq!(info.heater_type, HeaterType::Standard);
        }
        _ => panic!("Expected InformationResponse, got {:?}", msg),
    }
}

#[test]
fn test_fault_log_round_trip() {
    let mut sim = SpaSim::new();

    let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let request_frame = &frames[0];

    let response = sim
        .process_frame(request_frame)
        .expect("should return fault response");
    let response_frames = decoder.feed_slice(&response);
    let msg = dispatch_frame(&response_frames[0]);
    match msg {
        IncomingMessage::FaultLogResponse(entry) => {
            assert_eq!(entry.fault_count, 3);
            assert_eq!(entry.message_code, FaultCode::HeaterDry);
        }
        _ => panic!("Expected FaultLogResponse, got {:?}", msg),
    }
}

#[test]
fn test_filter_cycles_round_trip() {
    let mut sim = SpaSim::new();

    let (mt, payload) = Command::FilterCyclesRequest.encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let request_frame = &frames[0];

    let response = sim
        .process_frame(request_frame)
        .expect("should return filter response");
    let response_frames = decoder.feed_slice(&response);
    let msg = dispatch_frame(&response_frames[0]);
    match msg {
        IncomingMessage::FilterCyclesResponse(fc) => {
            assert_eq!(fc.filter1.start_hour, 8);
            assert_eq!(fc.filter1.duration_hours, 4);
            assert_eq!(fc.filter2.start_hour, 16);
            assert!(fc.filter2.enabled);
        }
        _ => panic!("Expected FilterCyclesResponse, got {:?}", msg),
    }
}

#[test]
fn test_toggle_pump1_command() {
    let mut sim = SpaSim::new();
    let mut decoder = FrameDecoder::new();

    // Verify initial state through decoded status frame
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.pumps[0], PumpState::Off);
    } else {
        panic!("Expected StatusUpdate");
    }

    let (mt, payload) = Command::ToggleItem(ToggleItem::Pump1).encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let frames = decoder.feed_slice(&encoded);
    let frame = &frames[0];

    sim.process_frame(frame);
    // Verify through decoded status: Off → Low
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.pumps[0], PumpState::Low);
    } else {
        panic!("Expected StatusUpdate");
    }

    sim.process_frame(frame);
    // Verify through decoded status: Low → High
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.pumps[0], PumpState::High);
    } else {
        panic!("Expected StatusUpdate");
    }

    sim.process_frame(frame);
    // Verify through decoded status: High → Off
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.pumps[0], PumpState::Off);
    } else {
        panic!("Expected StatusUpdate");
    }
}

#[test]
fn test_toggle_light_command() {
    let mut sim = SpaSim::new();
    let mut decoder = FrameDecoder::new();

    // Verify initial state through decoded status frame
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert!(!s.lights[0]);
    } else {
        panic!("Expected StatusUpdate");
    }

    let (mt, payload) = Command::ToggleItem(ToggleItem::Light1).encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let frames = decoder.feed_slice(&encoded);
    let frame = &frames[0];

    sim.process_frame(frame);
    // Verify through decoded status: light on
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert!(s.lights[0]);
    } else {
        panic!("Expected StatusUpdate");
    }

    sim.process_frame(frame);
    // Verify through decoded status: light off
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert!(!s.lights[0]);
    } else {
        panic!("Expected StatusUpdate");
    }
}

#[test]
fn test_set_temperature_command() {
    let mut sim = SpaSim::new();
    let mut decoder = FrameDecoder::new();

    // Verify initial state through decoded status frame
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.set_temp, 104.0);
    } else {
        panic!("Expected StatusUpdate");
    }

    let (mt, payload) = Command::SetTemperature(100).encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let frames = decoder.feed_slice(&encoded);
    let frame = &frames[0];

    sim.process_frame(frame);

    // Verify through decoded status frame
    let status_encoded = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_encoded);
    let msg = dispatch_frame(&status_frames[0]);
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, 100.0);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_corrupted_frame_bad_crc() {
    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0u8; 24],
    };
    let mut encoded = frame.encode().unwrap();

    let crc_idx = encoded.len() - 2;
    encoded[crc_idx] ^= 0xFF;

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    assert_eq!(
        frames.len(),
        0,
        "Corrupted CRC should not yield a valid frame"
    );
}

#[test]
fn test_truncated_frame() {
    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0u8; 24],
    };
    let encoded = frame.encode().unwrap();

    let truncated = &encoded[..encoded.len() - 5];

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(truncated);
    assert_eq!(frames.len(), 0, "Truncated frame should not decode");
}

#[test]
fn test_frame_wrong_markers() {
    let data: &[u8] = &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05];

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(data);
    assert_eq!(frames.len(), 0, "Bytes without markers should not decode");
}

#[test]
fn test_unknown_message_type() {
    let frame = Frame {
        message_type: [0xAB, 0xCD],
        payload: vec![0x01, 0x02, 0x03],
    };
    let msg = dispatch_frame(&frame);
    match msg {
        IncomingMessage::Unknown {
            message_type,
            payload,
        } => {
            assert_eq!(message_type, [0xAB, 0xCD]);
            assert_eq!(payload, vec![0x01, 0x02, 0x03]);
        }
        _ => panic!("Expected Unknown"),
    }
}

#[test]
fn test_unknown_0abf_subtype() {
    let frame = Frame {
        message_type: [0x0A, 0xBF],
        payload: vec![0xFF],
    };
    let msg = dispatch_frame(&frame);
    assert!(matches!(msg, IncomingMessage::Unknown { .. }));
}

#[test]
fn test_status_unknown_temp() {
    let mut sim = SpaSim::new();
    // Rationale: sim.state.current_temp is test input to configure unknown temp (255).
    // Verification is through the decoded status frame's current_temp being None.
    sim.state.current_temp = 255.0;

    let encoded = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let msg = dispatch_frame(&frames[0]);

    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.current_temp, None);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_status_max_temp() {
    let mut sim = SpaSim::new();
    // Rationale: sim.state fields are test inputs for boundary values.
    // Verification is through the decoded status frame.
    sim.state.current_temp = 254.0;
    sim.state.set_temp = 254.0;

    let encoded = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let msg = dispatch_frame(&frames[0]);

    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.current_temp, Some(254.0));
            assert_eq!(s.set_temp, 254.0);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_status_min_temp() {
    let mut sim = SpaSim::new();
    // Rationale: sim.state fields are test inputs for boundary values.
    // Verification is through the decoded status frame.
    sim.state.current_temp = 1.0;
    sim.state.set_temp = 1.0;

    let encoded = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let msg = dispatch_frame(&frames[0]);

    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.current_temp, Some(1.0));
            assert_eq!(s.set_temp, 1.0);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_celsius_status_values() {
    let mut sim = SpaSim::new();
    // Rationale: sim.state fields are test inputs for Celsius configuration.
    // Verification is through the decoded status frame.
    sim.state.temp_scale = TemperatureScale::Celsius;
    sim.state.current_temp = 38.0;
    sim.state.set_temp = 40.0;

    let encoded = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let msg = dispatch_frame(&frames[0]);

    match msg {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.current_temp, Some(38.0));
            assert_eq!(s.set_temp, 40.0);
            assert_eq!(s.temperature_scale, TemperatureScale::Celsius);
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_empty_frame_payload() {
    let frame = Frame {
        message_type: [0x0A, 0xBF],
        payload: vec![],
    };
    let msg = dispatch_frame(&frame);
    assert!(matches!(msg, IncomingMessage::Unknown { .. }));
}

#[test]
fn test_status_payload_too_short() {
    let frame = Frame {
        message_type: [0xFF, 0xAF],
        payload: vec![0u8; 10],
    };
    let msg = dispatch_frame(&frame);
    assert!(matches!(msg, IncomingMessage::Unknown { .. }));
}

#[test]
fn test_config_payload_too_short() {
    let frame = Frame {
        message_type: [0x0A, 0xBF],
        payload: vec![0x94, 0x01, 0x02],
    };
    let msg = dispatch_frame(&frame);
    assert!(matches!(msg, IncomingMessage::Unknown { .. }));
}

#[test]
fn test_toggle_all_items() {
    let mut sim = SpaSim::new();
    let mut decoder = FrameDecoder::new();

    let toggles = [ToggleItem::Pump1, ToggleItem::Pump2, ToggleItem::Pump3];

    for item in &toggles {
        let (mt, payload) = Command::ToggleItem(*item).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let frames = decoder.feed_slice(&encoded);
        sim.process_frame(&frames[0]);
    }

    // Verify pump states through decoded status frame
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.pumps[0], PumpState::Low);
        assert_eq!(s.pumps[1], PumpState::Low);
        assert_eq!(s.pumps[2], PumpState::Low);
    } else {
        panic!("Expected StatusUpdate");
    }

    let (mt, payload) = Command::ToggleItem(ToggleItem::Blower).encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);
    // Verify blower through decoded status
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert!(s.blower);
    } else {
        panic!("Expected StatusUpdate");
    }

    let (mt, payload) = Command::ToggleItem(ToggleItem::HeatingMode).encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);
    // Verify heating mode through decoded status
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.heating_mode, HeatingMode::Rest);
    } else {
        panic!("Expected StatusUpdate");
    }

    let (mt, payload) = Command::ToggleItem(ToggleItem::TemperatureRange).encode();
    let encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let frames = decoder.feed_slice(&encoded);
    sim.process_frame(&frames[0]);
    // Verify temp range through decoded status
    let status_bytes = sim.generate_status_frame();
    let status_frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&status_frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.temp_range, TempRange::Low);
    } else {
        panic!("Expected StatusUpdate");
    }
}
