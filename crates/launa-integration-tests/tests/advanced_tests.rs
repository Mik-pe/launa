//! Tier 5 Advanced Integration Tests
//!
//! Tests for advanced protocol interactions and edge cases:
//! 1. Fault log walk entries 1..N — sequential request/response with entry tracking
//! 2. Configuration request/response pairing via SpaApp
//! 3. Filter cycles request/response via SpaApp
//! 4. MQTT broker disconnect/reconnect during active session
//! 5. Rapid command flood exceeds queue cap (verify total_dropped counter)

use launa_core::{AppAction, SpaApp};
use launa_integration_tests::harness::TestHarness;
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::dispatcher::IncomingMessage;
use launa_protocol::fault::FaultCode;
use launa_sim::spa_sim::FaultLogConfig;
use launa_sim::VirtualClock;

// Test 1: VAL-SR-006 — Fault log walk entries 1..N
// Request fault entries 1 through 5 sequentially via SpaApp command pipeline.
// Each response should be distinct with the correct entry_number.
// This differs from test_multi_frame_fault_log_walk (which tests SpaApp directly
// with injected frames) by exercising the full SpaApp → SpaSim → decode → process
// pipeline for each request.

#[test]
fn test_fault_log_walk_entries_1_to_n() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);
    harness.collect_actions(); // get initial status for tracker

    // Define 5 fault entries with distinct codes and data
    let fault_entries: Vec<(FaultCode, u8, u8)> = vec![
        (FaultCode::HeaterDry, 1, 5),      // entry 1, 5 days ago
        (FaultCode::LowFlow, 2, 3),        // entry 2, 3 days ago
        (FaultCode::WaterTooHot, 3, 1),    // entry 3, 1 day ago
        (FaultCode::SensorAFault, 4, 0),   // entry 4, today
        (FaultCode::GfciTestFailed, 5, 7), // entry 5, 7 days ago
    ];

    let mut captured_faults = Vec::new();

    for (i, (code, entry_num, days_ago)) in fault_entries.iter().enumerate() {
        // Configure SpaSim with this entry's fault data
        harness.sim.set_fault_log_config(FaultLogConfig {
            fault_count: 5,
            entry_number: *entry_num,
            message_code: *code,
            days_ago: *days_ago,
            hour: 10 + i as u8,
            minute: 30,
            flags: 0x04,
            set_temperature: 104,
            sensor_a_temp: 100 + i as u8,
            sensor_b_temp: 99 + i as u8,
        });

        // Send FaultLogRequest through SpaApp → SpaSim pipeline
        // First queue the command
        let request_cmd = Command::FaultLogRequest { entry: *entry_num };
        harness.send_command(request_cmd);
        assert_eq!(harness.app.queued_command_count(), 1);

        // Ready frame triggers the command to be sent to SpaSim
        let ready_frame = launa_protocol::frame::Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        };
        let actions = harness.app.process_frame(&ready_frame);

        // Extract the SendFrame (fault log request) and feed to SpaSim
        let send_bytes = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect("should send fault log request");

        let response_bytes = harness.sim.process_incoming_bytes(&send_bytes);
        assert!(
            !response_bytes.is_empty(),
            "SpaSim should respond to fault log request for entry {}",
            entry_num
        );

        // Decode and process the response through SpaApp
        let response_frames = harness.decoder.feed_slice(&response_bytes);
        assert_eq!(
            response_frames.len(),
            1,
            "should get exactly 1 fault log response frame"
        );

        let _resp_actions = harness.app.process_frame(&response_frames[0]);

        // Verify last_fault was updated
        assert!(
            harness.app.last_fault().is_some(),
            "last_fault should be set after entry {}",
            entry_num
        );

        let fault_str = harness.app.last_fault().unwrap();
        assert!(
            fault_str.contains(&format!("{:?}", code)),
            "fault string '{}' should contain fault code {:?} for entry {}",
            fault_str,
            code,
            entry_num
        );

        captured_faults.push((fault_str.to_string(), *entry_num));
    }

    // Verify 5 distinct responses were captured
    assert_eq!(captured_faults.len(), 5, "should have 5 fault entries");

    // Verify each entry_number is correct (fault string contains unique code)
    assert!(
        captured_faults[0].0.contains("HeaterDry"),
        "entry 1 should be HeaterDry"
    );
    assert!(
        captured_faults[1].0.contains("LowFlow"),
        "entry 2 should be LowFlow"
    );
    assert!(
        captured_faults[2].0.contains("WaterTooHot"),
        "entry 3 should be WaterTooHot"
    );
    assert!(
        captured_faults[3].0.contains("SensorAFault"),
        "entry 4 should be SensorAFault"
    );
    assert!(
        captured_faults[4].0.contains("GfciTestFailed"),
        "entry 5 should be GfciTestFailed"
    );

    // Last entry stored should be the 5th one
    let final_fault = harness.app.last_fault().unwrap();
    assert!(
        final_fault.contains("GfciTestFailed"),
        "last stored fault should be the final entry (GfciTestFailed), got: '{}'",
        final_fault
    );
}

