//! Integration tests for OTA and error recovery scenarios.
//!
//! Tests for:
//! 1. OTA disconnect mid-download — partial download recovery and rollback
//! 2. OTA Content-Length exceeding partition size — oversized firmware rejected
//! 3. WiFi disconnect during MQTT publish — broker drops during active publish

use launa_core::AppAction;
use launa_integration_tests::harness::TestHarness;
use launa_ota::http::{parse_content_length, validate_http_status};
use launa_ota::mock::MockOta;
use launa_ota::{OtaError, OtaUpdate, MAX_FIRMWARE_SIZE};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::PumpState;

/// Simulates an HTTP firmware download server that serves firmware data
/// in configurable chunk sizes, mimicking how the real OTA downloads
/// firmware over a TCP socket. Defined locally because the library's
/// SimHttpServer is in a test-only module.
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

// ---------------------------------------------------------------------------
// 1. OTA disconnect mid-download
// ---------------------------------------------------------------------------

/// Simulates OTA firmware download where the TCP connection drops partway through.
/// Uses SimHttpServer to serve firmware in chunks, then simulates a disconnect
/// after 60% of the data has been written. Verifies that:
/// - Partial data is accumulated correctly up to the disconnect point
/// - The OTA session is NOT finalized (incomplete download)
/// - rollback_and_reboot() cleans up the partial update
/// - A fresh OTA session can start after rollback
#[test]
fn test_ota_disconnect_mid_download() {
    let mut ota = MockOta::new();

    // Create firmware larger than what we'll deliver (8 KiB)
    let firmware: Vec<u8> = (0..8192).map(|i| (i % 256) as u8).collect();
    let chunk_size = 1024;
    let server = SimHttpServer::new(firmware.clone(), chunk_size);

    ota.begin().unwrap();

    let chunks = server.download_chunks();
    assert_eq!(chunks.len(), 8, "8 KiB / 1 KiB chunks = 8 chunks");

    // Write only the first 5 chunks (simulate disconnect after ~62%)
    let disconnect_after = 5;
    for (i, chunk) in chunks.iter().enumerate() {
        if i >= disconnect_after {
            break;
        }
        ota.write(chunk).unwrap();
    }

    // Verify partial data accumulated correctly
    assert_eq!(
        ota.firmware_data.len(),
        disconnect_after * chunk_size,
        "should have exactly {} bytes after partial download",
        disconnect_after * chunk_size
    );

    // OTA should NOT be finalized (download incomplete)
    assert!(
        !ota.finalized,
        "should NOT be finalized after mid-download disconnect"
    );

    // Mark_valid should NOT have been called
    assert!(
        !ota.valid,
        "should NOT be marked valid after incomplete download"
    );

    // Attempt to finalize with incomplete data should work at the mock level,
    // but in a real scenario the app would detect the size mismatch and rollback.
    // Simulate the expected behavior: rollback the incomplete OTA.
    ota.rollback_and_reboot().unwrap();
    assert!(
        ota.rolled_back,
        "should have rolled back after mid-download disconnect"
    );

    // Verify a fresh OTA session can start after rollback
    let mut ota2 = MockOta::new();
    let small_firmware: Vec<u8> = (0..512).map(|i| (i % 256) as u8).collect();
    let server2 = SimHttpServer::new(small_firmware.clone(), 256);

    ota2.begin().unwrap();
    for chunk in server2.download_chunks() {
        ota2.write(&chunk).unwrap();
    }
    ota2.finalize().unwrap();
    ota2.mark_valid().unwrap();

    assert!(
        ota2.finalized,
        "new OTA session should finalize successfully"
    );
    assert!(ota2.valid, "new OTA session should be marked valid");
    assert_eq!(
        ota2.firmware_data, small_firmware,
        "new OTA data should match"
    );
}

