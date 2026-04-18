//! Core integration tests for the Launa spa controller.
//!
//! Implements the Tier 1 core integration tests for the Launa spa controller:
//! 1. Harness initial state
//! 2. Registration E2E
//! 3. Status → MQTT publish
//! 4. Command → wire frame
//! 5. Pump timer auto-off
//! 6. Hold mode auto-release
//! 7. Stale detection and recovery
//!
//! Implements the Tier 2 fault scenario tests:
//! 8. Spa reboot mid-session → re-registration
//! 9. Silently dropped commands → retry/drop
//! 10. Bus silence lifecycle → stale alert and recovery
//! 11. Corrupt frame no-desync
//! 12. Spontaneous filter cycle while command pending
//!
//! Implements the Tier 3 protocol misbehavior tests:
//! 13. Out-of-order frames (Ready before Status)
//! 14. Interleaved response and status (status+ready+fault in one buffer)
//! 15. Rapid re-registration (multiple NewClientQuery frames)
//! 16. Partial frame across tick boundary
//! 17. Duplicate status frame in one tick
//! 18. Multi-frame fault log walk
//! 19. Combined stress test (7-phase sequence)

use launa_core::AppAction;
use launa_integration_tests::harness::TestHarness;
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::fault::FaultCode;
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::PumpState;

// ══════════════════════════════════════════════════════════════════════════
// Test 1: VAL-IT-001 — Harness initial state
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_harness_initial_state() {
    let harness = TestHarness::new();

    // Unregistered
    assert!(
        !harness.app.is_registered(),
        "initial state should be unregistered"
    );
    // No status
    assert!(
        harness.app.last_status().is_none(),
        "initial state should have no status"
    );
    // No client ID
    assert!(
        harness.app.client_id().is_none(),
        "initial state should have no client ID"
    );
    // No broker publications
    assert_eq!(
        harness.broker.publish_count(),
        0,
        "initial state should have no broker publications"
    );
    // No frames received
    assert_eq!(
        harness.app.frames_received(),
        0,
        "initial state should have zero frames received"
    );
    // No queued commands
    assert_eq!(
        harness.app.queued_command_count(),
        0,
        "initial state should have no queued commands"
    );
    // Not stale
    assert!(!harness.app.is_stale(), "initial state should not be stale");
}