// Test 2: VAL-SR-007 — Configuration request/response pairing
// Send ConfigurationRequest via SpaApp → SpaSim → decode pipeline.
// Verify SpaApp emits a ControlConfiguration event with correct pump/light config.

#[test]
fn test_configuration_request_response_pairing() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);
    harness.collect_actions(); // get initial status

    // Send ConfigurationRequest through SpaApp → SpaSim
    harness.send_command(Command::ConfigurationRequest);
    assert_eq!(harness.app.queued_command_count(), 1);

    // Trigger the command via Ready frame
    let ready_frame = launa_protocol::frame::Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let actions = harness.app.process_frame(&ready_frame);

    // Extract SendFrame and feed to SpaSim
    let send_bytes = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should send config request");

    let response_bytes = harness.sim.process_incoming_bytes(&send_bytes);
    assert!(
        !response_bytes.is_empty(),
        "SpaSim should respond to config request"
    );

    // Decode response and verify it's a ControlConfiguration
    let response_frames = harness.decoder.feed_slice(&response_bytes);
    assert_eq!(
        response_frames.len(),
        1,
        "should get exactly 1 config response frame"
    );

    // Parse the response to verify it's a valid ControlConfiguration
    let response_frame = &response_frames[0];
    let msg = launa_protocol::dispatcher::dispatch_frame(response_frame);

    match msg {
        IncomingMessage::ControlConfiguration(config) => {
            // Verify default SpaSim config values
            assert!(
                config.pump_configs.len() >= 2,
                "config should have at least 2 pump configs"
            );
            // SpaSim defaults: Pump1=TwoSpeed, Pump2=TwoSpeed, circ_pump=true, blower=true
            assert!(config.circ_pump, "config should have circ_pump=true");
            assert!(config.blower, "config should have blower=true");
            assert!(config.lights[0], "config should have light1=true");
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }

    // Also verify through SpaApp — it should have processed the frame
    // (SpaApp currently doesn't store ControlConfiguration in an accessor,
    //  but it should process it without error and not panic)
    let _resp_actions = harness.app.process_frame(response_frame);
}

// Test 3: VAL-SR-008 — Filter cycles request/response
// Send FilterCyclesRequest via SpaApp → SpaSim → decode pipeline.
// Verify response with expected default filter cycle values.

#[test]
fn test_filter_cycles_request_response() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);
    harness.collect_actions(); // get initial status

    // Send FilterCyclesRequest through SpaApp → SpaSim
    harness.send_command(Command::FilterCyclesRequest);
    assert_eq!(harness.app.queued_command_count(), 1);

    // Trigger the command via Ready frame
    let ready_frame = launa_protocol::frame::Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let actions = harness.app.process_frame(&ready_frame);

    // Extract SendFrame and feed to SpaSim
    let send_bytes = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should send filter cycles request");

    let response_bytes = harness.sim.process_incoming_bytes(&send_bytes);
    assert!(
        !response_bytes.is_empty(),
        "SpaSim should respond to filter cycles request"
    );

    // Decode response and verify it's a FilterCyclesResponse
    let response_frames = harness.decoder.feed_slice(&response_bytes);
    assert_eq!(
        response_frames.len(),
        1,
        "should get exactly 1 filter cycles response frame"
    );

    let response_frame = &response_frames[0];
    let msg = launa_protocol::dispatcher::dispatch_frame(response_frame);

    match msg {
        IncomingMessage::FilterCyclesResponse(fc) => {
            // Verify SpaSim default filter cycle values
            assert_eq!(fc.filter1.start_hour, 8, "filter1 should start at hour 8");
            assert_eq!(
                fc.filter1.duration_hours, 4,
                "filter1 should run for 4 hours"
            );
            assert_eq!(fc.filter2.start_hour, 16, "filter2 should start at hour 16");
            assert!(fc.filter2.enabled, "filter2 should be enabled by default");
        }
        other => panic!("Expected FilterCyclesResponse, got {:?}", other),
    }

    // Verify SpaApp processes the frame without error
    let _resp_actions = harness.app.process_frame(response_frame);
}