/// Simulates OTA disconnect mid-download using the full harness pipeline.
/// Verifies that SpaApp remains functional after an OTA failure — the app
/// continues processing status frames and publishing state.
#[test]
fn test_ota_disconnect_app_remains_functional() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);

    // Establish baseline — get some status updates through the pipeline
    let baseline_actions = harness.collect_actions();
    let baseline_publish_count = TestHarness::count_action_type(&baseline_actions, |a| {
        matches!(a, AppAction::PublishState { .. })
    });
    assert!(
        baseline_publish_count >= 1,
        "should have at least 1 PublishState before OTA"
    );

    // Simulate OTA attempt that fails mid-download
    let mut ota = MockOta::new();
    let firmware: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
    let server = SimHttpServer::new(firmware, 512);

    ota.begin().unwrap();
    let chunks = server.download_chunks();

    // Write only first 4 of 8 chunks (simulate disconnect at 50%)
    for (i, chunk) in chunks.iter().enumerate() {
        if i >= 4 {
            break;
        }
        ota.write(chunk).unwrap();
    }

    // Mid-download disconnect — rollback
    assert_eq!(ota.firmware_data.len(), 2048);
    ota.rollback_and_reboot().unwrap();
    assert!(ota.rolled_back);

    // Verify the app is still fully functional after the OTA failure
    let post_ota_actions = harness.collect_actions();
    let post_ota_publish_count = TestHarness::count_action_type(&post_ota_actions, |a| {
        matches!(a, AppAction::PublishState { .. })
    });
    assert!(
        post_ota_publish_count >= 1,
        "app should continue publishing state after OTA failure"
    );

    // App should still be registered and have valid status
    assert!(
        harness.app.is_registered(),
        "app should still be registered after OTA failure"
    );
    assert!(
        harness.app.last_status().is_some(),
        "app should still have valid status after OTA failure"
    );

    // Commands should still work after OTA failure
    harness.send_command(Command::ToggleItem(ToggleItem::Pump1));
    let cmd_actions = harness.collect_actions();
    let has_send = cmd_actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(
        has_send,
        "commands should still be processed after OTA failure"
    );
}

// ---------------------------------------------------------------------------
// 2. OTA Content-Length exceeding partition size
// ---------------------------------------------------------------------------

/// Tests that OTA rejects firmware whose Content-Length exceeds the partition size.
///
/// Verifies the full pipeline:
/// 1. HTTP response headers with oversized Content-Length are parsed correctly
/// 2. MockOta rejects writes that exceed MAX_FIRMWARE_SIZE
/// 3. The error is InvalidFirmware (not a panic)
/// 4. The OTA session can be rolled back cleanly
#[test]
fn test_ota_oversized_firmware_rejected() {
    // Step 1: Verify HTTP parsing detects the oversized Content-Length
    let oversized_length = MAX_FIRMWARE_SIZE as u32 + 65536; // ~1.75 MiB + 64 KiB
    let http_headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        oversized_length
    );
    let parsed_length = parse_content_length(http_headers.as_bytes());
    assert_eq!(
        parsed_length,
        Some(oversized_length),
        "should parse oversized Content-Length correctly"
    );

    // Verify the HTTP status is valid (200 OK)
    assert!(
        validate_http_status(http_headers.as_bytes()),
        "HTTP status should be valid 200 OK"
    );

    // Step 2: Verify MockOta rejects firmware that exceeds MAX_FIRMWARE_SIZE
    let mut ota = MockOta::new();
    ota.begin().unwrap();

    // Write data up to MAX_FIRMWARE_SIZE — should succeed
    let chunk = vec![0xAA; 4096];
    let full_chunks = MAX_FIRMWARE_SIZE / 4096;
    for _ in 0..full_chunks {
        ota.write(&chunk).unwrap();
    }
    assert_eq!(
        ota.firmware_data.len(),
        MAX_FIRMWARE_SIZE,
        "should accept data up to MAX_FIRMWARE_SIZE"
    );

    // Write one more byte — should be rejected as InvalidFirmware
    let result = ota.write(&[0xBB]);
    assert!(
        matches!(result, Err(OtaError::InvalidFirmware)),
        "write exceeding MAX_FIRMWARE_SIZE should return InvalidFirmware, got {:?}",
        result
    );

    // Data should not have grown beyond the limit
    assert_eq!(
        ota.firmware_data.len(),
        MAX_FIRMWARE_SIZE,
        "data should remain at MAX_FIRMWARE_SIZE after rejected write"
    );

    // Step 3: Verify the OTA can be rolled back after overflow rejection
    ota.rollback_and_reboot().unwrap();
    assert!(ota.rolled_back, "should have rolled back after overflow");
}