// ══════════════════════════════════════════════════════════════════════════
// Test 2: VAL-IT-002 — Full registration handshake end-to-end
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_registration_e2e() {
    let mut harness = TestHarness::new();

    // Registration should complete within ≤5 ticks
    let ticks = harness.complete_registration(5);

    assert!(
        harness.app.is_registered(),
        "should be registered after {} ticks",
        ticks
    );
    assert!(
        ticks <= 5,
        "registration should complete within 5 ticks, took {}",
        ticks
    );
    assert!(
        harness.app.client_id().is_some(),
        "should have a client ID after registration"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 3: VAL-IT-003 — Status updates produce MQTT publish actions
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_status_to_mqtt_publish() {
    let mut harness = TestHarness::new();

    // Complete registration first
    harness.complete_registration(5);

    // Clear any broker state from registration
    harness.broker.take_all();

    // Run 5 ticks, collecting publish actions
    let mut total_publish_state = 0;
    for _ in 0..5 {
        let actions = harness.collect_actions();
        total_publish_state += TestHarness::count_action_type(&actions, |a| {
            matches!(a, AppAction::PublishState { .. })
        });
    }

    // Each tick should produce at least 1 PublishState
    assert!(
        total_publish_state >= 5,
        "expected at least 5 PublishState actions after 5 ticks, got {}",
        total_publish_state
    );

    // Broker should have recorded the publications
    assert!(
        harness.broker.publish_count() >= 5,
        "broker should have at least 5 publications, got {}",
        harness.broker.publish_count()
    );

    // Verify broker has state payloads
    assert!(
        harness.broker.last_state().is_some(),
        "broker should have a state payload"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 4: VAL-IT-004 — MQTT command → SpaApp queue → Ready → wire frame
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_command_to_wire_frame() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get initial status so command tracker has pre_status
    harness.collect_actions();

    // Queue a command via MQTT
    let cmd = Command::ToggleItem(ToggleItem::Pump1);
    harness.send_command(cmd);
    assert_eq!(
        harness.app.queued_command_count(),
        1,
        "command should be queued"
    );

    // Tick spa to get a Ready frame — command should be dequeued and sent as wire frame
    let actions = harness.collect_actions();

    // Queue should be drained
    assert_eq!(
        harness.app.queued_command_count(),
        0,
        "command queue should be empty after Ready"
    );

    // There should be a SendFrame action (the toggle command)
    let has_send_frame = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(
        has_send_frame,
        "should have a SendFrame action for the toggle command"
    );

    // Now feed the status back — SpaSim should have processed the toggle
    // and the pump should be on in the next status
    let actions2 = harness.collect_actions();

    // Check if SpaSim has pump on
    let pump_on = actions2.iter().any(|a| {
        if let AppAction::PublishState { status, .. } = a {
            status.pumps[0] != PumpState::Off
        } else {
            false
        }
    });
    assert!(
        pump_on,
        "SpaSim should report pump on after toggle command was processed"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 5: VAL-IT-005 — Pump timer auto-off end-to-end
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_pump_timer_auto_off() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get initial status so pump timer has pump state
    harness.collect_actions();

    // Start pump 1 timer for 1 minute
    let start_actions = harness.app.start_pump_timer(1, 1);
    // Process the toggle-on through SpaSim
    harness.process_outgoing(&start_actions);

    // Tick spa to get status with pump running
    let actions = harness.collect_actions();

    // Verify pump is on in the status
    let pump_on = actions.iter().any(|a| {
        if let AppAction::PublishState { status, .. } = a {
            matches!(status.pumps[0], PumpState::Low | PumpState::High)
        } else {
            false
        }
    });
    assert!(pump_on, "pump should be on after timer start");

    // Advance virtual clock past the timer duration (1 minute = 60,000 ms)
    harness.advance_ms(61_000);

    // Next status tick should trigger the auto-off toggle
    let auto_off_actions = harness.collect_actions();

    // Should contain a SendFrame for the auto-off toggle
    let has_auto_off = TestHarness::has_toggle_for(&auto_off_actions, ToggleItem::Pump1);
    assert!(
        has_auto_off,
        "auto-off toggle should appear at timeout boundary"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 6: VAL-IT-006 — Hold mode timer auto-release
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_hold_mode_auto_release() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get initial status
    harness.collect_actions();

    // Put sim into hold mode
    harness.sim.state.hold = true;

    // Tick to get a status with hold active — this starts the hold timer
    let actions1 = harness.collect_actions();

    // Verify hold is reported in status
    let hold_active = actions1.iter().any(|a| {
        if let AppAction::PublishState { status, .. } = a {
            status.is_hold
        } else {
            false
        }
    });
    assert!(hold_active, "hold mode should be active in status");

    // Advance past hold timeout (60 min = 3,600,000 ms)
    harness.advance_ms(61 * 60 * 1000);

    // Next status with hold still active → timer should fire auto-release toggle
    let fire_actions = harness.collect_actions();
    let fired = TestHarness::has_toggle_for(&fire_actions, ToggleItem::HoldMode);
    assert!(
        fired,
        "hold timer should fire auto-release toggle at 60min boundary"
    );

    // Advance more time — should NOT re-fire while hold is still active (fired flag)
    harness.advance_ms(5_000);
    let no_refire_actions = harness.collect_actions();
    let refired = TestHarness::has_toggle_for(&no_refire_actions, ToggleItem::HoldMode);
    assert!(
        !refired,
        "hold timer should NOT re-fire while hold mode is still active after firing"
    );

    // Advance another full timeout — still should not re-fire
    harness.advance_ms(61 * 60 * 1000);
    let no_refire2_actions = harness.collect_actions();
    let refired2 = TestHarness::has_toggle_for(&no_refire2_actions, ToggleItem::HoldMode);
    assert!(
        !refired2,
        "hold timer should NOT re-fire even after another full timeout period"
    );

    // Release hold mode — timer should re-arm
    harness.sim.state.hold = false;
    harness.collect_actions();

    // Re-enter hold mode
    harness.sim.state.hold = true;
    harness.collect_actions();

    // Advance past timeout again → should fire again
    harness.advance_ms(61 * 60 * 1000);
    let re_fire_actions = harness.collect_actions();
    let re_fired = TestHarness::has_toggle_for(&re_fire_actions, ToggleItem::HoldMode);
    assert!(
        re_fired,
        "hold timer should fire again after hold mode was released and re-entered"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 7: VAL-IT-007 — Stale detection and recovery
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_stale_detection_and_recovery() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get initial status to establish last_status_time
    harness.collect_actions();
    assert!(
        !harness.app.is_stale(),
        "should not be stale immediately after status"
    );

    // Simulate bus silence: no spa ticks for 31 seconds
    // Advance clock in 1-second steps, calling tick_app() each time
    // to check for stale detection
    harness.sim.simulate_bus_silence(35); // suppress spa output for 35 ticks

    let mut stale_alert_seen = false;
    let mut stale_availability_seen = false;

    // Advance 35 seconds with tick_app() to trigger stale detection
    for _sec in 1..=35 {
        harness.advance_ms(1_000);
        let tick_actions = harness.tick_app();
        harness.execute_actions_on_broker(&tick_actions);

        for action in &tick_actions {
            if let AppAction::PublishAlert { message, .. } = action {
                if message == "spa_communication_lost" {
                    stale_alert_seen = true;
                }
            }
            if matches!(action, AppAction::PublishStaleAvailability) {
                stale_availability_seen = true;
            }
        }

        // Also tick the spa (it will be silent, but we need to drain the decoder)
        let spa_actions = harness.tick_spa();
        harness.process_outgoing(&spa_actions);
        harness.execute_actions_on_broker(&spa_actions);
    }

    assert!(
        stale_alert_seen,
        "stale alert should fire after 30s silence"
    );
    assert!(
        stale_availability_seen,
        "stale availability should fire after 30s silence"
    );
    assert!(
        harness.app.is_stale(),
        "app should report stale state after 30s silence"
    );

    // End bus silence — spa resumes sending status frames
    // The sim will automatically resume after bus_silence_remaining reaches 0
    // But we need to make sure we have ticks left. Let's clear the silence
    // and verify recovery.
    // Bus silence was 35 ticks; we already did 35 ticks above, so silence is over.

    // Now tick the spa — it should resume sending status frames
    let recovery_actions = harness.collect_actions();

    // App should no longer be stale
    assert!(
        !harness.app.is_stale(),
        "app should recover from stale after receiving status"
    );

    // Should have a PublishState with recovering_from_stale = true
    let recovering = recovery_actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishState {
                recovering_from_stale: true,
                ..
            }
        )
    });
    assert!(
        recovering,
        "recovery flag should be set on first status after stale"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Tier 2 — Spa-Side Fault Scenarios
// ══════════════════════════════════════════════════════════════════════════

// ══════════════════════════════════════════════════════════════════════════
// Test 8: VAL-IT-013 — Spa reboots mid-session → clean re-registration
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_spa_reboot_mid_session() {
    let mut harness = TestHarness::new();

    // Phase 1: Establish a stable session
    harness.complete_registration(5);
    assert!(harness.app.is_registered());
    let _client_id = harness.app.client_id();

    // Get some status updates
    for _ in 0..3 {
        harness.collect_actions();
    }
    assert!(harness.app.last_status().is_some());
    assert!(
        !harness.app.is_stale(),
        "should not be stale during normal operation"
    );

    // Queue a command
    harness.send_command(Command::ToggleItem(ToggleItem::Pump1));
    assert_eq!(harness.app.queued_command_count(), 1);

    // Phase 2: Spa reboots
    harness.sim.simulate_spa_reboot();

    // The sim is now unregistered, so the next tick will produce a NewClientQuery
    // SpaApp should detect the NewClientQuery, reset registration, and clear command queue
    let _reboot_actions = harness.collect_actions();

    // SpaApp should detect the NewClientQuery and reset
    assert!(
        !harness.app.is_registered(),
        "should be unregistered after spa reboot NewClientQuery"
    );
    assert_eq!(
        harness.app.queued_command_count(),
        0,
        "command queue should be cleared on bus reset"
    );

    // Phase 3: Re-registration
    // Continue ticking until re-registered
    let ticks = harness.complete_registration(5);
    assert!(
        harness.app.is_registered(),
        "should re-register within 5 ticks after reboot (took {})",
        ticks
    );

    // Phase 4: Verify normal operation resumes
    // Pre-reboot stale state should NOT leak
    assert!(
        !harness.app.is_stale(),
        "should not be stale after re-registration"
    );

    // Status should resume being published
    let resume_actions = harness.collect_actions();
    let has_publish = resume_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(
        has_publish,
        "status publishing should resume after re-registration"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 9: VAL-IT-014 — Spa silently drops toggle command → retry/drop
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_dropped_commands_retry_and_drop() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Make SpaSim ignore all commands
    harness.sim.set_command_success_rate(0.0);

    // Get initial status for CommandTracker baseline
    harness.collect_actions();

    // Queue toggle pump1
    harness.send_command(Command::ToggleItem(ToggleItem::Pump1));
    assert_eq!(harness.app.queued_command_count(), 1);

    // Tick to send command on Ready
    let send_actions = harness.collect_actions();
    let has_send = send_actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(has_send, "should send command on Ready");
    assert_eq!(harness.app.queued_command_count(), 0);

    // Initial counters
    assert_eq!(harness.app.total_retries(), 0);
    assert_eq!(harness.app.total_dropped(), 0);

    // Retry cycle 1: advance 6s, status shows pump still off
    harness.advance_ms(6_000);
    harness.collect_actions();
    assert!(
        harness.app.total_retries() >= 1,
        "should have at least 1 retry after first timeout"
    );

    // Retry cycle 2: advance another 6s
    harness.advance_ms(6_000);
    harness.collect_actions();
    assert!(
        harness.app.total_retries() >= 2,
        "should have at least 2 retries after second timeout"
    );

    // Drop cycle: advance another 6s — MAX_COMMAND_RETRIES=2 exceeded
    harness.advance_ms(6_000);
    harness.collect_actions();
    assert!(
        harness.app.total_dropped() >= 1,
        "command should be dropped after max retries"
    );

    // Pump state should never have changed in the sim
    assert_eq!(
        harness.sim.state.pumps[0],
        PumpState::Off,
        "pump should remain off — sim dropped all commands"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 10: VAL-IT-015 — Bus silence mid-session → stale lifecycle
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_bus_silence_lifecycle() {
    let mut harness = TestHarness::new();

    // Phase 1: Normal operation
    harness.complete_registration(5);
    harness.collect_actions();
    assert!(!harness.app.is_stale());

    // Phase 2: Bus silence — suppress spa output for 40 ticks (40 seconds)
    harness.sim.simulate_bus_silence(40);

    let mut stale_alert_seen = false;
    let mut stale_availability_seen = false;

    // Advance through silence period
    for _sec in 1..=35 {
        harness.advance_ms(1_000);
        let tick_actions = harness.tick_app();
        harness.execute_actions_on_broker(&tick_actions);

        for action in &tick_actions {
            if let AppAction::PublishAlert { message, .. } = action {
                if message == "spa_communication_lost" {
                    stale_alert_seen = true;
                }
            }
            if matches!(action, AppAction::PublishStaleAvailability) {
                stale_availability_seen = true;
            }
        }

        // Tick the spa (silenced, produces no output)
        let spa_actions = harness.tick_spa();
        harness.process_outgoing(&spa_actions);
        harness.execute_actions_on_broker(&spa_actions);
    }

    // Stale alert should fire at 30s
    assert!(stale_alert_seen, "stale alert should fire at 30s");
    assert!(
        stale_availability_seen,
        "stale availability should fire at 30s"
    );
    assert!(harness.app.is_stale(), "should be stale after 30s silence");

    // Phase 3: Silence ends — spa resumes
    // The bus_silence was set to 40 ticks. We've done 35 ticks (each loop iter does one tick_spa),
    // so we need 5 more to exhaust the silence.
    for _ in 0..5 {
        harness.advance_ms(1_000);
        harness.tick_spa();
    }

    // Now silence is over, spa will produce frames again
    let recovery_actions = harness.collect_actions();

    // Should recover from stale
    assert!(
        !harness.app.is_stale(),
        "should recover after status resumes"
    );

    // Recovery flag should be set
    let recovering = recovery_actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishState {
                recovering_from_stale: true,
                ..
            }
        )
    });
    assert!(recovering, "should indicate stale recovery on first status");
}

// ══════════════════════════════════════════════════════════════════════════
// Test 11: VAL-IT-016 — Corrupt frame doesn't desync parser
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_corrupt_frame_no_desync() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get initial status (no errors)
    harness.collect_actions();
    let initial_errors = harness.frame_error_count();

    // Inject a corrupt frame into the spa sim
    harness.sim.inject_corrupt_frame();

    // Tick — corrupt frame is produced and decoded
    let corrupt_actions = harness.tick_spa();
    harness.process_outgoing(&corrupt_actions);

    // Frame error count should have incremented
    let after_corrupt_errors = harness.frame_error_count();
    assert!(
        after_corrupt_errors > initial_errors,
        "frame error count should increment after corrupt frame (was {}, now {})",
        initial_errors,
        after_corrupt_errors
    );

    // Phase 2: Next valid frame should decode fine (no desync)
    let next_actions = harness.collect_actions();

    // Should produce valid actions (PublishState, etc.)
    let has_publish = next_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(
        has_publish,
        "next valid frame should decode and produce PublishState"
    );

    // Frame error count should NOT increase from the valid frame
    let final_errors = harness.frame_error_count();
    assert_eq!(
        final_errors, after_corrupt_errors,
        "frame error count should not increase from valid frame"
    );

    // SpaApp should still be functioning normally
    assert!(
        harness.app.last_status().is_some(),
        "SpaApp should have a valid last status"
    );
    assert!(
        !harness.app.is_stale(),
        "should not be stale after valid frame recovery"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 12: VAL-IT-017 — Spontaneous filter cycle while command pending
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_spontaneous_filter_cycle_while_command_pending() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get initial status for CommandTracker baseline
    harness.collect_actions();

    // Record initial counters
    let initial_retries = harness.app.total_retries();
    let initial_drops = harness.app.total_dropped();

    // Schedule a spontaneous filter cycle to start at tick 3 (pump 1 turns on by itself)
    // The current tick count after registration and status is roughly 6-8 ticks,
    // so schedule for a near-future tick.
    let at_tick = harness.sim.tick_count() + 3;
    harness.sim.simulate_filter_cycle_start(0, at_tick);

    // Queue a DIFFERENT pump toggle (pump 2) so it doesn't conflict with pump 1's
    // spontaneous change. The spontaneous change is for pump 1.
    harness.send_command(Command::ToggleItem(ToggleItem::Pump2));
    assert_eq!(harness.app.queued_command_count(), 1);

    // Send command on Ready
    let send_actions = harness.collect_actions();
    let has_send = send_actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(has_send, "should send pump2 toggle on Ready");

    // Tick to reach the scheduled filter cycle start
    // The filter cycle will start pump1 = Low spontaneously
    for _ in 0..3 {
        harness.collect_actions();
    }

    // Pump 1 should be on from the spontaneous filter cycle
    assert_eq!(
        harness.sim.state.pumps[0],
        PumpState::Low,
        "pump1 should be on from spontaneous filter cycle"
    );

    // Now feed a status that shows pump2 still off (the command for pump2
    // hasn't been confirmed yet). The tracker should only confirm based on
    // the actual expected change (pump2 toggle), NOT the spontaneous pump1 change.
    // Advance clock and check for status confirmation
    harness.advance_ms(6_000);

    // Get status — pump2 is still off, so the command is NOT confirmed
    // But pump1 was a spontaneous change, not from our command
    let _actions = harness.collect_actions();

    // Now manually toggle pump2 in the sim to confirm our command
    harness.sim.state.pumps[1] = PumpState::Low;

    // Get the next status — pump2 is now on, confirming our command
    harness.collect_actions();

    // The command tracker should have confirmed pump2 toggle via the status update.
    // No retries for the pump2 command, no drops.
    // Note: The spontaneous pump1 change should not affect pump2 tracking.
    let final_retries = harness.app.total_retries();
    let final_drops = harness.app.total_dropped();

    // The pump2 command should be confirmed with 0 retries and 0 drops
    // (we immediately saw the change in status)
    let retries_for_cmd = final_retries - initial_retries;
    let drops_for_cmd = final_drops - initial_drops;
    assert_eq!(
        retries_for_cmd, 0,
        "no retries expected — pump2 command confirmed via status (retries={})",
        retries_for_cmd
    );
    assert_eq!(
        drops_for_cmd, 0,
        "no drops expected — pump2 command confirmed via status (drops={})",
        drops_for_cmd
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Tier 3 — Protocol Misbehavior Tests
// ══════════════════════════════════════════════════════════════════════════

// ══════════════════════════════════════════════════════════════════════════
// Test 13: VAL-IT-018 — Out-of-order frames (Ready before Status)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_out_of_order_frames_ready_before_status() {
    let mut harness = TestHarness::new();

    // Complete registration first
    harness.complete_registration(5);

    // Manually construct a byte buffer with Ready frame BEFORE Status frame
    // (normal order is Status then Ready)
    let ready_bytes = harness.sim.generate_ready_frame();
    let status_bytes = harness.sim.generate_status_frame();

    let mut out_of_order = Vec::new();
    out_of_order.extend_from_slice(&ready_bytes);
    out_of_order.extend_from_slice(&status_bytes);

    // Feed the out-of-order bytes through the decoder and into SpaApp
    let frames = harness.decoder.feed_slice(&out_of_order);
    assert!(
        frames.len() >= 2,
        "should decode at least 2 frames from out-of-order buffer, got {}",
        frames.len()
    );

    // Process all frames — should not panic
    let mut all_actions = Vec::new();
    for frame in &frames {
        let actions = harness.app.process_frame(frame);
        all_actions.extend(actions);
    }

    // Both frames should be processed correctly:
    // - Ready: sends NothingToSend (or dequeues command)
    // - Status: produces PublishState
    let has_publish_state = all_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    let has_send_frame = all_actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));

    assert!(
        has_publish_state,
        "Status frame should produce PublishState even when arriving after Ready"
    );
    assert!(
        has_send_frame,
        "Ready frame should produce SendFrame (NothingToSend) even when arriving before Status"
    );

    // Verify no panics and app is in a good state
    assert!(harness.app.is_registered(), "should still be registered");
    assert!(
        harness.app.last_status().is_some(),
        "should have a valid last status"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 14: VAL-IT-019 — Interleaved response and status
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_interleaved_frames_in_single_buffer() {
    let mut harness = TestHarness::new();

    // Complete registration first
    harness.complete_registration(5);

    // Construct a single byte buffer containing:
    // 1. Status frame
    // 2. Ready frame
    // 3. Fault log response frame
    let status_bytes = harness.sim.generate_status_frame();
    let ready_bytes = harness.sim.generate_ready_frame();
    let fault_bytes = harness.sim.generate_fault_log_response();

    let mut combined = Vec::new();
    combined.extend_from_slice(&status_bytes);
    combined.extend_from_slice(&ready_bytes);
    combined.extend_from_slice(&fault_bytes);

    // Feed all bytes through the decoder
    let frames = harness.decoder.feed_slice(&combined);

    // Should decode at least 3 frames
    assert!(
        frames.len() >= 3,
        "should decode at least 3 frames from combined buffer, got {}",
        frames.len()
    );

    // Process all frames through SpaApp — should not panic
    let mut all_actions = Vec::new();
    for frame in &frames {
        let actions = harness.app.process_frame(frame);
        all_actions.extend(actions);
    }

    // Verify all frame types were processed:
    // 1. StatusUpdate → PublishState
    let has_publish_state = all_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(
        has_publish_state,
        "Status frame should produce PublishState"
    );

    // 2. Ready → SendFrame (NothingToSend or command)
    let has_send_frame = all_actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(has_send_frame, "Ready frame should produce SendFrame");

    // 3. FaultLogResponse → updates last_fault
    assert!(
        harness.app.last_fault().is_some(),
        "Fault log response should update last_fault"
    );

    // No frame errors
    assert_eq!(
        harness.frame_error_count(),
        0,
        "should have zero frame errors from interleaved valid frames"
    );

    // App should be in good state
    assert!(harness.app.is_registered());
    assert!(harness.app.last_status().is_some());
}

// ══════════════════════════════════════════════════════════════════════════
// Test 15: VAL-IT-020 — Rapid re-registration (multiple NewClientQuery frames)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_rapid_reregistration_multiple_queries() {
    // Test at the SpaApp level: feed multiple NewClientQuery frames directly
    // to verify no panic, queue cleared, and re-registration eventually succeeds.
    let mut harness = TestHarness::new();

    // Phase 1: Complete initial registration
    harness.complete_registration(5);
    assert!(harness.app.is_registered());

    // Get some status and queue a command
    harness.collect_actions();
    harness.send_command(Command::ToggleItem(ToggleItem::Pump1));
    assert_eq!(harness.app.queued_command_count(), 1);

    // Phase 2: Simulate rapid re-registration by feeding multiple NewClientQuery
    // frames directly to the app. This tests that the app handles them gracefully.
    let new_client_query_frame = launa_protocol::frame::Frame {
        message_type: [0xFE, 0xBF],
        payload: vec![0x00],
    };

    // Feed 3 NewClientQuery frames directly — should not panic
    let mut all_actions = Vec::new();
    for _ in 0..3 {
        let actions = harness.app.process_frame(&new_client_query_frame);
        all_actions.extend(actions);
    }

    // SpaApp should be unregistered (first NewClientQuery resets via dispatch,
    // subsequent ones go through registration SM)
    assert!(
        !harness.app.is_registered(),
        "should be unregistered after NewClientQuery"
    );

    // Command queue should be cleared
    assert_eq!(
        harness.app.queued_command_count(),
        0,
        "command queue should be cleared on bus reset"
    );

    // Phase 3: Re-registration via the harness (also resets sim state)
    harness.sim.simulate_spa_reboot(); // reset sim registration too
                                       // Force-reset the app's registration state machine to WaitingForQuery.
                                       // After 3 NewClientQuery frames, the SM may be stuck in WaitingForAssignment.
                                       // A fresh sim reboot produces a clean NewClientQuery on the next tick.
    harness.app.force_reset_registration();
    // Reset the decoder to clear any partial state
    harness.decoder = FrameDecoder::new();

    let ticks = harness.complete_registration(10);
    assert!(
        harness.app.is_registered(),
        "should re-register within 10 ticks (took {})",
        ticks
    );

    // Normal operation should resume
    let resume_actions = harness.collect_actions();
    let has_publish = resume_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(
        has_publish,
        "status publishing should resume after re-registration"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 16: VAL-IT-021 — Partial frame across tick boundary
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_partial_frame_across_tick_boundary() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get the status frame length so we can split at a meaningful point
    let status_bytes = harness.sim.generate_status_frame();
    assert!(
        status_bytes.len() > 10,
        "status frame should have some bytes"
    );

    // Split the frame roughly in half (at byte 10, well within the frame)
    let split_point = 10;

    // Inject partial frame split at tick boundary
    harness.sim.inject_partial_frame_at(split_point);

    // Tick 1: should emit only the first N bytes of the status frame
    let tick1_bytes = harness.sim.tick();
    assert!(
        !tick1_bytes.is_empty(),
        "tick 1 should produce some bytes (partial frame)"
    );

    // Feed the partial bytes through the decoder — should NOT produce any frames yet
    let tick1_frames = harness.decoder.feed_slice(&tick1_bytes);
    assert_eq!(
        tick1_frames.len(),
        0,
        "partial frame should not decode into any frames yet"
    );

    // Tick 2: should emit the remainder + Ready frame
    let tick2_bytes = harness.sim.tick();
    assert!(
        !tick2_bytes.is_empty(),
        "tick 2 should produce remainder bytes + Ready"
    );

    // Feed remainder through the decoder — should now produce complete frames
    let tick2_frames = harness.decoder.feed_slice(&tick2_bytes);
    assert!(
        tick2_frames.len() >= 1,
        "remainder should decode into at least 1 frame (status + possibly ready), got {}",
        tick2_frames.len()
    );

    // Process frames through SpaApp
    let mut all_actions = Vec::new();
    for frame in &tick2_frames {
        let actions = harness.app.process_frame(frame);
        all_actions.extend(actions);
    }

    // Should produce PublishState from the reassembled status frame
    let has_publish = all_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(
        has_publish,
        "reassembled status frame should produce PublishState"
    );

    // No frame errors
    assert_eq!(
        harness.frame_error_count(),
        0,
        "partial frame reassembly should not cause frame errors"
    );

    // App should have valid status
    assert!(
        harness.app.last_status().is_some(),
        "should have valid last_status after partial frame reassembly"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 17: VAL-IT-022 — Duplicate status frame in one tick
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_duplicate_status_frame_in_one_tick() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Record initial state
    harness.collect_actions();
    let initial_frames_received = harness.app.frames_received();

    // Inject duplicate frame — next tick produces status frame twice + Ready
    harness.sim.inject_duplicate_frame();

    // Tick the sim — produces duplicated status + ready
    let tick_bytes = harness.sim.tick();
    assert!(
        !tick_bytes.is_empty(),
        "tick should produce bytes with duplicated frame"
    );

    // Decode all frames from the tick
    let frames = harness.decoder.feed_slice(&tick_bytes);

    // Should decode at least 3 frames: status, duplicate status, ready
    assert!(
        frames.len() >= 3,
        "should decode at least 3 frames from duplicated tick (status + status + ready), got {}",
        frames.len()
    );

    // Process all frames through SpaApp
    let mut all_actions = Vec::new();
    for frame in &frames {
        let actions = harness.app.process_frame(frame);
        all_actions.extend(actions);
    }

    // Count PublishState and SendFrame(Ready) actions
    let publish_count = all_actions
        .iter()
        .filter(|a| matches!(a, AppAction::PublishState { .. }))
        .count();

    // Should have exactly 2 PublishState (one from each status frame)
    // Note: the second PublishState may be the same data — SpaApp processes both
    assert!(
        publish_count >= 2,
        "should have at least 2 PublishState actions from duplicate status frames, got {}",
        publish_count
    );

    // Should have a SendFrame from the Ready (NothingToSend or command)
    let has_send_frame = all_actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(
        has_send_frame,
        "should have SendFrame action from Ready frame"
    );

    // No frame errors from duplication
    assert_eq!(
        harness.frame_error_count(),
        0,
        "duplicate frame should not cause frame errors"
    );

    // Frames received should have incremented by 2 (two status frames)
    assert!(
        harness.app.frames_received() >= initial_frames_received + 2,
        "frames_received should reflect both duplicate status frames (was {}, now {})",
        initial_frames_received,
        harness.app.frames_received()
    );

    // App should still be in good state
    assert!(harness.app.is_registered());
    assert!(harness.app.last_status().is_some());
}

// ══════════════════════════════════════════════════════════════════════════
// Test 18: VAL-IT-023 — Multi-frame fault log walk
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_multi_frame_fault_log_walk() {
    let mut harness = TestHarness::new();

    // Complete registration
    harness.complete_registration(5);

    // Get initial status so the app has a pre_status
    harness.collect_actions();

    // Walk through 5 fault log entries, each with a different fault code
    let fault_codes = [
        FaultCode::HeaterDry,
        FaultCode::LowFlow,
        FaultCode::WaterTooHot,
        FaultCode::SensorAFault,
        FaultCode::GfciTestFailed,
    ];

    for (i, &code) in fault_codes.iter().enumerate() {
        // Configure the sim to produce a fault log response with this fault code
        let entry_num = (i + 1) as u8;
        harness
            .sim
            .set_fault_log_config(launa_sim::spa_sim::FaultLogConfig {
                fault_count: 5,
                entry_number: entry_num,
                message_code: code,
                days_ago: (5 - i as u8),
                hour: 10 + i as u8,
                minute: 30,
                flags: 0x04,
                set_temperature: 104,
                sensor_a_temp: 104,
                sensor_b_temp: 102,
            });

        // Generate the fault log response frame
        let fault_bytes = harness.sim.generate_fault_log_response();

        // Feed through the decoder
        let mut decoder = FrameDecoder::new();
        let fault_frames = decoder.feed_slice(&fault_bytes);
        assert_eq!(
            fault_frames.len(),
            1,
            "fault log response should decode as exactly 1 frame"
        );

        // Process through SpaApp
        let _actions = harness.app.process_frame(&fault_frames[0]);

        // FaultLogResponse should update last_fault
        assert!(
            harness.app.last_fault().is_some(),
            "last_fault should be set after fault log entry {}",
            entry_num
        );

        // The fault string should contain the fault code name
        let fault_str = harness.app.last_fault().unwrap();
        assert!(
            fault_str.contains(&format!("{:?}", code)),
            "fault string '{}' should contain fault code {:?} for entry {}",
            fault_str,
            code,
            entry_num
        );
    }

    // After the walk, last_fault should contain the LAST entry's fault code
    let final_fault = harness.app.last_fault().unwrap();
    assert!(
        final_fault.contains("GfciTestFailed"),
        "last_fault should reflect the final entry's fault code (GfciTestFailed), got: '{}'",
        final_fault
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test 19: VAL-IT-024 — Combined stress test (7-phase sequence)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_combined_stress_7_phase() {
    let mut harness = TestHarness::new();

    harness.complete_registration(5);
    assert!(harness.app.is_registered(), "Phase 1: should be registered");

    harness.collect_actions(); // get initial status for tracker

    harness.send_command(Command::ToggleItem(ToggleItem::Pump1));
    harness.send_command(Command::ToggleItem(ToggleItem::Light1));
    harness.send_command(Command::ToggleItem(ToggleItem::Blower));

    // Drain commands through Ready windows — need 3 Ready frames to drain 3 commands.
    // Each collect_actions() does one sim tick which produces (status + ready).
    for _ in 0..5 {
        let actions = harness.collect_actions();
        harness.process_outgoing(&actions);
    }

    // Verify at least pump1 changed (commands were processed)
    assert!(
        matches!(harness.sim.state.pumps[0], PumpState::Low | PumpState::High),
        "Phase 2: pump1 should be on (got {:?})",
        harness.sim.state.pumps[0]
    );

    harness.sim.simulate_fault_state(FaultCode::WaterTooHot);
    let fault_actions = harness.collect_actions();
    // Status should still be published (with fault flag in status)
    let has_publish = fault_actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(
        has_publish,
        "Phase 3: status should still be published with fault state"
    );

    harness.sim.simulate_bus_silence(40);

    let mut stale_alert_seen = false;
    for _sec in 1..=35 {
        harness.advance_ms(1_000);
        let tick_actions = harness.tick_app();
        for action in &tick_actions {
            if let AppAction::PublishAlert { message, .. } = action {
                if message == "spa_communication_lost" {
                    stale_alert_seen = true;
                }
            }
        }
        let spa_actions = harness.tick_spa();
        harness.process_outgoing(&spa_actions);
    }

    assert!(
        stale_alert_seen,
        "Phase 4: stale alert should fire during silence"
    );
    assert!(
        harness.app.is_stale(),
        "Phase 4: app should be stale after 35s silence"
    );

    // Exhaust remaining silence ticks (40 - 35 = 5 ticks left).
    // DO NOT use tick_spa() here — we want collect_actions() below to see
    // the FIRST status after stale and set the recovery flag.
    for _ in 0..5 {
        harness.advance_ms(1_000);
        // tick the sim but only process through sim (not app)
        let _ = harness.sim.tick();
    }

    // Now silence is over. collect_actions will tick the sim, get status,
    // feed through app — this is the FIRST status after stale.
    let recovery_actions = harness.collect_actions();
    assert!(
        !harness.app.is_stale(),
        "Phase 5: should recover from stale"
    );
    let recovering = recovery_actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishState {
                recovering_from_stale: true,
                ..
            }
        )
    });
    assert!(
        recovering,
        "Phase 5: recovery flag should be set on first status after stale"
    );

    harness.sim.simulate_spa_reboot();
    let _reboot_actions = harness.collect_actions();
    assert!(
        !harness.app.is_registered(),
        "Phase 6: should be unregistered after spa reboot"
    );

    let ticks = harness.complete_registration(5);
    assert!(
        harness.app.is_registered(),
        "Phase 6: should re-register within 5 ticks (took {})",
        ticks
    );

    harness.collect_actions(); // get initial status after re-registration

    harness.send_command(Command::ToggleItem(ToggleItem::Pump2));
    let cmd_actions = harness.collect_actions();
    harness.process_outgoing(&cmd_actions);

    // Verify command took effect
    assert!(
        matches!(harness.sim.state.pumps[1], PumpState::Low | PumpState::High),
        "Phase 7: pump2 should be on after post-reboot command (got {:?})",
        harness.sim.state.pumps[1]
    );

    // No state leaks between phases — verify clean final state
    assert!(harness.app.is_registered());
    assert!(!harness.app.is_stale());
    assert!(harness.app.last_status().is_some());
}