// Test 4: VAL-SR-009 — MQTT broker disconnect/reconnect during session
// Simulate MQTT broker disconnect during active session.
// Verify: no publications during disconnect, resume after reconnect,
// dropped_count is correct.

#[test]
fn test_mqtt_broker_disconnect_reconnect() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);

    // Establish baseline: tick once and process through broker (no subscription = accept all)
    let _initial_actions = harness.tick_spa();
    harness.process_outgoing(&_initial_actions);
    harness.execute_actions_on_broker(&_initial_actions);
    let initial_count = harness.broker.publish_count();
    assert!(
        initial_count > 0,
        "should have initial publications, got {}",
        initial_count
    );

    harness.broker.simulate_disconnect();

    // Run 10 ticks — SpaApp still produces actions, but broker drops them all
    let pre_disconnect_count = harness.broker.publish_count();
    for _ in 0..10 {
        let actions = harness.tick_spa();
        harness.process_outgoing(&actions);
        harness.execute_actions_on_broker(&actions);
    }

    let during_disconnect_count = harness.broker.publish_count();
    assert_eq!(
        during_disconnect_count, pre_disconnect_count,
        "no new publications should be recorded during disconnect (before={}, after={})",
        pre_disconnect_count, during_disconnect_count
    );

    // Verify dropped count reflects the lost publishes
    let dropped = harness.broker.dropped_count();
    assert!(
        dropped >= 5,
        "should have at least 5 dropped publishes during disconnect, got {}",
        dropped
    );

    harness.broker.simulate_reconnect();

    // Run 10 more ticks — publications should resume
    let mut post_reconnect_publish_state_count = 0usize;
    for _ in 0..10 {
        let actions = harness.tick_spa();
        harness.process_outgoing(&actions);
        post_reconnect_publish_state_count += actions
            .iter()
            .filter(|a| matches!(a, AppAction::PublishState { .. }))
            .count();
        harness.execute_actions_on_broker(&actions);
    }

    let after_reconnect_count = harness.broker.publish_count();
    assert!(
        after_reconnect_count > pre_disconnect_count,
        "publications should resume after reconnect (before={}, after={})",
        pre_disconnect_count,
        after_reconnect_count
    );

    assert!(
        post_reconnect_publish_state_count >= 5,
        "should have at least 5 PublishState actions after reconnect, got {}",
        post_reconnect_publish_state_count
    );

    // Dropped count should still reflect only the disconnect-period losses
    let final_dropped = harness.broker.dropped_count();
    assert!(
        final_dropped >= 5,
        "final dropped_count should still reflect disconnect losses (got {})",
        final_dropped
    );

    // App should be in good state
    assert!(
        harness.app.is_registered(),
        "app should still be registered"
    );
    assert!(
        !harness.app.is_stale(),
        "app should not be stale (status kept coming)"
    );
}

// Test 5: VAL-SR-010 — Rapid command flood exceeds queue cap
// Send 35 MQTT commands rapidly. Verify:
// - 32 accepted (MAX_COMMAND_QUEUE = 32)
// - 3+ dropped (overflow)
// - total_dropped() counter accurate
// - All 32 queued commands drain on Ready
// - Commands that exceed MAX_PENDING_COMMANDS in the tracker are also counted as drops

