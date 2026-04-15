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
        let sim = SpaSim::new();
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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let client_request = FrameEncoder::encode([0xFE, 0xBF], &[0x01]);
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
                let ack = FrameEncoder::encode([id, 0xBF], &[0x03]);
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
        let sim = SpaSim::new();
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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);

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

        ota.write(&[0x01, 0x02, 0x03]).unwrap();
        assert_eq!(ota.firmware_data.len(), 3);

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

        assert_eq!(configs.len(), 20);

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
        let mut encoded = frame.encode();

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
        let encoded = frame.encode();

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
        let sim = SpaSim::new();
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
        let sim = SpaSim::new();

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
        let encoded = frame.encode();

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
            let encoded = FrameEncoder::encode(mt, &payload);
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&encoded);
            sim.process_frame(&frames[0]);
        }

        assert_eq!(sim.state.pumps[0], PumpState::Low);
        assert_eq!(sim.state.pumps[1], PumpState::Low);
        assert_eq!(sim.state.pumps[2], PumpState::Low);

        let (mt, payload) = Command::ToggleItem(ToggleItem::Blower).encode();
        let encoded = FrameEncoder::encode(mt, &payload);
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        sim.process_frame(&frames[0]);
        assert!(sim.state.blower);

        let (mt, payload) = Command::ToggleItem(ToggleItem::HeatingMode).encode();
        let encoded = FrameEncoder::encode(mt, &payload);
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&encoded);
        sim.process_frame(&frames[0]);
        assert_eq!(sim.state.heating_mode, HeatingMode::Rest);

        let (mt, payload) = Command::ToggleItem(ToggleItem::TemperatureRange).encode();
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);

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
            20,
            "should produce exactly 20 discovery configs"
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
                    return Err(OtaError::WriteFailed);
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

    #[test]
    fn test_ota_empty_firmware() {
        let mut ota = launa_ota::mock::MockOta::new();

        // Edge case: zero-length firmware
        let firmware: Vec<u8> = Vec::new();
        let server = SimHttpServer::new(firmware.clone(), 1024);

        ota.begin().unwrap();
        // No chunks to write — download_chunks returns empty vec
        let chunks = server.download_chunks();
        assert!(chunks.is_empty(), "empty firmware should yield no chunks");
        for chunk in &chunks {
            ota.write(chunk).unwrap();
        }
        ota.finalize().unwrap();
        ota.mark_valid().unwrap();

        assert!(ota.finalized);
        assert!(ota.valid);
        assert!(ota.firmware_data.is_empty());
    }
}