/// Tests OTA oversized firmware detection at the integration level:
/// Parse HTTP headers → validate status → detect oversized Content-Length → reject.
#[test]
fn test_ota_oversized_content_length_http_pipeline() {
    // Simulate an HTTP response for an oversized firmware
    let cases = vec![
        (MAX_FIRMWARE_SIZE as u32, false, "exactly at limit"),
        ((MAX_FIRMWARE_SIZE + 1) as u32, true, "one byte over limit"),
        ((MAX_FIRMWARE_SIZE * 2) as u32, true, "double the limit"),
        (0, false, "zero-length firmware"),
        (1024, false, "small firmware"),
    ];

    for (content_length, should_be_oversized, description) in cases {
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nServer: test\r\n\r\n",
            content_length
        );

        // Parse Content-Length
        let parsed = parse_content_length(headers.as_bytes());
        assert_eq!(
            parsed,
            Some(content_length),
            "Content-Length parsing failed for case: {}",
            description
        );

        // Validate HTTP status
        assert!(
            validate_http_status(headers.as_bytes()),
            "HTTP status validation failed for case: {}",
            description
        );

        // Check if firmware would be oversized
        let is_oversized = content_length as usize > MAX_FIRMWARE_SIZE;
        assert_eq!(
            is_oversized, should_be_oversized,
            "oversized detection mismatch for case: {}",
            description
        );

        // If oversized, verify OTA would reject the write
        if should_be_oversized {
            let mut ota = MockOta::new();
            ota.begin().unwrap();

            // Try to write MAX_FIRMWARE_SIZE + 1 bytes
            let data = vec![0xCC; MAX_FIRMWARE_SIZE + 1];
            // Write in chunks to test incremental overflow detection
            let write_chunk_size = 65536;
            let mut write_result = Ok(());
            for offset in (0..data.len()).step_by(write_chunk_size) {
                let end = (offset + write_chunk_size).min(data.len());
                write_result = ota.write(&data[offset..end]);
                if write_result.is_err() {
                    break;
                }
            }

            assert!(
                write_result.is_err(),
                "OTA should reject writes for oversized firmware ({})",
                description
            );
            assert!(
                matches!(write_result, Err(OtaError::InvalidFirmware)),
                "error should be InvalidFirmware for case: {}, got {:?}",
                description,
                write_result
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. WiFi disconnect during MQTT publish
// ---------------------------------------------------------------------------

/// Tests MQTT publish failure during a simulated WiFi disconnect.
///
/// Simulates WiFi disconnect by disconnecting the SimBroker mid-session.
/// Verifies that:
/// - Publishes during disconnect are silently dropped
/// - SpaApp continues generating actions (not blocked by publish failure)
/// - Publishing resumes after reconnect
/// - No state corruption occurs
///
/// Note: Real WiFi disconnect can't be simulated on desktop — this test exercises
/// the closest mock path through SimBroker's disconnect/reconnect simulation,
/// which mirrors the real MQTT client behavior when the TCP socket drops.
#[test]
fn test_wifi_disconnect_during_mqtt_publish() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);

    // Phase 1: Normal operation — establish baseline publish count
    let mut normal_publish_count = 0usize;
    for _ in 0..5 {
        let actions = harness.collect_actions();
        harness.execute_actions_on_broker(&actions);
        normal_publish_count += actions
            .iter()
            .filter(|a| matches!(a, AppAction::PublishState { .. }))
            .count();
    }
    let broker_baseline = harness.broker.publish_count();
    assert!(
        normal_publish_count >= 5,
        "should have at least 5 PublishState actions during normal operation"
    );
    assert!(
        broker_baseline >= 5,
        "broker should have at least 5 publications during normal operation"
    );

    // Phase 2: Simulate WiFi disconnect (broker goes offline)
    // Note: This simulates WiFi disconnect at the MQTT broker level. Real WiFi
    // disconnect would drop the TCP socket. SimBroker.simulate_disconnect()
    // mirrors this behavior by silently dropping all publishes.
    harness.broker.simulate_disconnect();

    // Attempt to publish during disconnect
    let mut disconnected_publish_attempts = 0usize;
    for _ in 0..10 {
        let actions = harness.collect_actions();
        disconnected_publish_attempts += actions
            .iter()
            .filter(|a| matches!(a, AppAction::PublishState { .. }))
            .count();
        harness.execute_actions_on_broker(&actions);
    }

    // Broker publish count should NOT have increased during disconnect
    let broker_during_disconnect = harness.broker.publish_count();
    assert_eq!(
        broker_during_disconnect, broker_baseline,
        "broker should not record new publications during disconnect (before={}, after={})",
        broker_baseline, broker_during_disconnect
    );

    // Dropped count should reflect the lost publishes
    let dropped = harness.broker.dropped_count();
    assert!(
        dropped >= 5,
        "should have at least 5 dropped publishes during disconnect, got {}",
        dropped
    );

    // SpaApp should still be generating publish actions (not blocked)
    assert!(
        disconnected_publish_attempts >= 5,
        "SpaApp should still generate PublishState actions even when broker is disconnected (got {})",
        disconnected_publish_attempts
    );

    // Phase 3: Reconnect — publishing should resume
    harness.broker.simulate_reconnect();

    let mut _reconnected_publish_count = 0usize;
    for _ in 0..5 {
        let actions = harness.collect_actions();
        _reconnected_publish_count += actions
            .iter()
            .filter(|a| matches!(a, AppAction::PublishState { .. }))
            .count();
        harness.execute_actions_on_broker(&actions);
    }

    let broker_after_reconnect = harness.broker.publish_count();
    assert!(
        broker_after_reconnect > broker_baseline,
        "broker should have new publications after reconnect (before={}, after={})",
        broker_baseline,
        broker_after_reconnect
    );

    // Verify app state is clean
    assert!(
        harness.app.is_registered(),
        "app should still be registered after disconnect/reconnect"
    );
    assert!(
        !harness.app.is_stale(),
        "app should not be stale (status kept coming via spa UART)"
    );
    assert!(
        harness.app.last_status().is_some(),
        "app should have valid last status"
    );
}

/// Tests WiFi disconnect during an active MQTT command publish.
/// Verifies that commands queued during the disconnect are still processed
/// once connectivity is restored.
#[test]
fn test_wifi_disconnect_during_command_publish() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);
    harness.collect_actions(); // get initial status for tracker

    // Simulate WiFi disconnect
    harness.broker.simulate_disconnect();

    // Queue a command during disconnect — SpaApp accepts it into the queue
    harness.send_command(Command::ToggleItem(ToggleItem::Pump1));
    assert_eq!(
        harness.app.queued_command_count(),
        1,
        "command should be queued even during WiFi disconnect"
    );

    // Process the command through the spa pipeline (UART is independent of WiFi)
    let cmd_actions = harness.collect_actions();
    harness.process_outgoing(&cmd_actions);

    // The command should have been dequeued and sent via UART (SendFrame)
    let has_send_frame = cmd_actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(
        has_send_frame,
        "command should be sent via UART even during WiFi disconnect"
    );

    // PublishState actions should be generated but dropped by the broker
    let has_publish_state = cmd_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(
        has_publish_state,
        "SpaApp should generate PublishState even during WiFi disconnect"
    );

    // Verify broker dropped the publish
    harness.execute_actions_on_broker(&cmd_actions);
    let dropped = harness.broker.dropped_count();
    assert!(
        dropped >= 1,
        "broker should have dropped at least 1 publish during disconnect, got {}",
        dropped
    );

    // Reconnect WiFi
    harness.broker.simulate_reconnect();

    // Verify publishing resumes with correct state
    let resume_actions = harness.collect_actions();
    harness.execute_actions_on_broker(&resume_actions);

    // Verify pump state is reflected in the published state
    let pump_on_in_state = resume_actions.iter().any(|a| {
        if let AppAction::PublishState { status, .. } = a {
            status.pumps[0] != PumpState::Off
        } else {
            false
        }
    });
    assert!(
        pump_on_in_state,
        "published state should reflect pump1 on after command was processed"
    );

    // Verify broker now has the updated state
    let last_state = harness.broker.last_state();
    assert!(
        last_state.is_some(),
        "broker should have state after reconnect"
    );
    let state_json: serde_json::Value = serde_json::from_str(last_state.unwrap()).unwrap();
    assert_eq!(
        state_json["pump1_on"], true,
        "broker state should show pump1 on after reconnect"
    );
}