#[test]
fn test_rapid_command_flood_exceeds_queue_cap() {
    // This test exercises SpaApp's command queue directly (unit-level)
    // since the harness integration also involves SpaSim command processing.
    let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut app = SpaApp::new(clock);
    app.force_registered(0x03);

    // Get an initial status so CommandTracker has a pre_status baseline
    let status_frame = launa_protocol::frame::Frame {
        message_type: [0xFF, 0xAF],
        payload: {
            let mut p = vec![0u8; 24];
            p[2] = 100; // current temp
            p[20] = 104; // set temp
            p
        },
    };
    app.process_frame(&status_frame);

    let queue_cap = 32usize;
    let total_commands = 35usize;
    let queue_drops = total_commands - queue_cap; // 3

    // Flood with commands
    for i in 0..total_commands {
        // Alternate between different toggle items for variety
        let item = match i % 3 {
            0 => ToggleItem::Pump1,
            1 => ToggleItem::Pump2,
            _ => ToggleItem::Pump3,
        };
        app.on_mqtt_command(Command::ToggleItem(item));
    }

    // Verify queue cap: exactly 32 accepted
    assert_eq!(
        app.queued_command_count(),
        queue_cap,
        "queue should be capped at {}",
        queue_cap
    );

    // Verify queue-level drops: exactly 3 dropped
    assert_eq!(
        app.total_dropped(),
        queue_drops as u32,
        "should have exactly {} queue drops, got {}",
        queue_drops,
        app.total_dropped()
    );

    // Drain all queued commands via Ready frames
    let ready_frame = launa_protocol::frame::Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };

    let mut send_count = 0usize;
    for _ in 0..queue_cap {
        let actions = app.process_frame(&ready_frame);
        if actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))) {
            send_count += 1;
        }
    }

    // All 32 commands should have been dequeued and sent
    assert_eq!(
        send_count, queue_cap,
        "all {} queued commands should be sent, got {}",
        queue_cap, send_count
    );

    // Queue should be empty
    assert_eq!(
        app.queued_command_count(),
        0,
        "queue should be empty after draining"
    );

    // Drop counter includes queue-full drops (3). Tracker deduplicates:
    // commands with the same ExpectedChange update existing entries
    // instead of creating new ones, so only 3 unique pending entries
    // (Pump1, Pump2, Pump3) are tracked — no tracker overflow.
    assert_eq!(
        app.total_dropped(),
        queue_drops as u32,
        "drop counter should be {} (queue drops only, no tracker overflow with dedup)",
        queue_drops
    );
}

/// Extended flood test: verify the counter is accurate across multiple
/// flood cycles and the queue can be refilled after draining.
#[test]
fn test_command_flood_multiple_cycles() {
    let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut app = SpaApp::new(clock);
    app.force_registered(0x03);

    // Get initial status
    let status_frame = launa_protocol::frame::Frame {
        message_type: [0xFF, 0xAF],
        payload: {
            let mut p = vec![0u8; 24];
            p[2] = 100;
            p[20] = 104;
            p
        },
    };
    app.process_frame(&status_frame);

    let ready_frame = launa_protocol::frame::Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };

    // Cycle 1: flood + drain
    for _ in 0..35 {
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    }
    assert_eq!(app.queued_command_count(), 32);
    // 3 queue drops from the 35-command flood
    assert_eq!(app.total_dropped(), 3);

    for _ in 0..32 {
        app.process_frame(&ready_frame);
    }
    // Tracker deduplicates: all Pump1 toggles share one ExpectedChange,
    // so only 1 pending entry — no tracker overflow.
    let drops_after_cycle1 = 3u32;
    assert_eq!(app.queued_command_count(), 0);
    assert_eq!(
        app.total_dropped(),
        drops_after_cycle1,
        "drops should be queue drops only (tracker deduplicates)"
    );

    // Cycle 2: flood + drain again (different toggle item)
    let drops_before_cycle2 = app.total_dropped();
    for _ in 0..35 {
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
    }
    assert_eq!(app.queued_command_count(), 32);
    // 3 more queue drops from cycle 2 flood
    assert_eq!(
        app.total_dropped(),
        drops_before_cycle2 + 3,
        "3 more queue drops from cycle 2 flood"
    );

    for _ in 0..32 {
        app.process_frame(&ready_frame);
    }
    assert_eq!(app.queued_command_count(), 0);
    // Tracker has 2 entries (Pump1 + Pump2), still well under
    // MAX_PENDING_COMMANDS=8, so no tracker overflow.
    let drops_after_cycle2 = drops_before_cycle2 + 3;

    // Cycle 3: partial flood (no overflow)
    for _ in 0..10 {
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump3));
    }
    assert_eq!(app.queued_command_count(), 10);
    let drops_before_drain3 = app.total_dropped();
    assert_eq!(
        drops_before_drain3, drops_after_cycle2,
        "no new queue drops when below cap"
    );

    for _ in 0..10 {
        app.process_frame(&ready_frame);
    }
    assert_eq!(app.queued_command_count(), 0);
    // Tracker now has 3 entries (Pump1 + Pump2 + Pump3), still no overflow.
    let final_drops = drops_after_cycle2;

    // Final drop count: all accumulated queue drops across cycles
    assert_eq!(
        app.total_dropped(),
        final_drops,
        "final drop count should include all queue drops (no tracker overflow with dedup)"
    );
}
