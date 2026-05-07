//! MQTT publishing and discovery integration tests.
//!
//! Tests for MQTT state serialization, HA discovery validation,
//! full pipeline from simulator status to MQTT JSON, and
//! topic builder / custom device name scenarios.

use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::PumpState;
use launa_protocol::Temperature;
use launa_sim::SpaSim;

#[test]
fn test_status_to_mqtt_json() {
    let mut sim = SpaSim::new();
    let encoded = sim.generate_status_frame();

    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&encoded);
    let msg = dispatch_frame(&frames[0]);

    match msg {
        IncomingMessage::StatusUpdate(status) => {
            let json_str =
                launa_mqtt::state::status_to_json(&status, None, None, false, None, "registered");
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

            assert_eq!(parsed["current_temp"], 38.0);
            assert_eq!(parsed["set_temp"], 40.0);
            // Default SpaState has circ_pump=true, so heating can be active if temp < set_temp
            assert_eq!(parsed["heating_mode"], "ready");
            assert_eq!(parsed["temp_range"], "high");
            assert_eq!(parsed["temp_scale"], "celsius");
        }
        _ => panic!("Expected StatusUpdate"),
    }
}

#[test]
fn test_full_pipeline_status_frame_to_mqtt_json() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = Temperature::celsius(38.0);
    sim.state.set_temp = Temperature::celsius(40.0);
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
            let json_str =
                launa_mqtt::state::status_to_json(&status, None, None, false, None, "registered");
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

            assert_eq!(parsed["current_temp"], 38.0);
            assert_eq!(parsed["set_temp"], 40.0);
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

#[test]
fn test_ha_discovery_full_validation() {
    let builder = launa_mqtt::discovery::DiscoveryBuilder::new("test_spa");
    let configs = builder.build();

    assert_eq!(
        configs.len(),
        32,
        "should produce exactly 32 discovery configs"
    );

    let mut topics_seen = std::collections::HashSet::new();

    for (topic, json_str) in &configs {
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

        assert!(
            topics_seen.insert(topic.clone()),
            "duplicate topic: {}",
            topic
        );

        let v: serde_json::Value = serde_json::from_str(json_str)
            .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));

        assert!(v.get("name").is_some(), "missing name in {}", topic);
        assert!(
            v.get("unique_id").is_some(),
            "missing unique_id in {}",
            topic
        );
        let is_optimistic = v
            .get("optimistic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Button entities are command-only (have payload_press, no state_topic)
        let is_button = v.get("payload_press").is_some();
        // Text entities are command-only (no state_topic)
        let is_text = topic.contains("/text/");
        if !is_optimistic && !is_button && !is_text {
            assert!(
                v.get("state_topic").is_some(),
                "missing state_topic in {}",
                topic
            );
        }
        assert!(
            v.get("availability_topic").is_some(),
            "missing availability_topic in {}",
            topic
        );

        let uid = v["unique_id"].as_str().unwrap();
        assert!(
            uid.starts_with("test_spa_"),
            "unique_id should start with device_id: {}",
            uid
        );

        if !is_optimistic && !is_button && !is_text {
            let st = v["state_topic"].as_str().unwrap();
            let uid = v["unique_id"].as_str().unwrap();
            let is_dedicated_topic = uid.ends_with("_diagnostics")
                || uid.ends_with("_alert")
                || uid.ends_with("_firmware_version");
            if !is_dedicated_topic {
                assert_eq!(
                    st, "launa/test_spa/state",
                    "state_topic should match device state topic for {}",
                    uid
                );
            } else {
                assert!(
                    st.starts_with("launa/test_spa/"),
                    "dedicated state_topic should be under device namespace: {}",
                    st
                );
            }
        }

        let at = v["availability_topic"].as_str().unwrap();
        assert_eq!(at, "launa/test_spa/availability");

        if let Some(ct) = v.get("command_topic").and_then(|t| t.as_str()) {
            assert!(
                ct.starts_with("launa/test_spa/command/"),
                "command_topic should be under device command base: {}",
                ct
            );
        }
    }
}
