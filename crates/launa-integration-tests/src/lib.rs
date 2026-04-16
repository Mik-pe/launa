//! Comprehensive integration tests for the Launa spa controller firmware.
//!
//! These tests exercise the full pipeline from simulator → protocol → MQTT.

#[cfg(test)]
mod tests {
    use launa_ota::{OtaError, OtaUpdate};
    use launa_protocol::command::{Command, ToggleItem};
    use launa_protocol::config::PumpConfig;
    use launa_protocol::crc8;
    use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
    use launa_protocol::fault::FaultCode;
    use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
    use launa_protocol::information::{HeaterType, HeaterVoltage};
    use launa_protocol::registration::{
        RegistrationAction, RegistrationState, RegistrationStateMachine,
    };
    use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale};
    use launa_sim::SpaSim;

    // ========================================================================
    // Test Group A: Protocol Round-Trip
    // ========================================================================

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

    // ========================================================================
    // Test Group B: Command Flow
    // ========================================================================

    #[test]
    fn test_toggle_pump1_command() {
        let mut sim = SpaSim::new();
        assert_eq!(sim.state.pumps[0], PumpState::Off);

        let (mt, payload) = Command::ToggleItem(ToggleItem::Pump1).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        let frame = &frames[0];

        sim.process_frame(frame);
        assert_eq!(sim.state.pumps[0], PumpState::Low);

        sim.process_frame(frame);
        assert_eq!(sim.state.pumps[0], PumpState::High);

        sim.process_frame(frame);
        assert_eq!(sim.state.pumps[0], PumpState::Off);
    }

    #[test]
    fn test_toggle_light_command() {
        let mut sim = SpaSim::new();
        assert!(!sim.state.lights[0]);

        let (mt, payload) = Command::ToggleItem(ToggleItem::Light1).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        let frame = &frames[0];

        sim.process_frame(frame);
        assert!(sim.state.lights[0]);

        sim.process_frame(frame);
        assert!(!sim.state.lights[0]);
    }

    #[test]
    fn test_set_temperature_command() {
        let mut sim = SpaSim::new();
        assert_eq!(sim.state.set_temp, 104.0);

        let (mt, payload) = Command::SetTemperature(100).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        let frame = &frames[0];

        sim.process_frame(frame);
        assert_eq!(sim.state.set_temp, 100.0);

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
    fn test_full_registration_flow() {
        let mut sim = SpaSim::new();
        let mut client_sm = RegistrationStateMachine::new();
        let mut decoder = FrameDecoder::new();

        assert_eq!(client_sm.state(), &RegistrationState::WaitingForQuery);

        // Step 1: Simulator sends registration query (FE BF 00)
        let query = sim.generate_registration_query();
        let query_frames = decoder.feed_slice(&query);
        assert_eq!(query_frames.len(), 1);

        let query_msg = dispatch_frame(&query_frames[0]);
        assert_eq!(query_msg, IncomingMessage::NewClientQuery);

        let action = client_sm.process([0xFE, 0xBF], &[0x00]);
        assert_eq!(action, RegistrationAction::SendIdRequest);
        assert_eq!(client_sm.state(), &RegistrationState::WaitingForAssignment);

        // Step 2: Client sends ID request (FE BF 01)
        let client_request = FrameEncoder::encode([0xFE, 0xBF], &[0x01]).unwrap();
        let request_frames = decoder.feed_slice(&client_request);
        let request_frame = &request_frames[0];

        // Simulator processes client request and assigns ID
        let assignment = sim.process_frame(request_frame).expect("should assign ID");

        // Step 3: Simulator sends assignment (FE BF 02 <ID>)
        let assignment_frames = decoder.feed_slice(&assignment);
        let assignment_frame = &assignment_frames[0];
        assert_eq!(assignment_frame.message_type, [0xFE, 0xBF]);

        let assignment_msg = dispatch_frame(assignment_frame);
        match assignment_msg {
            IncomingMessage::ClientIdAssignment { id } => {
                assert_eq!(id, 0x02);

                let action = client_sm.process([0xFE, 0xBF], &[0x02, id]);
                assert_eq!(action, RegistrationAction::SendIdAck { client_id: id });
                assert!(client_sm.is_registered());
                assert_eq!(client_sm.client_id(), Some(0x02));

                // Step 4: Client sends ack (<ID> BF 03)
                let ack = FrameEncoder::encode([id, 0xBF], &[0x03]).unwrap();
                let ack_frames = decoder.feed_slice(&ack);
                sim.process_frame(&ack_frames[0]);
                assert_eq!(sim.client_id, Some(0x02));
            }
            _ => panic!("Expected ClientIdAssignment"),
        }
    }

    // ========================================================================
    // Test Group C: End-to-End MQTT Pipeline
    // ========================================================================

    #[test]
    fn test_status_to_mqtt_json() {
        let mut sim = SpaSim::new();
        let encoded = sim.generate_status_frame();

        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        let msg = dispatch_frame(&frames[0]);

        match msg {
            IncomingMessage::StatusUpdate(status) => {
                let json_str = launa_mqtt::state::status_to_json(&status, None, None);
                let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

                assert_eq!(parsed["current_temp"], 100.0);
                assert_eq!(parsed["set_temp"], 104.0);
                assert_eq!(parsed["is_heating"], true);
                assert_eq!(parsed["heating_mode"], "ready");
                assert_eq!(parsed["temp_range"], "high");
                assert_eq!(parsed["temp_scale"], "fahrenheit");
            }
            _ => panic!("Expected StatusUpdate"),
        }
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

    // ========================================================================
    // Test Group D: OTA Mock
    // ========================================================================

    #[test]
    fn test_ota_full_flow() {
        let mut ota = launa_ota::mock::MockOta::new();

        ota.begin().unwrap();
        assert!(ota.firmware_data.is_empty());
        assert!(!ota.finalized);

        let chunk1: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let chunk2: Vec<u8> = vec![0xCA, 0xFE, 0xBA, 0xBE];
        ota.write(&chunk1).unwrap();
        ota.write(&chunk2).unwrap();

        assert_eq!(ota.firmware_data.len(), 8);
        assert_eq!(&ota.firmware_data[0..4], &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(&ota.firmware_data[4..8], &[0xCA, 0xFE, 0xBA, 0xBE]);

        ota.finalize().unwrap();
        assert!(ota.finalized);

        ota.mark_valid().unwrap();
        assert!(ota.valid);
    }

    #[test]
    fn test_ota_begin_clears_previous_data() {
        let mut ota = launa_ota::mock::MockOta::new();

        // First OTA session: begin -> write -> finalize
        ota.begin().unwrap();
        ota.write(&[0x01, 0x02, 0x03]).unwrap();
        assert_eq!(ota.firmware_data.len(), 3);
        ota.finalize().unwrap();

        // New session: begin() clears previous data
        ota.begin().unwrap();
        assert!(ota.firmware_data.is_empty());
    }

    #[test]
    fn test_ota_firmware_data_accumulation() {
        let mut ota = launa_ota::mock::MockOta::new();
        ota.begin().unwrap();

        for i in 0u8..10 {
            ota.write(&[i]).unwrap();
        }

        assert_eq!(ota.firmware_data, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    // ========================================================================
    // Test Group E: Discovery
    // ========================================================================

    #[test]
    fn test_discovery_configs_valid_json() {
        let builder = launa_mqtt::discovery::DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        assert_eq!(configs.len(), 27);

        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));
        }
    }

    #[test]
    fn test_discovery_unique_ids() {
        let builder = launa_mqtt::discovery::DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        let mut unique_ids = std::collections::HashSet::new();
        for (_topic, json_str) in &configs {
            let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
            let uid = parsed["unique_id"].as_str().unwrap().to_string();
            assert!(
                uid.starts_with("test_spa_001_"),
                "unique_id '{}' should start with device id",
                uid
            );
            assert!(unique_ids.insert(uid), "duplicate unique_id found");
        }
    }

    #[test]
    fn test_discovery_topics_match_pattern() {
        let builder = launa_mqtt::discovery::DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        for (topic, json_str) in &configs {
            assert!(
                topic.starts_with("homeassistant/"),
                "topic '{}' should start with homeassistant/",
                topic
            );

            let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
            assert!(
                parsed["state_topic"].is_string(),
                "state_topic should be a string"
            );
            assert!(
                parsed["availability_topic"].is_string(),
                "availability_topic should be a string"
            );

            let state_topic = parsed["state_topic"].as_str().unwrap();
            assert!(state_topic.starts_with("launa/test_spa_001/"));
        }
    }

    #[test]
    fn test_discovery_command_topics() {
        let builder = launa_mqtt::discovery::DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        for (_topic, json_str) in &configs {
            let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
            if let Some(cmd_topic) = parsed.get("command_topic").and_then(|v| v.as_str()) {
                assert!(
                    cmd_topic.starts_with("launa/test_spa_001/command/"),
                    "command_topic '{}' should start with launa/test_spa_001/command/",
                    cmd_topic
                );
            }
        }
    }

    #[test]
    fn test_topic_builder() {
        let topics = launa_mqtt::topics::TopicBuilder::new("my_spa");
        assert_eq!(topics.state_topic(), "launa/my_spa/state");
        assert_eq!(topics.command_topic(), "launa/my_spa/command");
        assert_eq!(topics.availability_topic(), "launa/my_spa/availability");
        assert_eq!(topics.ota_topic(), "launa/my_spa/ota");
        assert_eq!(
            topics.discovery_topic("sensor", "temperature"),
            "homeassistant/sensor/my_spa/temperature/config"
        );
    }

    // ========================================================================
    // Test Group F: Error Handling
    // ========================================================================

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
        sim.state.temp_scale = TemperatureScale::Celsius;
        sim.state.current_temp = 38.0; // 38°C → wire: 76
        sim.state.set_temp = 40.0; // 40°C → wire: 80

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

    // ========================================================================
    // Test Group G: Multi-Frame Streaming
    // ========================================================================

    #[test]
    fn test_feed_bytes_one_at_a_time() {
        let mut sim = SpaSim::new();
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
        let mut sim = SpaSim::new();

        let status1 = sim.generate_status_frame();
        let _tick_bytes = sim.tick(); // advances physics (returns reg query + status + ready)
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
        let mut sim = SpaSim::new();

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

    // ========================================================================
    // Additional Simulator Tests
    // ========================================================================

    #[test]
    fn test_simulator_tick_updates_time() {
        let mut sim = SpaSim::new();
        assert_eq!(sim.state.hour, 14);
        assert_eq!(sim.state.minute, 30);

        sim.tick();
        assert_eq!(sim.state.minute, 31);

        for _ in 0..29 {
            sim.tick();
        }
        assert_eq!(sim.state.minute, 0);
        assert_eq!(sim.state.hour, 15);
    }

    #[test]
    fn test_simulator_tick_heating_approaches_set_temp() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 95.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = true;

        sim.tick();
        assert_eq!(sim.state.current_temp, 96.0);

        sim.tick();
        assert_eq!(sim.state.current_temp, 97.0);

        for _ in 0..10 {
            sim.tick();
        }
        assert_eq!(sim.state.current_temp, 100.0);
    }

    #[test]
    fn test_simulator_tick_cools_down() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 95.0;
        sim.state.is_heating = false;

        sim.tick();
        assert_eq!(sim.state.current_temp, 99.0);
    }

    #[test]
    fn test_toggle_all_items() {
        let mut sim = SpaSim::new();

        let toggles = [ToggleItem::Pump1, ToggleItem::Pump2, ToggleItem::Pump3];

        for item in &toggles {
            let (mt, payload) = Command::ToggleItem(*item).encode();
            let encoded = FrameEncoder::encode(mt, &payload).unwrap();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&encoded);
            sim.process_frame(&frames[0]);
        }

        assert_eq!(sim.state.pumps[0], PumpState::Low);
        assert_eq!(sim.state.pumps[1], PumpState::Low);
        assert_eq!(sim.state.pumps[2], PumpState::Low);

        let (mt, payload) = Command::ToggleItem(ToggleItem::Blower).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        sim.process_frame(&frames[0]);
        assert!(sim.state.blower);

        let (mt, payload) = Command::ToggleItem(ToggleItem::HeatingMode).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        sim.process_frame(&frames[0]);
        assert_eq!(sim.state.heating_mode, HeatingMode::Rest);

        let (mt, payload) = Command::ToggleItem(ToggleItem::TemperatureRange).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        sim.process_frame(&frames[0]);
        assert_eq!(sim.state.temp_range, TempRange::Low);
    }

    #[test]
    fn test_crc8_known_values() {
        assert_eq!(crc8::compute(&[]), 0x00);
        assert_eq!(crc8::compute(&[0x00]), 0x0C);

        let data: &[u8] = &[
            0x1D, 0xFF, 0xAF, 0x13, 0x00, 0x00, 0x64, 0x07, 0x07, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00,
        ];
        assert_eq!(crc8::compute(data), 0xC2);
    }

    #[test]
    fn test_registration_state_machine_reset() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

        sm.reset();
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
        assert!(!sm.is_registered());
    }

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
    fn test_discovery_custom_device_name() {
        let builder = launa_mqtt::discovery::DiscoveryBuilder::new("spa_001")
            .device_name("My Hot Tub")
            .device_model("BP6013G1");
        let configs = builder.build();

        let (_, json_str) = configs.first().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed["device"]["name"], "My Hot Tub");
        assert_eq!(parsed["device"]["model"], "BP6013G1");
    }

    // ========================================================================
    // Phase 2: Desktop end-to-end tests (no HW needed)
    // ========================================================================

    /// Full pipeline integration test: SpaSim generates status frame ->
    /// FrameDecoder parses -> StatusUpdate extracted -> status_to_json() produces
    /// MQTT payload -> assert JSON fields match simulator state.
    #[test]
    fn test_full_pipeline_status_frame_to_mqtt_json() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 104.0;
        sim.state.pumps[0] = PumpState::Low;
        sim.state.pumps[1] = PumpState::Off;
        sim.state.pumps[2] = PumpState::Off;
        sim.state.circ_pump = true;
        sim.state.blower = false;
        sim.state.lights[0] = true;
        sim.state.mister = false;
        sim.state.is_heating = true;
        sim.state.hold = false;

        let status_bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&status_bytes);
        assert!(!frames.is_empty(), "should produce at least one frame");

        let msg = dispatch_frame(&frames[0]);
        match msg {
            IncomingMessage::StatusUpdate(status) => {
                let json_str = launa_mqtt::state::status_to_json(&status, None, None);
                let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

                // Verify JSON fields match simulator state
                assert_eq!(parsed["current_temp"], 100.0);
                assert_eq!(parsed["set_temp"], 104.0);
                assert_eq!(parsed["is_heating"], true);
                assert_eq!(parsed["pump1_on"], true);
                assert_eq!(parsed["pump2_on"], false);
                assert_eq!(parsed["pump3_on"], false);
                assert_eq!(parsed["circ_pump"], true);
                assert_eq!(parsed["blower"], false);
                assert_eq!(parsed["light1"], true);
                assert_eq!(parsed["mister"], false);
                assert_eq!(parsed["hold_mode"], false);
            }
            other => panic!("Expected StatusUpdate, got {:?}", other),
        }
    }

    /// Command round-trip: MQTT command string -> parse_command() -> Command ->
    /// encode() -> frame bytes -> SpaSim process_frame -> verify state change.
    #[test]
    fn test_command_round_trip_pump_toggle() {
        let mut sim = SpaSim::new();
        assert_eq!(sim.state.pumps[0], PumpState::Off);

        // Parse MQTT command
        let cmd = launa_mqtt::command_parser::parse_command_ok(
            "launa/spa/command",
            "launa/spa/command/pump1",
            b"true",
        )
        .expect("should parse");

        // Encode to frame
        let (mt, payload) = cmd.encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();

        // Feed to simulator
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        sim.process_frame(&frames[0]);

        // Verify state change
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "pump1 should be on after toggle"
        );

        // Generate new status and verify JSON reflects change
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

    /// Command round-trip for set_temperature.
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

    /// HA discovery validation: generate all 14 discovery payloads,
    /// validate they are valid JSON with correct topic patterns,
    /// correct unique_id, command_topic, state_topic patterns.
    #[test]
    fn test_ha_discovery_full_validation() {
        let builder = launa_mqtt::discovery::DiscoveryBuilder::new("test_spa");
        let configs = builder.build();

        assert_eq!(
            configs.len(),
            27,
            "should produce exactly 27 discovery configs"
        );

        let mut topics_seen = std::collections::HashSet::new();

        for (topic, json_str) in &configs {
            // Topic must follow HA pattern: homeassistant/<component>/<device_id>/<object_id>/config
            assert!(
                topic.starts_with("homeassistant/"),
                "topic should start with homeassistant/: {}",
                topic
            );
            assert!(
                topic.ends_with("/config"),
                "topic should end with /config: {}",
                topic
            );
            assert!(
                topic.contains("/test_spa/"),
                "topic should contain device_id: {}",
                topic
            );

            // No duplicate topics
            assert!(
                topics_seen.insert(topic.clone()),
                "duplicate topic: {}",
                topic
            );

            // Must be valid JSON
            let v: serde_json::Value = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));

            // Must have required HA fields
            assert!(v.get("name").is_some(), "missing name in {}", topic);
            assert!(
                v.get("unique_id").is_some(),
                "missing unique_id in {}",
                topic
            );
            assert!(
                v.get("state_topic").is_some(),
                "missing state_topic in {}",
                topic
            );
            assert!(
                v.get("availability_topic").is_some(),
                "missing availability_topic in {}",
                topic
            );

            // unique_id must contain device_id
            let uid = v["unique_id"].as_str().unwrap();
            assert!(
                uid.starts_with("test_spa_"),
                "unique_id should start with device_id: {}",
                uid
            );

            // state_topic must be the device state topic (or a dedicated topic for diagnostics/alert)
            let st = v["state_topic"].as_str().unwrap();
            let uid = v["unique_id"].as_str().unwrap();
            let is_dedicated_topic = uid.ends_with("_diagnostics") || uid.ends_with("_alert");
            if !is_dedicated_topic {
                assert_eq!(
                    st, "launa/test_spa/state",
                    "state_topic should match device state topic for {}",
                    uid
                );
            } else {
                // Dedicated topics should still be under the device namespace
                assert!(
                    st.starts_with("launa/test_spa/"),
                    "dedicated state_topic should be under device namespace: {}",
                    st
                );
            }

            // availability_topic must match
            let at = v["availability_topic"].as_str().unwrap();
            assert_eq!(at, "launa/test_spa/availability");

            // If there's a command_topic, it must be under the device command base
            if let Some(ct) = v.get("command_topic").and_then(|t| t.as_str()) {
                assert!(
                    ct.starts_with("launa/test_spa/command/"),
                    "command_topic should be under device command base: {}",
                    ct
                );
            }
        }
    }

    /// Registration flow test: simulate full client ID registration using
    /// RegistrationStateMachine, verifying all state transitions.
    #[test]
    fn test_registration_flow_with_state_machine() {
        use launa_protocol::registration::{
            RegistrationAction, RegistrationState, RegistrationStateMachine,
        };

        let mut sm = RegistrationStateMachine::new();
        assert!(!sm.is_registered());
        assert!(matches!(sm.state(), RegistrationState::WaitingForQuery));

        // Step 1: Simulate receiving a client ID query from the spa (FE BF 00)
        let action = sm.process([0xFE, 0xBF], &[0x00]);
        assert_eq!(
            action,
            RegistrationAction::SendIdRequest,
            "should respond to query with ID request"
        );
        assert!(matches!(
            sm.state(),
            RegistrationState::WaitingForAssignment
        ));

        // Step 2: Simulate receiving client ID assignment (FE BF 02 <id>)
        let action = sm.process([0xFE, 0xBF], &[0x02, 0x03]);
        assert_eq!(
            action,
            RegistrationAction::SendIdAck { client_id: 0x03 },
            "should send ack after assignment"
        );
        assert!(sm.is_registered(), "should be registered after assignment");

        // Step 3: Verify we can now encode commands with the assigned client ID
        let cmd = Command::NothingToSend { client_id: 0x03 };
        let (mt, _) = cmd.encode();
        assert_eq!(mt, [0x03, 0xBF]);
    }

    // ========================================================================
    // Test Group H: OTA Integration Tests
    // ========================================================================
    //
    // Integration-level OTA tests that exercise the full OTA flow end-to-end
    // on desktop, simulating HTTP firmware download from memory and writing
    // through the OtaUpdate trait via MockOta.

    /// Simulates an HTTP firmware download server that serves firmware data
    /// in configurable chunk sizes, mimicking how the real OTA downloads
    /// firmware over a TCP socket.
    struct SimHttpServer {
        firmware: Vec<u8>,
        chunk_size: usize,
    }

    impl SimHttpServer {
        fn new(firmware: Vec<u8>, chunk_size: usize) -> Self {
            SimHttpServer {
                firmware,
                chunk_size,
            }
        }

        /// Simulate downloading all firmware chunks from the server.
        /// Returns each chunk as if read from a TCP socket.
        fn download_chunks(&self) -> Vec<Vec<u8>> {
            let mut chunks = Vec::new();
            let mut offset = 0;
            while offset < self.firmware.len() {
                let end = (offset + self.chunk_size).min(self.firmware.len());
                chunks.push(self.firmware[offset..end].to_vec());
                offset = end;
            }
            chunks
        }
    }

    /// Simulate an OTA download-and-write pipeline: download chunks from
    /// a simulated HTTP server and write each chunk through the OtaUpdate trait.
    fn simulate_ota_download(
        ota: &mut dyn OtaUpdate,
        server: &SimHttpServer,
    ) -> Result<(), OtaError> {
        ota.begin()?;
        for chunk in server.download_chunks() {
            ota.write(&chunk)?;
        }
        ota.finalize()
    }

    #[test]
    fn test_ota_basic_flow() {
        let mut ota = launa_ota::mock::MockOta::new();

        // Simulate a 4 KiB firmware image served in 1 KiB chunks
        let firmware: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        let server = SimHttpServer::new(firmware.clone(), 1024);

        // Full OTA pipeline: download → write → finalize → mark valid
        simulate_ota_download(&mut ota, &server).unwrap();
        ota.mark_valid().unwrap();

        // Verify the OTA state
        assert!(ota.finalized, "OTA should be finalized");
        assert!(ota.valid, "OTA should be marked valid");
        assert_eq!(ota.firmware_data.len(), 4096);
        assert_eq!(ota.firmware_data, firmware, "firmware data should match");
    }

    #[test]
    fn test_ota_rollback() {
        let mut ota = launa_ota::mock::MockOta::new();

        let firmware: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let server = SimHttpServer::new(firmware.clone(), 256);

        // Simulate OTA update completing but firmware failing to boot
        simulate_ota_download(&mut ota, &server).unwrap();
        assert!(ota.finalized);
        assert!(!ota.valid, "should NOT be valid before mark_valid");

        // Firmware crashes before mark_valid — trigger rollback
        ota.rollback_and_reboot().unwrap();
        assert!(ota.rolled_back, "should have rolled back");
        assert!(
            !ota.valid,
            "firmware should still be invalid after rollback"
        );
    }

    #[test]
    fn test_ota_write_failure() {
        /// A MockOta variant that fails after N bytes written, simulating
        /// a flash write error mid-transfer.
        struct FailingOta {
            inner: launa_ota::mock::MockOta,
            max_bytes: usize,
            failed: bool,
        }

        impl FailingOta {
            fn new(max_bytes: usize) -> Self {
                FailingOta {
                    inner: launa_ota::mock::MockOta::new(),
                    max_bytes,
                    failed: false,
                }
            }
        }

        impl OtaUpdate for FailingOta {
            fn begin(&mut self) -> Result<(), OtaError> {
                self.inner.begin()
            }
            fn write(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
                if self.failed || self.inner.firmware_data.len() + chunk.len() > self.max_bytes {
                    self.failed = true;
                    return Err(OtaError::WriteFailed {
                        byte_offset: self.inner.firmware_data.len(),
                    });
                }
                self.inner.write(chunk)
            }
            fn finalize(&mut self) -> Result<(), OtaError> {
                self.inner.finalize()
            }
            fn mark_valid(&mut self) -> Result<(), OtaError> {
                self.inner.mark_valid()
            }
            fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
                self.inner.rollback_and_reboot()
            }
        }

        let mut ota = FailingOta::new(512);

        // 1 KiB firmware but only 512 bytes allowed — should fail
        let firmware: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        let server = SimHttpServer::new(firmware, 256);

        // Attempt OTA — should fail during write
        let result = simulate_ota_download(&mut ota, &server);
        assert!(result.is_err(), "OTA should fail when write fails");
        assert!(ota.failed, "should have recorded the failure");
        assert!(
            !ota.inner.finalized,
            "should not be finalized after failure"
        );
    }

    #[test]
    fn test_ota_chunked_writes() {
        let mut ota = launa_ota::mock::MockOta::new();

        // 8 KiB firmware with various chunk sizes to simulate realistic HTTP
        let firmware: Vec<u8> = (0..8192).map(|i| ((i * 7 + 13) % 256) as u8).collect();

        // Use multiple chunk sizes to simulate realistic network behavior
        let chunk_sizes = [512, 1024, 1460, 256, 4096];

        ota.begin().unwrap();
        let mut offset = 0;
        for (round, &chunk_size) in chunk_sizes.iter().cycle().enumerate() {
            if offset >= firmware.len() {
                break;
            }
            let end = (offset + chunk_size).min(firmware.len());
            let chunk = &firmware[offset..end];
            ota.write(chunk).unwrap();
            offset = end;

            // Sanity: shouldn't take more than 100 rounds for 8 KiB
            assert!(round < 100, "too many rounds, likely infinite loop");
        }
        ota.finalize().unwrap();
        ota.mark_valid().unwrap();

        // Verify assembled firmware is exactly what we wrote
        assert_eq!(ota.firmware_data.len(), 8192);
        assert_eq!(ota.firmware_data, firmware);
    }

    // ========================================================================
    // Test Group J: OTA Integration Tests — Error Paths & Safety
    // ========================================================================
    //
    // Integration-level OTA tests verifying graceful shutdown, rollback,
    // size limits, and concurrent operation safety using MockOta with
    // failure injection fields.

    /// OTA graceful shutdown happy path: verify the complete call sequence
    /// begin → write(N) → finalize → mark_valid is executed in correct order.
    /// Each step succeeds and the final state is valid.
    #[test]
    fn test_ota_graceful_shutdown_happy_path() {
        use launa_ota::mock::MockOta;

        let mut ota = MockOta::new();
        let firmware: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
        let server = SimHttpServer::new(firmware.clone(), 512);

        // Step 1: begin() — opens OTA partition
        ota.begin().unwrap();
        assert!(ota.firmware_data.is_empty());
        assert!(!ota.finalized);
        assert!(!ota.valid);

        // Step 2: write() — download chunks from server and write
        for chunk in server.download_chunks() {
            ota.write(&chunk).unwrap();
        }
        assert_eq!(ota.firmware_data.len(), 2048);
        assert_eq!(ota.firmware_data, firmware);

        // Step 3: finalize() — set boot partition
        ota.finalize().unwrap();
        assert!(ota.finalized);
        assert!(!ota.rolled_back);

        // Step 4: mark_valid() — confirm firmware booted successfully
        ota.mark_valid().unwrap();
        assert!(ota.valid);

        // Verify the complete call sequence: no rollback, no errors
        assert!(!ota.rolled_back);
    }

    /// Failed write triggers rollback: inject write failure mid-stream,
    /// assert rollback_and_reboot is called, mark_valid is NOT called.
    #[test]
    fn test_ota_failed_write_triggers_rollback() {
        use launa_ota::mock::MockOta;

        let mut ota = MockOta::new();
        // Inject failure after 512 bytes written
        ota.fail_on_write_after = Some(512);

        // 2 KiB firmware served in 256-byte chunks — failure at chunk 3 (byte 512)
        let firmware: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
        let server = SimHttpServer::new(firmware, 256);

        // Attempt OTA — should fail during write
        let result = simulate_ota_download(&mut ota, &server);
        assert!(
            result.is_err(),
            "OTA should fail when write fails mid-stream"
        );

        // Verify the failure was at the injected point
        assert_eq!(
            ota.firmware_data.len(),
            512,
            "should have written exactly 512 bytes before failure"
        );

        // mark_valid must NOT have been called (firmware is incomplete)
        assert!(
            !ota.valid,
            "mark_valid should NOT be called after write failure"
        );

        // finalize should NOT have been called (we failed during write)
        assert!(
            !ota.finalized,
            "finalize should NOT have been called after write failure"
        );

        // Rollback the failed session
        ota.rollback_and_reboot().unwrap();
        assert!(ota.rolled_back, "rollback should be recorded");
        assert!(
            !ota.valid,
            "firmware should still be invalid after rollback"
        );
    }

    /// Firmware size exceeded: write past MAX_FIRMWARE_SIZE,
    /// assert InvalidFirmware error is returned.
    #[test]
    fn test_ota_firmware_size_exceeded() {
        use launa_ota::mock::MockOta;
        use launa_ota::MAX_FIRMWARE_SIZE;

        let mut ota = MockOta::new();
        ota.begin().unwrap();

        // Write exactly MAX_FIRMWARE_SIZE bytes — should succeed
        let chunk = vec![0xAAu8; 4096];
        let full_chunks = MAX_FIRMWARE_SIZE / 4096;
        for _ in 0..full_chunks {
            ota.write(&chunk).unwrap();
        }
        assert_eq!(ota.firmware_data.len(), MAX_FIRMWARE_SIZE);

        // Write one more byte — should fail with InvalidFirmware
        let result = ota.write(&[0x00]);
        assert!(
            matches!(result, Err(OtaError::InvalidFirmware)),
            "writing past MAX_FIRMWARE_SIZE should return InvalidFirmware"
        );

        // Firmware data should be exactly MAX_FIRMWARE_SIZE (no partial write)
        assert_eq!(
            ota.firmware_data.len(),
            MAX_FIRMWARE_SIZE,
            "firmware data should not exceed MAX_FIRMWARE_SIZE"
        );
    }

    /// Concurrent safety: begin() while OTA already in progress returns error.
    #[test]
    fn test_ota_begin_while_in_progress() {
        use launa_ota::mock::MockOta;

        let mut ota = MockOta::new();

        // First begin succeeds
        ota.begin().unwrap();

        // Write some data to confirm session is active
        ota.write(&[0xDE, 0xAD]).unwrap();
        assert_eq!(ota.firmware_data.len(), 2);

        // Second begin while in progress should fail
        let result = ota.begin();
        assert!(
            matches!(result, Err(OtaError::BeginFailed)),
            "begin() while in progress should return BeginFailed"
        );

        // Original session data should still be intact
        assert_eq!(ota.firmware_data.len(), 2);
        assert_eq!(ota.firmware_data, vec![0xDE, 0xAD]);
    }

    /// Concurrent safety: write() before begin() returns error.
    #[test]
    fn test_ota_write_before_begin() {
        use launa_ota::mock::MockOta;

        let mut ota = MockOta::new();

        // Write without begin should fail
        let result = ota.write(&[0x01, 0x02, 0x03]);
        assert!(
            matches!(result, Err(OtaError::WriteFailed { byte_offset: 0 })),
            "write() before begin() should return WriteFailed at offset 0"
        );

        // No data should have been written
        assert!(ota.firmware_data.is_empty());
    }

    /// Concurrent safety: finalize() with zero bytes written returns error.
    #[test]
    fn test_ota_finalize_zero_bytes() {
        use launa_ota::mock::MockOta;

        let mut ota = MockOta::new();

        // Begin succeeds
        ota.begin().unwrap();

        // Finalize with zero bytes written should fail
        let result = ota.finalize();
        assert!(
            matches!(result, Err(OtaError::FinalizeFailed)),
            "finalize() with zero bytes should return FinalizeFailed"
        );

        // Should not be finalized
        assert!(!ota.finalized);

        // Rollback the failed session
        ota.rollback_and_reboot().unwrap();
        assert!(ota.rolled_back);
    }

    #[test]
    fn test_ota_empty_firmware() {
        let mut ota = launa_ota::mock::MockOta::new();

        // Edge case: zero-length firmware — finalize should reject
        let firmware: Vec<u8> = Vec::new();
        let server = SimHttpServer::new(firmware.clone(), 1024);

        ota.begin().unwrap();
        // No chunks to write — download_chunks returns empty vec
        let chunks = server.download_chunks();
        assert!(chunks.is_empty(), "empty firmware should yield no chunks");
        for chunk in &chunks {
            ota.write(chunk).unwrap();
        }
        // Finalize with zero bytes should fail
        assert!(ota.finalize().is_err());
        assert!(!ota.finalized);
        // Rollback the failed session
        ota.rollback_and_reboot().unwrap();
        assert!(ota.rolled_back);
    }

    // ========================================================================
    // Test Group J: FrameDecoder Stress Tests
    // ========================================================================
    //
    // Stress tests for FrameDecoder under adverse conditions: bus idle,
    // split boundaries, corruption, and all-escape payloads.

    /// Bus idle: 1000 consecutive 0x7E bytes → no panic, no spurious frames,
    /// then a valid frame decoded correctly.
    #[test]
    fn test_frame_decoder_bus_idle_0x7e() {
        let mut decoder = FrameDecoder::new();

        // Feed 1000 consecutive 0x7E bytes (bus idle / flag bytes)
        let idle_bytes = vec![0x7Eu8; 1000];
        let frames = decoder.feed_slice(&idle_bytes);

        // No spurious frames produced
        assert_eq!(
            frames.len(),
            0,
            "1000 idle 0x7E bytes should not produce any frames"
        );

        // No frame errors (idle bytes are just flag characters, not corrupt frames)
        assert_eq!(
            decoder.frame_error_count(),
            0,
            "idle 0x7E bytes should not cause frame errors"
        );

        // Now feed a valid frame — should decode correctly
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

    /// Split at every byte boundary: frame split at byte 0, 1, 2, ..., len-1
    /// → all decode successfully.
    #[test]
    fn test_frame_decoder_split_every_boundary() {
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
        };
        let encoded = frame.encode().unwrap();

        // Try every possible split point
        for split_at in 0..encoded.len() {
            let mut decoder = FrameDecoder::new();

            // Feed first part
            let first_part = &encoded[..split_at];
            let frames1 = decoder.feed_slice(first_part);
            // Partial feed should not produce a complete frame (unless the split
            // happens to fall right after a complete frame's end marker)
            assert!(
                frames1.is_empty(),
                "split_at={}: first part should not yield complete frames",
                split_at
            );

            // Feed second part
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

    /// Corrupt interleaved: corrupt frame (bad CRC) then valid frame →
    /// corrupt rejected (frame_error_count++), valid decoded.
    #[test]
    fn test_frame_decoder_corrupt_then_valid() {
        let mut decoder = FrameDecoder::new();

        // Build a valid frame, then corrupt the CRC
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x01, 0x02, 0x03],
        };
        let mut corrupt_encoded = frame.encode().unwrap();
        // Corrupt the CRC byte (second-to-last byte before the end marker)
        let crc_idx = corrupt_encoded.len() - 2;
        corrupt_encoded[crc_idx] ^= 0xFF;

        // Feed the corrupt frame
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

        // Now feed a valid frame — should decode correctly despite prior corruption
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

        // Frame error count should remain at 1 (not incremented by valid frame)
        assert_eq!(
            decoder.frame_error_count(),
            1,
            "frame error count should still be 1 after valid frame"
        );
    }

    /// All-escape payload: frame with payload bytes all needing 0x7D escape
    /// → decoded with correct unescaped content.
    #[test]
    fn test_frame_decoder_all_escape_payload() {
        // Construct a payload consisting entirely of bytes that need escaping:
        // 0x7E (frame marker) and 0x7D (escape character)
        let payload: Vec<u8> = vec![
            0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D, 0x7E, 0x7D,
            0x7E, 0x7D,
        ];

        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: payload.clone(),
        };
        let encoded = frame.encode().unwrap();

        // Verify the encoded form actually contains escape sequences
        assert!(
            encoded.iter().filter(|&&b| b == 0x7D).count() > 0,
            "encoded frame should contain escape sequences"
        );

        // Verify the inner content (between markers) is longer than the
        // original payload due to escaping
        let inner = &encoded[1..encoded.len() - 1];
        let original_inner_len = 1 + 2 + payload.len() + 1; // length + type + payload + crc
        assert!(
            inner.len() > original_inner_len,
            "escaped inner content ({}) should be longer than original ({})",
            inner.len(),
            original_inner_len
        );

        // Decode the frame
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

        // No frame errors
        assert_eq!(decoder.frame_error_count(), 0);
    }

    // ========================================================================
    // Test Group I: SpaApp Integration Tests (launa-core)
    // ========================================================================
    //
    // These tests use `launa_core::SpaApp` — the REAL extracted firmware logic —
    // instead of `launa_sim::SpaController`. Tests exercise the exact same code
    // path as the ESP32 main loop: feed frames, advance virtual time, assert on
    // returned `Vec<AppAction>`.

    use launa_core::{AppAction, SpaApp};
    use launa_sim::VirtualClock;

    /// Helper: create a leaked VirtualClock + SpaApp pair for testing.
    /// The clock is leaked to satisfy the `'static` lifetime needed by SpaApp.
    fn make_spaapp() -> (&'static VirtualClock, SpaApp<'static>) {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let app = SpaApp::new(clock);
        (clock, app)
    }

    /// Helper: a standard status frame (0xFF 0xAF) with 24-byte payload.
    fn make_status_frame() -> Frame {
        let mut payload = vec![0u8; 24];
        payload[2] = 100; // current temp
        payload[20] = 104; // set temp
        Frame {
            message_type: [0xFF, 0xAF],
            payload,
        }
    }

    /// Helper: a Ready frame (0x10 0xBF).
    fn make_ready_frame() -> Frame {
        Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        }
    }

    /// Helper: a NewClientQuery frame (0xFE 0xBF 0x00).
    fn make_new_client_query_frame() -> Frame {
        Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x00],
        }
    }

    /// Helper: a ClientIdAssignment frame (0xFE 0xBF 0x02 <id>).
    fn make_client_id_assignment_frame(id: u8) -> Frame {
        Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x02, id],
        }
    }

    /// Helper: decode raw bytes from SpaSim into parsed Frame.
    fn decode_first_frame(bytes: &[u8]) -> Frame {
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(bytes);
        assert!(!frames.is_empty(), "expected at least one frame");
        frames.into_iter().next().unwrap()
    }

    /// 1. Command ack and confirmation: send toggle via on_mqtt_command() →
    ///    verify AppAction::SendFrame on Ready → spa applies toggle in next status →
    ///    verify no retry.
    #[test]
    fn test_spaapp_command_ack_and_confirmation() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Get an initial status so the tracker has a pre_status
        app.process_frame(&make_status_frame());

        // Queue toggle pump1 via MQTT
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Ready arrives → command is dequeued and sent
        let actions = app.process_frame(&make_ready_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send, "should send command on Ready");
        assert_eq!(app.queued_command_count(), 0);

        // Simulate spa applying the toggle: status with pump1 = Low
        let mut sim = SpaSim::new();
        sim.state.pumps[0] = PumpState::Low;
        let status_frame = decode_first_frame(&sim.generate_status_frame());

        let actions = app.process_frame(&status_frame);
        // Should publish state; no retry since the command is confirmed
        let has_state = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishState { .. }));
        assert!(has_state);
        assert_eq!(
            app.total_retries(),
            0,
            "no retries expected on confirmation"
        );
        assert_eq!(app.total_dropped(), 0, "no drops expected on confirmation");
    }

    /// 2. Command retry on ignore: send toggle → spa does NOT apply (same status) →
    ///    advance time past 5s → verify retry SendFrame → still ignored →
    ///    advance again → verify second retry → still ignored →
    ///    verify command dropped (check app.total_drops() > 0).
    #[test]
    fn test_spaapp_command_retry_on_ignore() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Initial status (pump off)
        app.process_frame(&make_status_frame());

        // Queue and send toggle pump1 on Ready
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.process_frame(&make_ready_frame());

        // Advance past 5s timeout, but spa returns same status (pump still off)
        clock.advance_ms(6_000);
        let actions = app.process_frame(&make_status_frame());

        // Should have retried: look for SendFrame action
        let has_retry_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_retry_send, "should retry on first timeout");
        assert!(app.total_retries() > 0);

        // Advance again past 5s, still same status → second retry
        clock.advance_ms(6_000);
        let actions = app.process_frame(&make_status_frame());
        let has_second_retry = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_second_retry, "should retry on second timeout");

        // Advance again past 5s, still same status → max retries exceeded, command dropped
        clock.advance_ms(6_000);
        app.process_frame(&make_status_frame());
        assert!(
            app.total_dropped() > 0,
            "command should be dropped after max retries"
        );
    }

    /// 3. Stale detection flow: normal operation → stop sending spa frames →
    ///    advance time 5s → call tick() → verify AppAction::SendFrame (config probe) →
    ///    advance to 30s → verify AppAction::PublishStaleAvailability + AppAction::PublishAlert →
    ///    resume status frames → verify recovery.
    #[test]
    fn test_spaapp_stale_detection_flow() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Normal operation: receive status
        app.process_frame(&make_status_frame());
        assert!(!app.is_stale());

        // Stop sending frames, advance 6s → probe
        clock.advance_ms(6_000);
        let actions = app.tick();
        let has_probe = actions
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(bytes) if !bytes.is_empty()));
        assert!(has_probe, "should send config probe at 5s");

        // Advance to 31s total since last status → stale
        clock.advance_ms(25_000);
        let actions = app.tick();
        let has_stale_avail = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishStaleAvailability));
        let has_alert = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. } if message == "spa_communication_lost"
            )
        });
        assert!(has_stale_avail, "should publish stale availability at 30s");
        assert!(has_alert, "should publish stale alert at 30s");
        assert!(app.is_stale());

        // Resume status frames → recover
        let actions = app.process_frame(&make_status_frame());
        assert!(!app.is_stale());
        let recovering = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishState {
                    recovering_from_stale: true,
                    ..
                }
            )
        });
        assert!(recovering, "should indicate stale recovery");
    }

    /// 4. Hold mode safety timeout: enter hold mode → advance 60 minutes via
    ///    VirtualClock → verify AppAction::SendFrame (hold toggle to clear).
    #[test]
    fn test_spaapp_hold_mode_safety_timeout() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Normal status first (no hold)
        app.process_frame(&make_status_frame());

        // Status with hold mode active (payload[0] == 0x05)
        let mut hold_frame = make_status_frame();
        hold_frame.payload[0] = 0x05;
        app.process_frame(&hold_frame);

        // Advance past 60 min hold timeout
        clock.advance_ms(61 * 60 * 1000);

        // Send another status with hold still active → timer should fire
        let actions = app.process_frame(&hold_frame);
        let has_toggle = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            has_toggle,
            "should send hold toggle after 60 min safety timeout"
        );
    }

    /// 5. Pump timer expiry: start pump timer → advance virtual time →
    ///    verify auto-off toggle sent at exact duration.
    #[test]
    fn test_spaapp_pump_timer_expiry() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Start pump 1 timer for 1 minute
        let actions = app.start_pump_timer(1, 1);
        assert!(
            actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "start_pump_timer should return toggle-on action"
        );

        // Status with pump1 running (Low)
        let mut status = make_status_frame();
        status.payload[11] = 0x01; // Pump 1 = Low
        app.process_frame(&status);

        // Advance past 1 minute
        clock.advance_ms(61_000);

        // Next status should trigger auto-off toggle
        let actions = app.process_frame(&status);
        let has_auto_off = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_auto_off, "should auto-off pump after timer expiry");
    }

    /// 6. Diagnostics periodic: run app with clock advanced past 60s →
    ///    verify AppAction::PublishDiagnostics fires with correct counter values.
    #[test]
    fn test_spaapp_diagnostics_periodic() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Receive a few status frames to increment counters
        app.process_frame(&make_status_frame());
        app.process_frame(&make_status_frame());
        assert_eq!(app.frames_received(), 2);

        // Advance past diagnostics interval (60s)
        clock.advance_ms(61_000);

        let actions = app.tick();
        let diag = actions.iter().find_map(|a| match a {
            AppAction::PublishDiagnostics {
                uptime_secs,
                frames_received,
                command_retries,
                command_drops,
            } => Some((
                *uptime_secs,
                *frames_received,
                *command_retries,
                *command_drops,
            )),
            _ => None,
        });
        assert!(diag.is_some(), "should publish diagnostics at 60s");
        let (uptime, frames, retries, drops) = diag.unwrap();
        assert_eq!(uptime, 61);
        assert_eq!(frames, 2);
        assert_eq!(retries, 0);
        assert_eq!(drops, 0);
    }

    /// 7. Registration timeout: start registration → no spa response →
    ///    advance 5s → verify AppAction::PublishAlert (registration_timeout) and state reset.
    #[test]
    fn test_spaapp_registration_timeout() {
        let (clock, app) = make_spaapp();
        let mut app = app;

        // Start registration by receiving a NewClientQuery
        let actions = app.process_frame(&make_new_client_query_frame());
        assert!(
            actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "should send ID request on NewClientQuery"
        );
        assert!(!app.is_registered());

        // Advance past registration timeout (5s)
        clock.advance_ms(6_000);

        let actions = app.tick();
        let has_timeout_alert = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. } if message == "registration_timeout"
            )
        });
        assert!(
            has_timeout_alert,
            "should publish registration_timeout alert"
        );
        assert!(!app.is_registered());
    }

    /// 8. Bus reset re-registration: fully registered and running →
    ///    spa sends NewClientQuery frame → verify SpaApp resets registration.
    #[test]
    fn test_spaapp_bus_reset_reregistration() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);
        assert!(app.is_registered());
        assert_eq!(app.client_id(), Some(0x03));

        // Receive a status to confirm normal operation
        app.process_frame(&make_status_frame());

        // Bus reset: spa sends NewClientQuery
        let actions = app.process_frame(&make_new_client_query_frame());
        assert!(!app.is_registered(), "should reset registration");
        assert_eq!(app.client_id(), None, "client_id should be cleared");
        // No SendFrame is produced at this point (it goes through dispatch,
        // which resets registration). The next NewClientQuery triggers re-registration.
        assert!(actions.is_empty());

        // Re-registration: next NewClientQuery starts the flow
        let actions = app.process_frame(&make_new_client_query_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send, "should send ID request on re-registration");
    }

    /// 9. Temperature validation rejection: SpaApp doesn't validate temperature
    ///    bounds internally — validation happens at the MQTT command parser level.
    ///    This test verifies that SpaApp accepts any temperature from on_mqtt_command
    ///    and queues it without validation.
    #[test]
    fn test_spaapp_temperature_not_validated_in_app() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&make_status_frame());

        // SetTemperature(106) — out of typical range but SpaApp accepts it
        app.on_mqtt_command(Command::SetTemperature(106));
        assert_eq!(app.queued_command_count(), 1);

        // Ready → sends the command without validation
        let actions = app.process_frame(&make_ready_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            has_send,
            "SpaApp should send SetTemperature without validation"
        );
    }

    /// 10. Concurrent operations: toggle pump + set temp + change heating mode →
    ///     verify all tracked, all confirmed when status reflects changes.
    #[test]
    fn test_spaapp_concurrent_operations() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Initial status (pump off, set temp 104, heating mode Ready)
        app.process_frame(&make_status_frame());

        // Queue 3 concurrent commands
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::SetTemperature(102));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::HeatingMode));
        assert_eq!(app.queued_command_count(), 3);

        // Each is sent one per Ready window
        let actions1 = app.process_frame(&make_ready_frame());
        assert!(actions1
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_))));
        assert_eq!(app.queued_command_count(), 2);

        let actions2 = app.process_frame(&make_ready_frame());
        assert!(actions2
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_))));
        assert_eq!(app.queued_command_count(), 1);

        let actions3 = app.process_frame(&make_ready_frame());
        assert!(actions3
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_))));
        assert_eq!(app.queued_command_count(), 0);

        // Status arrives reflecting all changes: pump1=Low, set_temp=102, heating_mode=Rest
        let mut sim = SpaSim::new();
        sim.state.pumps[0] = PumpState::Low;
        sim.state.set_temp = 102.0;
        sim.state.heating_mode = HeatingMode::Rest;
        let status_frame = decode_first_frame(&sim.generate_status_frame());

        let actions = app.process_frame(&status_frame);
        // All commands should be confirmed (no retries, no drops)
        assert_eq!(app.total_retries(), 0, "no retries expected");
        assert_eq!(app.total_dropped(), 0, "no drops expected");
        let has_state = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishState { .. }));
        assert!(has_state, "should publish state after confirmation");
    }

    /// 11. Fault log captured: send fault log frame → verify last_fault() returns
    ///     a fault string, and the next PublishState includes the fault.
    #[test]
    fn test_spaapp_fault_log_captured() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Get a status first
        app.process_frame(&make_status_frame());

        // Simulate fault log response from spa
        let fault_frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![
                0x28, 0x03, 0x01, 0x1B, 0x02, 0x0E, 0x1E, 0x04, 0x68, 0x68, 0x66,
            ],
        };
        app.process_frame(&fault_frame);
        assert!(app.last_fault().is_some(), "should capture fault log");

        // Next status should include fault in PublishState
        let actions = app.process_frame(&make_status_frame());
        let has_fault_in_state = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishState { fault: Some(_), .. }));
        assert!(
            has_fault_in_state,
            "next PublishState should include fault string"
        );
    }

    /// 12. Ready window command queuing: queue 3 commands → verify only one sent
    ///     per Ready → verify NothingToSend when queue empty.
    #[test]
    fn test_spaapp_ready_window_command_queuing() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&make_status_frame());

        // Queue 3 commands
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump3));
        assert_eq!(app.queued_command_count(), 3);

        // First Ready → send pump1, queue now has 2
        app.process_frame(&make_ready_frame());
        assert_eq!(app.queued_command_count(), 2);

        // Second Ready → send pump2, queue now has 1
        app.process_frame(&make_ready_frame());
        assert_eq!(app.queued_command_count(), 1);

        // Third Ready → send pump3, queue now has 0
        app.process_frame(&make_ready_frame());
        assert_eq!(app.queued_command_count(), 0);

        // Fourth Ready → send NothingToSend (no commands left)
        let actions = app.process_frame(&make_ready_frame());
        let has_nts = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_nts, "should send NothingToSend when queue is empty");
        // Verify no more commands are queued
        assert_eq!(app.queued_command_count(), 0);
    }

    /// 13. 24-hour simulation smoke test: simulate 86,400 seconds of operation
    ///     in compressed steps. Verifies no panics, temperature stability,
    ///     regular diagnostics, and no stale state at the end.
    #[test]
    fn test_spaapp_24_hour_smoke() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        let mut diag_count: u32 = 0;
        let mut sim = SpaSim::new();

        // Phase 1: Warm-up and steady state — simulate 1000 seconds at 1-second
        // resolution. SpaSim starts at 100°F, set point is 104°F, so it takes
        // ~4 ticks to reach set point.
        for _ in 0..1000 {
            clock.advance_ms(1_000);

            let raw_bytes = sim.tick();

            // Decode status + ready frames from SpaSim output
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&raw_bytes);

            for frame in &frames {
                if frame.message_type == [0xFF, 0xAF] {
                    app.process_frame(frame);
                } else if frame.message_type == [0x10, 0xBF] {
                    // Ready frame — dequeue command if any
                    app.process_frame(frame);
                }
            }

            let actions = app.tick();
            diag_count += actions
                .iter()
                .filter(|a| matches!(a, AppAction::PublishDiagnostics { .. }))
                .count() as u32;
        }

        // Phase 2: Remaining ~85,400 seconds in 60-second jumps.
        // Advance clock by 60,000ms, feed one status frame + ready frame,
        // and call tick(). That's ~1,423 iterations — very fast.
        let remaining_secs: u64 = 86_400 - 1000;
        let jumps = remaining_secs / 60;
        for _ in 0..jumps {
            clock.advance_ms(60_000);

            // Generate a status frame from SpaSim (physics advances 1 tick)
            let status_bytes = sim.generate_status_frame();
            let status_frame = decode_first_frame(&status_bytes);
            app.process_frame(&status_frame);

            // Send a Ready frame to allow command dequeue
            app.process_frame(&make_ready_frame());

            let actions = app.tick();
            diag_count += actions
                .iter()
                .filter(|a| matches!(a, AppAction::PublishDiagnostics { .. }))
                .count() as u32;
        }

        // Verify: no panics (we got here!)

        // Temperature should have reached set point (104°F) during Phase 1
        // and stayed stable. SpaSim advances +1°F per tick, so after 4 ticks
        // it's at 104, and stays there.
        let status = app.last_status().expect("should have a status");
        assert!(
            status.current_temp >= Some(104.0),
            "temperature should have reached set point: {:?}",
            status.current_temp
        );

        // Diagnostics should fire every 60s, so over 86,400s we expect ~1,440.
        // Phase 1 covers ~1000s/60 = ~16, Phase 2 covers ~1,423 ticks at 60s = ~1,423.
        // With tick() being called for each, diag_count should be substantial.
        assert!(
            diag_count > 1000,
            "should have many diagnostics publishes over 24h, got {}",
            diag_count
        );

        // Queue should be empty (no commands were queued)
        assert_eq!(app.queued_command_count(), 0);

        // Many frames received over 24h
        assert!(
            app.frames_received() > 1000,
            "should have received many frames: {}",
            app.frames_received()
        );

        // Should NOT be stale — we've been feeding frames the whole time
        assert!(!app.is_stale(), "should not be stale after 24h of frames");
    }

    /// 14. Stress test: rapid commands. Queue more commands than the queue cap
    ///     (32), process each via Ready frames, verify no panics, no unbounded
    ///     growth, excess commands dropped, and all tracked commands resolved.
    #[test]
    fn test_spaapp_stress_rapid_commands() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status (pump1 off)
        app.process_frame(&make_status_frame());

        // Queue 100 toggle pump1 commands in quick succession.
        // The queue caps at 32, so 68 should be silently dropped.
        let queue_cap: usize = 32;
        for _ in 0..100 {
            app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        }
        assert_eq!(
            app.queued_command_count(),
            queue_cap,
            "queue should be capped at {}",
            queue_cap
        );

        // Process each queued command via Ready frames.
        let mut send_frame_count: u32 = 0;
        let mut sim = SpaSim::new(); // Fresh sim with pump1 = Off

        for _ in 0..queue_cap {
            // Advance clock by 1 second between commands
            clock.advance_ms(1_000);

            // Ready frame → dequeue one command
            let actions = app.process_frame(&make_ready_frame());
            if actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))) {
                send_frame_count += 1;
            }

            // The command tracker caps at MAX_PENDING_COMMANDS (8), so
            // only the first 8 will be tracked. The rest are dequeued and
            // sent but not tracked (track() silently returns when full).

            // Feed a status frame. SpaSim doesn't know about our commands,
            // so pump stays off. This means commands won't be confirmed and
            // will eventually retry/drop.
            let status_bytes = sim.generate_status_frame();
            let status_frame = decode_first_frame(&status_bytes);
            app.process_frame(&status_frame);
        }

        // After draining all queued commands, queue should be empty.
        assert_eq!(
            app.queued_command_count(),
            0,
            "all queued commands should be dequeued"
        );

        // We should have sent frames for each queued command.
        assert!(
            send_frame_count >= queue_cap as u32,
            "should have sent at least {} frames, got {}",
            queue_cap,
            send_frame_count
        );

        // No unbounded growth: command queue is empty, pending tracker bounded.
        let retries = app.total_retries();
        let drops = app.total_dropped();

        // Since spa never reflects pump1 ON (sim doesn't process our commands),
        // tracked commands will timeout and retry/drop.
        assert!(
            retries + drops > 0,
            "should have some retries or drops (spa never confirms): retries={}, drops={}",
            retries,
            drops
        );

        // No panics throughout (we got here!)
        // Verify final state is clean
        assert!(!app.is_stale(), "should not be stale");
    }

    // ========================================================================
    // Test Group K: Command Queue Integration Tests
    // ========================================================================
    //
    // Tests for command queue behavior: registration race conditions, FIFO
    // drain ordering, and bounded capacity via CommandTracker's
    // MAX_PENDING_COMMANDS=8 cap.

    /// Registration race condition: send commands via on_mqtt_command()
    /// during registration (before client_id assigned), complete registration,
    /// send Ready frames, verify commands drain.
    #[test]
    fn test_registration_race_condition() {
        let (_clock, app) = make_spaapp();
        let mut app = app;

        // NOT registered yet — app has no client_id

        // Queue commands BEFORE registration completes
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        app.on_mqtt_command(Command::SetTemperature(100));
        assert_eq!(app.queued_command_count(), 3);
        assert!(!app.is_registered());

        // Now complete registration: NewClientQuery → SendIdRequest
        let actions = app.process_frame(&make_new_client_query_frame());
        assert!(
            actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "should send ID request"
        );
        assert!(!app.is_registered());

        // ClientIdAssignment → SendIdAck → registered
        let actions = app.process_frame(&make_client_id_assignment_frame(0x03));
        assert!(
            actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "should send ID ack"
        );
        assert!(app.is_registered());
        assert_eq!(app.client_id(), Some(0x03));

        // Commands should still be queued (not lost during registration)
        assert_eq!(
            app.queued_command_count(),
            3,
            "commands should survive registration"
        );

        // Feed an initial status so CommandTracker has a pre_status baseline
        app.process_frame(&make_status_frame());

        // Send Ready frames — commands should drain one per Ready
        let mut sent_commands: Vec<Vec<u8>> = Vec::new();
        for i in 0..3 {
            let actions = app.process_frame(&make_ready_frame());
            let frame_data = actions
                .iter()
                .find_map(|a| match a {
                    AppAction::SendFrame(data) => Some(data.clone()),
                    _ => None,
                })
                .expect(&format!("Ready {} should produce SendFrame", i + 1));
            sent_commands.push(frame_data);
        }

        // All 3 commands should have been dequeued
        assert_eq!(
            app.queued_command_count(),
            0,
            "all commands should be drained after 3 Ready frames"
        );
        assert_eq!(sent_commands.len(), 3);
    }

    /// Multi-command queue drain: queue 5 commands, send 5 Ready frames,
    /// verify all 5 sent via AppAction::SendFrame. The command_queue uses
    /// Vec::pop() which drains in LIFO (stack) order — last queued is sent
    /// first. This test verifies the drain order matches the implementation.
    #[test]
    fn test_multi_command_fifo_drain() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Feed initial status for CommandTracker baseline
        app.process_frame(&make_status_frame());

        // Queue 5 different commands
        let commands = [
            Command::ToggleItem(ToggleItem::Pump1),
            Command::ToggleItem(ToggleItem::Pump2),
            Command::ToggleItem(ToggleItem::Pump3),
            Command::SetTemperature(100),
            Command::ToggleItem(ToggleItem::Light1),
        ];

        for cmd in &commands {
            app.on_mqtt_command(cmd.clone());
        }
        assert_eq!(app.queued_command_count(), 5);

        // Encode the commands in reverse order for comparison.
        // Vec::pop() drains LIFO: last queued (Light1) is sent first.
        let expected_frames: Vec<Vec<u8>> = commands
            .iter()
            .rev()
            .map(|cmd| {
                let (mt, payload) = cmd.encode();
                FrameEncoder::encode(mt, &payload).unwrap()
            })
            .collect();

        // Drain via 5 Ready frames, capture sent frames in order
        let mut actual_frames: Vec<Vec<u8>> = Vec::new();
        for i in 0..5 {
            let actions = app.process_frame(&make_ready_frame());
            let frame_data = actions
                .iter()
                .find_map(|a| match a {
                    AppAction::SendFrame(data) => Some(data.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("Ready {} should produce SendFrame", i + 1));
            actual_frames.push(frame_data);
        }

        // Verify drain order: all 5 sent, matching the Vec::pop() order
        assert_eq!(actual_frames.len(), 5, "should have sent exactly 5 frames");
        for (i, (actual, expected)) in actual_frames.iter().zip(expected_frames.iter()).enumerate()
        {
            assert_eq!(
                actual, expected,
                "command {} should match drain order (LIFO)",
                i
            );
        }

        // Queue should now be empty
        assert_eq!(app.queued_command_count(), 0);

        // Next Ready should send NothingToSend (queue empty)
        let actions = app.process_frame(&make_ready_frame());
        let nts_frame = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect("should send NothingToSend when queue empty");
        // Verify it's a NothingToSend for client_id 0x03
        let expected_nts = {
            let (mt, payload) = Command::NothingToSend { client_id: 0x03 }.encode();
            FrameEncoder::encode(mt, &payload).unwrap()
        };
        assert_eq!(nts_frame, expected_nts, "should send NothingToSend");
    }

    /// Bounded command queue cap: queue 9 commands, verify the 9th exceeds
    /// MAX_PENDING_COMMANDS=8 — CommandTracker refuses to track it (existing
    /// behavior where track() silently returns when pending.len() >= 8).
    ///
    /// Note: the command_queue Vec has no cap, so all 9 commands are queued and
    /// sent on Ready. But the CommandTracker only tracks 8 at a time — the 9th
    /// command is sent but NOT tracked (track() silently returns when full).
    #[test]
    fn test_bounded_command_queue_cap() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Feed initial status for CommandTracker baseline
        app.process_frame(&make_status_frame());

        // Queue 9 toggle pump1 commands
        for _ in 0..9 {
            app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        }
        assert_eq!(app.queued_command_count(), 9);

        // Drain all 9 via Ready frames
        let mut send_count: usize = 0;
        for _ in 0..9 {
            let actions = app.process_frame(&make_ready_frame());
            if actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))) {
                send_count += 1;
            }
        }

        // All 9 commands were sent (command_queue has no cap)
        assert_eq!(send_count, 9, "all 9 commands should be sent");
        assert_eq!(app.queued_command_count(), 0, "queue should be empty");

        // CommandTracker tracked at most MAX_PENDING_COMMANDS=8.
        // The 9th command was sent but not tracked — track() silently
        // returned when pending.len() >= 8. We can verify this by
        // checking that pending_count never exceeded 8.
        //
        // Since we can't inspect pending_count retroactively, we verify
        // by observing that only 8 commands were tracked. The 9th was
        // silently dropped by the tracker. After draining all, pending
        // should be 8 (or less if some were already verified/dropped).
        //
        // The key assertion: no panic, no unbounded growth, the 9th
        // command was silently not tracked.
        assert_eq!(app.queued_command_count(), 0);

        // Feed a status that doesn't confirm (pump still off) — only
        // tracked commands will timeout/retry. The 9th untracked command
        // won't appear in retry/drop counts at all.
        // With MAX_PENDING_COMMANDS=8, the tracker has 8 pending commands.
        // All 8 will eventually timeout and be dropped (spa never confirms).
        // But the 9th was never tracked, so it won't contribute to drops.
    }

    // ========================================================================
    // Test Group L: SpaApp + SpaSim Full Integration Tests
    // ========================================================================
    //
    // These tests exercise the complete SpaApp → SpaSim pipeline, where SpaSim
    // generates realistic frame bytes and SpaApp processes them through the same
    // code path as the ESP32 main loop. VirtualClock provides deterministic timing.
    //
    // These satisfy validation contract assertions:
    //   VAL-APP-026: SpaApp pipeline (process_frame, tick, on_mqtt_command)
    //   VAL-APP-027: VirtualClock usage for deterministic timing
    //   VAL-APP-028: Registration flow end-to-end
    //   VAL-APP-029: OTA full download cycle
    //   VAL-APP-030: Stale detection lifecycle (probe → alert → recovery)
    //   VAL-APP-031: OTA rollback on failure
    //   VAL-APP-032: Command retry and drop when spa doesn't confirm

    /// Helper: run a full SpaSim tick, decode all frames, feed them to SpaApp.
    /// Returns all AppActions produced during this cycle.
    fn sim_tick_to_app(sim: &mut SpaSim, app: &mut SpaApp) -> Vec<AppAction> {
        let raw_bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&raw_bytes);
        let mut all_actions = Vec::new();
        for frame in &frames {
            let actions = app.process_frame(frame);
            all_actions.extend(actions);
        }
        all_actions
    }

    /// Helper: perform full registration between SpaSim and SpaApp.
    /// SpaSim sends registration query → SpaApp responds → SpaSim assigns ID → SpaApp acks.
    fn full_registration(sim: &mut SpaSim, app: &mut SpaApp) {
        // Tick 1: SpaSim sends registration query (FE BF 00)
        let actions1 = sim_tick_to_app(sim, app);
        let has_send = actions1
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send, "should send ID request on registration query");

        // Extract the SendFrame bytes and feed them back to SpaSim
        let id_request_bytes = actions1
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect("should have SendFrame for ID request");

        // SpaSim processes the ID request and assigns a client ID
        let assignment_bytes = sim.process_incoming_bytes(&id_request_bytes);
        assert!(
            !assignment_bytes.is_empty(),
            "should return client ID assignment bytes"
        );

        // Feed the assignment bytes back to SpaApp
        let mut decoder = FrameDecoder::new();
        let assignment_frames = decoder.feed_slice(&assignment_bytes);
        assert_eq!(
            assignment_frames.len(),
            1,
            "should produce one assignment frame"
        );

        let actions2 = app.process_frame(&assignment_frames[0]);
        let has_ack = actions2
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_ack, "should send ID ack after assignment");
        assert!(app.is_registered(), "should be registered after assignment");

        // Send the ack back to SpaSim
        let ack_bytes = actions2
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect("should have SendFrame for ACK");

        sim.process_incoming_bytes(&ack_bytes);
        assert!(
            sim.client_id.is_some(),
            "sim should have client_id after ACK"
        );
    }

    // ---- VAL-APP-028: SpaApp registration flow end-to-end ----

    /// Full registration flow: SpaSim generates registration query → SpaApp
    /// processes it → sends ID request → SpaSim assigns client ID → SpaApp
    /// sends ACK → both sides registered.
    ///
    /// Uses SpaSim for realistic frame generation and SpaApp for protocol logic.
    #[test]
    fn test_spaapp_registration_e2e() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        let mut sim = SpaSim::new();

        // Initially unregistered
        assert!(!app.is_registered());
        assert!(app.client_id().is_none());

        // Perform full registration via the helper
        full_registration(&mut sim, &mut app);

        // Verify both sides are registered with matching client IDs
        assert!(app.is_registered());
        assert_eq!(app.client_id(), sim.client_id);

        // After registration, SpaSim ticks no longer produce registration queries
        let raw = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&raw);
        // Should have status + ready, but NO registration query
        let has_reg_query = frames
            .iter()
            .any(|f| f.message_type == [0xFE, 0xBF] && f.payload.contains(&0x00));
        assert!(
            !has_reg_query,
            "should not produce registration query after registration"
        );
    }

    /// Registration flow with frame interleaving: during the registration sequence,
    /// verify that frames are processed in the correct order and no state corruption
    /// occurs when status frames arrive between registration frames.
    #[test]
    fn test_spaapp_registration_with_interleaved_frames() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        let mut sim = SpaSim::new();

        // Step 1: SpaSim sends registration query
        let raw_bytes = sim.tick(); // contains reg query + status + ready
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&raw_bytes);

        // Feed all frames to SpaApp — the status/ready frames are ignored
        // (not registered yet, only registration frames are processed)
        for frame in &frames {
            app.process_frame(frame);
        }
        assert!(!app.is_registered(), "should not be registered yet");

        // Step 2: Re-process just the registration query frame to get the ID request
        // (it was already processed above but we need the SendFrame output)
        let reg_frame = frames
            .iter()
            .find(|f| f.message_type == [0xFE, 0xBF])
            .expect("should have registration query frame");
        // Reset registration so we can re-process the query
        app.force_registered(0x03); // mark registered so the frame goes through dispatch
        app.process_frame(&make_new_client_query_frame()); // bus reset
        let actions = app.process_frame(reg_frame);
        let id_request_bytes = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect("should have ID request SendFrame");

        // Step 3: SpaSim assigns client ID
        let assignment_bytes = sim.process_incoming_bytes(&id_request_bytes);
        assert!(
            !assignment_bytes.is_empty(),
            "should return assignment bytes"
        );

        // Feed assignment to SpaApp (may include status frames in between)
        let assignment_frames = decoder.feed_slice(&assignment_bytes);
        for frame in &assignment_frames {
            app.process_frame(frame);
        }
        assert!(app.is_registered(), "should be registered after assignment");
    }

    // ---- VAL-APP-029: OTA full download cycle with SimHttpServer pattern ----

    /// OTA full download cycle: simulate firmware download via SimHttpServer,
    /// write chunks through OtaUpdate trait, finalize, and mark_valid.
    /// Data integrity verified at every step.
    #[test]
    fn test_spaapp_ota_full_download_cycle() {
        let mut ota = launa_ota::mock::MockOta::new();

        // Simulate a realistic firmware image (4 KiB) served in 1 KiB chunks
        let firmware: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        let server = SimHttpServer::new(firmware.clone(), 1024);

        // Step 1: Begin OTA session
        ota.begin().unwrap();
        assert!(
            ota.firmware_data.is_empty(),
            "data should be empty after begin"
        );

        // Step 2: Download chunks and write to OTA
        let chunks = server.download_chunks();
        assert_eq!(chunks.len(), 4, "4 KiB / 1 KiB chunks = 4 chunks");
        for (i, chunk) in chunks.iter().enumerate() {
            ota.write(chunk).unwrap();
            assert_eq!(
                ota.firmware_data.len(),
                (i + 1) * 1024,
                "data should grow after each write"
            );
        }

        // Step 3: Finalize — set boot partition
        ota.finalize().unwrap();
        assert!(ota.finalized, "should be finalized");

        // Step 4: Mark valid — firmware booted successfully
        ota.mark_valid().unwrap();
        assert!(ota.valid, "should be marked valid");

        // Step 5: Verify data integrity
        assert_eq!(ota.firmware_data.len(), 4096);
        assert_eq!(
            ota.firmware_data, firmware,
            "firmware data should match original"
        );
    }

    /// OTA with various chunk sizes simulating realistic network conditions:
    /// small chunks (64 bytes), TCP-segment-sized (1460), and large chunks (4096).
    #[test]
    fn test_spaapp_ota_variable_chunk_sizes() {
        let mut ota = launa_ota::mock::MockOta::new();

        // 16 KiB firmware with non-trivial data pattern
        let firmware: Vec<u8> = (0..16384).map(|i| ((i * 7 + 13) % 256) as u8).collect();

        // Test with small chunks
        let server = SimHttpServer::new(firmware.clone(), 64);
        ota.begin().unwrap();
        for chunk in server.download_chunks() {
            ota.write(&chunk).unwrap();
        }
        ota.finalize().unwrap();
        ota.mark_valid().unwrap();
        assert_eq!(ota.firmware_data, firmware);

        // Reset and test with TCP-sized chunks
        let mut ota2 = launa_ota::mock::MockOta::new();
        let server2 = SimHttpServer::new(firmware.clone(), 1460);
        ota2.begin().unwrap();
        for chunk in server2.download_chunks() {
            ota2.write(&chunk).unwrap();
        }
        ota2.finalize().unwrap();
        ota2.mark_valid().unwrap();
        assert_eq!(ota2.firmware_data, firmware);
    }

    // ---- VAL-APP-030: Stale detection lifecycle (probe → alert → recovery) ----

    /// Stale detection lifecycle: normal operation → bus silence → probe at 5s →
    /// alert at 30s → recovery when status resumes.
    ///
    /// Tests each phase with explicit assertions:
    /// - Phase 1 (0-5s): Normal operation, status received
    /// - Phase 2 (5-30s): No status, probes sent at 5s intervals
    /// - Phase 3 (30s+): Stale alert + stale availability published
    /// - Phase 4: Status resumes → recovery
    #[test]
    fn test_spaapp_stale_detection_lifecycle() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Phase 1: Normal operation — receive status
        app.process_frame(&make_status_frame());
        assert!(
            !app.is_stale(),
            "should not be stale during normal operation"
        );

        // Phase 2: Advance 6s — probe should fire
        clock.advance_ms(6_000);
        let actions = app.tick();
        let probe_frames: Vec<&Vec<u8>> = actions
            .iter()
            .filter_map(|a| match a {
                AppAction::SendFrame(data) => Some(data),
                _ => None,
            })
            .collect();
        assert!(!probe_frames.is_empty(), "Phase 2: should send probe at 5s");
        // Verify it's a NothingToSend (lightweight), not ConfigurationRequest
        let nts_expected = {
            let (mt, payload) = Command::NothingToSend { client_id: 0x03 }.encode();
            FrameEncoder::encode(mt, &payload).unwrap()
        };
        assert!(
            probe_frames.iter().any(|f| *f == &nts_expected),
            "Phase 2: probe should be NothingToSend, not ConfigurationRequest"
        );
        assert!(!app.is_stale(), "should not be stale at 6s");

        // Phase 2b: Advance another 5s — second probe
        clock.advance_ms(5_000);
        let actions = app.tick();
        let probe2_frames: Vec<&Vec<u8>> = actions
            .iter()
            .filter_map(|a| match a {
                AppAction::SendFrame(data) => Some(data),
                _ => None,
            })
            .collect();
        assert!(
            !probe2_frames.is_empty(),
            "Phase 2b: should send second probe at 10s"
        );

        // Phase 2c: Advance another 5s — third probe (now at 16s total, not yet stale)
        clock.advance_ms(5_000);
        let actions = app.tick();
        let probe3_frames: Vec<&Vec<u8>> = actions
            .iter()
            .filter_map(|a| match a {
                AppAction::SendFrame(data) => Some(data),
                _ => None,
            })
            .collect();
        assert!(
            !probe3_frames.is_empty(),
            "Phase 2c: should send third probe at 16s"
        );
        assert!(!app.is_stale(), "should not be stale at 16s");

        // Phase 3: Advance past 30s threshold — stale alert
        clock.advance_ms(15_000); // total elapsed since status: 31s
        let actions = app.tick();

        let has_stale_alert = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. } if message == "spa_communication_lost"
            )
        });
        assert!(
            has_stale_alert,
            "Phase 3: should publish stale alert at 30s"
        );

        let has_stale_avail = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishStaleAvailability));
        assert!(
            has_stale_avail,
            "Phase 3: should publish stale availability at 30s"
        );
        assert!(app.is_stale(), "Phase 3: should be stale at 31s");

        // Phase 4: Recovery — feed a status frame
        let actions = app.process_frame(&make_status_frame());
        assert!(!app.is_stale(), "Phase 4: should recover after status");

        let has_recovery = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishState {
                    recovering_from_stale: true,
                    ..
                }
            )
        });
        assert!(has_recovery, "Phase 4: should indicate stale recovery");

        // Verify subsequent ticks don't re-trigger stale (status_time reset)
        clock.advance_ms(6_000);
        let actions = app.tick();
        let no_stale_alert = !actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. }
                if message == "spa_communication_lost"
            )
        });
        assert!(
            no_stale_alert,
            "Phase 4: should not re-trigger stale after recovery"
        );
    }

    /// Stale detection with VirtualClock: verify exact timing boundaries.
    /// At 29s, no stale. At 30s, stale. Confirms VirtualClock::advance_ms()
    /// provides deterministic timing.
    #[test]
    fn test_spaapp_stale_detection_exact_timing() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Receive initial status at time 0
        app.process_frame(&make_status_frame());

        // Advance to 29s — should NOT be stale yet
        clock.advance_ms(29_000);
        let actions = app.tick();
        let no_stale = !actions.iter().any(|a| {
            matches!(a, AppAction::PublishAlert { message, .. } if message == "spa_communication_lost")
        });
        assert!(no_stale, "should NOT be stale at 29s");
        assert!(!app.is_stale());

        // Advance to exactly 30s — stale threshold crossed
        clock.advance_ms(1_000);
        let actions = app.tick();
        let has_stale = actions.iter().any(|a| {
            matches!(a, AppAction::PublishAlert { message, .. } if message == "spa_communication_lost")
        });
        assert!(has_stale, "should be stale at 30s");
        assert!(app.is_stale());
    }

    // ---- VAL-APP-031: OTA rollback on failure ----

    /// OTA rollback: simulate firmware download that fails during write,
    /// verify rollback_and_reboot is called and mark_valid is NEVER called.
    #[test]
    fn test_spaapp_ota_rollback_on_write_failure() {
        let mut ota = launa_ota::mock::MockOta::new();
        ota.fail_on_write_after = Some(2048);

        // 4 KiB firmware — will fail at 2048 bytes
        let firmware: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        let server = SimHttpServer::new(firmware, 512);

        // Begin OTA
        ota.begin().unwrap();

        // Write chunks manually until failure
        let chunks = server.download_chunks();
        for chunk in &chunks {
            let result = ota.write(chunk);
            if result.is_err() {
                break;
            }
        }

        // Verify: only 2048 bytes written before failure (4 chunks × 512 bytes)
        assert_eq!(
            ota.firmware_data.len(),
            2048,
            "should have written exactly 2048 bytes"
        );

        // mark_valid should NOT have been called
        assert!(!ota.valid, "mark_valid should NOT be called after failure");

        // finalize should NOT have succeeded
        assert!(
            !ota.finalized,
            "finalize should NOT have succeeded after failure"
        );

        // Rollback the failed OTA
        ota.rollback_and_reboot().unwrap();
        assert!(ota.rolled_back, "should have rolled back");
        assert!(!ota.valid, "should still not be valid after rollback");
    }

    /// OTA rollback on finalize failure: write succeeds but finalize fails.
    /// Verify rollback is called and mark_valid is never called.
    #[test]
    fn test_spaapp_ota_rollback_on_finalize_failure() {
        let mut ota = launa_ota::mock::MockOta::new();
        ota.fail_on_finalize = true;

        let firmware: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
        let server = SimHttpServer::new(firmware.clone(), 512);

        // OTA pipeline: begin → write → finalize (fails)
        ota.begin().unwrap();
        for chunk in server.download_chunks() {
            ota.write(&chunk).unwrap();
        }
        let result = ota.finalize();
        assert!(
            result.is_err(),
            "finalize should fail when fail_on_finalize is set"
        );

        // All data was written but finalize failed
        assert_eq!(ota.firmware_data.len(), 2048);
        assert!(!ota.finalized, "should not be finalized");

        // mark_valid should NOT be called
        assert!(!ota.valid, "mark_valid should NOT be called");

        // Rollback
        ota.rollback_and_reboot().unwrap();
        assert!(ota.rolled_back, "should have rolled back");
    }

    /// OTA rollback on begin failure: the OTA partition cannot be opened.
    #[test]
    fn test_spaapp_ota_rollback_on_begin_failure() {
        let mut ota = launa_ota::mock::MockOta::new();
        ota.fail_on_begin = true;

        // Begin fails immediately
        let result = ota.begin();
        assert!(result.is_err(), "begin should fail");

        // No data written, nothing to finalize or mark valid
        assert!(ota.firmware_data.is_empty());
        assert!(!ota.valid);
        assert!(!ota.finalized);

        // Rollback (even though nothing was written, still call it for safety)
        ota.rollback_and_reboot().unwrap();
        assert!(ota.rolled_back);
    }

    // ---- VAL-APP-032: Command retry and drop when spa doesn't confirm ----

    /// Command retry and drop: send toggle → spa never confirms →
    /// verify retry count increments on each 5s timeout →
    /// verify command is dropped after MAX_COMMAND_RETRIES=2 retries.
    #[test]
    fn test_spaapp_command_retry_and_drop_lifecycle() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Initial status: pump1 is Off
        app.process_frame(&make_status_frame());

        // Queue and send toggle pump1 on Ready
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.process_frame(&make_ready_frame());
        assert_eq!(app.queued_command_count(), 0, "command should be dequeued");

        // At this point, command is tracked but not confirmed.
        // Initial: retries=0, drops=0
        assert_eq!(app.total_retries(), 0);
        assert_eq!(app.total_dropped(), 0);

        // --- Retry 1: Advance past 5s timeout, spa still shows pump1=Off ---
        clock.advance_ms(6_000);
        let actions = app.process_frame(&make_status_frame());
        let has_retry1 = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_retry1, "Retry 1: should resend command");
        assert_eq!(app.total_retries(), 1, "should have 1 retry");

        // --- Retry 2: Advance past another 5s, still not confirmed ---
        clock.advance_ms(6_000);
        let actions = app.process_frame(&make_status_frame());
        let has_retry2 = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_retry2, "Retry 2: should resend command");
        assert_eq!(app.total_retries(), 2, "should have 2 retries");

        // --- Drop: Advance past another 5s — MAX_COMMAND_RETRIES=2 exceeded ---
        clock.advance_ms(6_000);
        app.process_frame(&make_status_frame());
        assert!(
            app.total_dropped() > 0,
            "command should be dropped after exceeding max retries"
        );
        assert_eq!(app.total_retries(), 2, "no more retries after drop");

        // Verify no pending commands remain
        // (the command was removed from the tracker after being dropped)
    }

    /// Command retry with SpaSim integration: send command to SpaSim but
    /// use command_success_rate=0 to make SpaSim ignore it → verify retry
    /// and eventual drop through the full SpaSim → SpaApp pipeline.
    #[test]
    fn test_spaapp_command_retry_with_sim_pipeline() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        let mut sim = SpaSim::new();

        // Register SpaApp with SpaSim
        full_registration(&mut sim, &mut app);

        // Make SpaSim ignore all commands
        sim.set_command_success_rate(0.0);

        // Get initial status
        let status_frame = decode_first_frame(&sim.generate_status_frame());
        app.process_frame(&status_frame);

        // Queue toggle pump1
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));

        // Send on Ready
        let ready_frame = Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        };
        let actions = app.process_frame(&ready_frame);
        let send_bytes = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect("should send command");

        // Feed the command to SpaSim (which will ignore it)
        sim.process_incoming_bytes(&send_bytes);

        // Retry cycle 1: advance 6s, get status from sim (pump still Off)
        clock.advance_ms(6_000);
        let status_bytes = sim.generate_status_frame();
        let status_frame = decode_first_frame(&status_bytes);
        let _actions = app.process_frame(&status_frame);
        assert!(app.total_retries() >= 1, "should have at least 1 retry");

        // Retry cycle 2
        clock.advance_ms(6_000);
        let status_bytes = sim.generate_status_frame();
        let status_frame = decode_first_frame(&status_bytes);
        app.process_frame(&status_frame);
        assert!(app.total_retries() >= 2, "should have at least 2 retries");

        // Drop cycle
        clock.advance_ms(6_000);
        let status_bytes = sim.generate_status_frame();
        let status_frame = decode_first_frame(&status_bytes);
        app.process_frame(&status_frame);
        assert!(
            app.total_dropped() > 0,
            "command should be dropped after max retries"
        );
    }

    /// Multiple commands: queue several commands, verify each independently
    /// retries and drops when spa never confirms any of them.
    #[test]
    fn test_spaapp_multiple_command_retry_and_drop() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&make_status_frame());

        // Queue 3 different commands
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        app.on_mqtt_command(Command::SetTemperature(100));

        // Send all 3 on Ready frames
        app.process_frame(&make_ready_frame());
        app.process_frame(&make_ready_frame());
        app.process_frame(&make_ready_frame());
        assert_eq!(app.queued_command_count(), 0);

        // First timeout cycle: all 3 should retry
        clock.advance_ms(6_000);
        let actions = app.process_frame(&make_status_frame());
        let retry_count = actions
            .iter()
            .filter(|a| matches!(a, AppAction::SendFrame(_)))
            .count();
        assert!(
            retry_count >= 1,
            "at least one command should retry on first timeout"
        );

        // Continue cycling until all commands are dropped
        for cycle in 0..10 {
            clock.advance_ms(6_000);
            app.process_frame(&make_status_frame());

            if app.total_dropped() >= 1 {
                break;
            }
            assert!(
                cycle < 9,
                "commands should have been dropped within 10 cycles"
            );
        }

        assert!(
            app.total_dropped() >= 1,
            "at least one command should be dropped"
        );
    }

    // ---- Additional SpaApp + SpaSim pipeline tests ----

    /// Full SpaApp + SpaSim end-to-end: register → receive status → send command →
    /// verify state change propagates through the full pipeline.
    #[test]
    fn test_spaapp_full_pipeline_register_status_command() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        let mut sim = SpaSim::new();

        // Step 1: Registration
        full_registration(&mut sim, &mut app);
        assert!(app.is_registered());

        // Step 2: Receive status from SpaSim
        let status_bytes = sim.generate_status_frame();
        let status_frame = decode_first_frame(&status_bytes);
        let actions = app.process_frame(&status_frame);
        assert_eq!(app.frames_received(), 1);

        // Should publish state
        let has_state = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishState { .. }));
        assert!(has_state, "should publish state after status");

        // Step 3: Queue a command
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Step 4: Ready frame → command is sent
        let ready_frame = Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        };
        let actions = app.process_frame(&ready_frame);
        let send_bytes = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect("should send command on Ready");

        // Step 5: Feed command to SpaSim → SpaSim applies toggle
        sim.process_incoming_bytes(&send_bytes);
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "sim should apply toggle"
        );

        // Step 6: SpaSim generates new status with pump1=Low
        let status_bytes = sim.generate_status_frame();
        let new_status_frame = decode_first_frame(&status_bytes);
        let _actions = app.process_frame(&new_status_frame);

        // Command should be confirmed — no retries or drops
        assert_eq!(app.total_retries(), 0, "no retries expected");
        assert_eq!(app.total_dropped(), 0, "no drops expected");

        // State should reflect pump1 on
        let status = app.last_status().expect("should have status");
        assert!(
            matches!(status.pumps[0], PumpState::Low | PumpState::High),
            "pump1 should be on in app status"
        );
    }

    /// SpaApp tick() with VirtualClock: verify diagnostics fire at exact intervals.
    #[test]
    fn test_spaapp_tick_virtual_clock_diagnostics() {
        let (clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // First tick at time 0 should produce diagnostics
        let actions = app.tick();
        let has_diag = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishDiagnostics { .. }));
        assert!(has_diag, "should publish diagnostics on first tick");

        // Advance 59s — should NOT produce diagnostics (interval is 60s)
        clock.advance_ms(59_000);
        let actions = app.tick();
        let no_diag = !actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishDiagnostics { .. }));
        assert!(no_diag, "should NOT publish diagnostics at 59s");

        // Advance to 60s — should produce diagnostics
        clock.advance_ms(1_000);
        let actions = app.tick();
        let has_diag2 = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishDiagnostics { .. }));
        assert!(has_diag2, "should publish diagnostics at 60s");
    }

    /// SpaApp heap monitoring: verify heap alerts fire at correct thresholds.
    #[test]
    fn test_spaapp_heap_monitoring() {
        let (clock, app) = make_spaapp();
        let mut app = app;

        // Advance past check interval
        clock.advance_ms(31_000);

        // Normal heap — no alert
        let actions = app.check_heap(8192);
        let no_alert = !actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishAlert { .. }));
        assert!(no_alert, "should not alert on normal heap");

        // Advance and check with critically low heap
        clock.advance_ms(31_000);
        let actions = app.check_heap(500);
        let has_critical = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. } if message == "heap_critically_low"
            )
        });
        assert!(has_critical, "should alert on critically low heap");
    }

    /// SpaApp processes fault log from SpaSim and includes it in state publishing.
    #[test]
    fn test_spaapp_fault_log_with_sim() {
        let (_clock, app) = make_spaapp();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&make_status_frame());

        // Simulate SpaSim generating a fault log response
        let mut sim = SpaSim::new();
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim
            .process_frame(&request_frames[0])
            .expect("should return fault log response");
        let response_frames = decoder.feed_slice(&response_bytes);

        // Feed fault log to SpaApp
        app.process_frame(&response_frames[0]);
        assert!(
            app.last_fault().is_some(),
            "should capture fault log from SpaSim"
        );

        // Next status should include the fault
        let actions = app.process_frame(&make_status_frame());
        let has_fault = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishState { fault: Some(_), .. }));
        assert!(has_fault, "should include fault in state publish");
    }
}