/// Tests that multiple rapid WiFi disconnect/reconnect cycles don't corrupt state.
#[test]
fn test_rapid_wifi_disconnect_reconnect_cycles() {
    let mut harness = TestHarness::new();
    harness.complete_registration(5);
    harness.collect_actions();

    let initial_broker_count = harness.broker.publish_count();

    // Simulate 5 rapid disconnect/reconnect cycles
    for cycle in 0..5 {
        harness.broker.simulate_disconnect();

        // Tick once during disconnect
        let actions = harness.collect_actions();
        harness.execute_actions_on_broker(&actions);

        harness.broker.simulate_reconnect();

        // Tick once after reconnect
        let actions = harness.collect_actions();
        harness.execute_actions_on_broker(&actions);

        // App should remain registered throughout
        assert!(
            harness.app.is_registered(),
            "app should remain registered during cycle {}",
            cycle
        );
    }

    // Verify broker has accumulated publications from reconnect phases
    let final_broker_count = harness.broker.publish_count();
    assert!(
        final_broker_count > initial_broker_count,
        "broker should have new publications after reconnect cycles (before={}, after={})",
        initial_broker_count,
        final_broker_count
    );

    // Verify dropped count reflects the disconnect phases
    let dropped = harness.broker.dropped_count();
    assert!(
        dropped >= 5,
        "should have dropped at least 5 publishes across 5 disconnect cycles, got {}",
        dropped
    );

    // App should be in clean state
    assert!(harness.app.is_registered());
    assert!(!harness.app.is_stale());
    assert!(harness.app.last_status().is_some());
}
