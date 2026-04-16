//! SpaAppTestHarness — Integration test harness wiring SpaSim → FrameDecoder → SpaApp → SimBroker.
//!
//! Implements the Tier 1 core integration tests for the Launa spa controller:
//! 1. Harness initial state
//! 2. Registration E2E
//! 3. Status → MQTT publish
//! 4. Command → wire frame
//! 5. Pump timer auto-off
//! 6. Hold mode auto-release
//! 7. Stale detection and recovery

use launa_core::{AppAction, SpaApp};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::PumpState;
use launa_sim::{SimBroker, SpaSim, VirtualClock};
use std::boxed::Box;

// ══════════════════════════════════════════════════════════════════════════
// SpaAppTestHarness
// ══════════════════════════════════════════════════════════════════════════

/// Integration test harness wiring SpaSim → FrameDecoder → SpaApp → SimBroker.
///
/// The harness provides a clean pipeline:
/// - `tick_spa()`: SpaSim generates bytes → FrameDecoder decodes frames → SpaApp processes them
/// - `tick_app()`: SpaApp periodic tick for time-based checks (stale detection, etc.)
/// - `send_command()`: Queue MQTT command into SpaApp
/// - `complete_registration()`: Drive registration to completion
/// - `advance_ms()`: Advance VirtualClock by N milliseconds
/// - `collect_actions()`: Gather all actions from a tick cycle
pub struct SpaAppTestHarness {
    pub sim: SpaSim,
    pub app: SpaApp<'static>,
    pub broker: SimBroker,
    pub clock: &'static VirtualClock,
    decoder: FrameDecoder,
}

/// Collected actions from a single tick cycle.
pub struct TickResult {
    pub actions: Vec<AppAction>,
}

impl SpaAppTestHarness {
    /// Create a new harness with clean state: unregistered, no status, no publications.
    pub fn new() -> Self {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let sim = SpaSim::new();
        let app = SpaApp::new(clock);
        let broker = SimBroker::new("test_spa");

        SpaAppTestHarness {
            sim,
            app,
            broker,
            clock,
            decoder: FrameDecoder::new(),
        }
    }

    /// Run one SpaSim tick: generate spa bytes, decode frames, process through SpaApp.
    /// Returns all AppActions produced.
    pub fn tick_spa(&mut self) -> Vec<AppAction> {
        let spa_bytes = self.sim.tick();
        let frames = self.decoder.feed_slice(&spa_bytes);
        let mut all_actions = Vec::new();
        for frame in &frames {
            let actions = self.app.process_frame(frame);
            all_actions.extend(actions);
        }
        all_actions
    }

    /// Run SpaApp periodic tick (time-based checks: stale, diagnostics, etc.).
    pub fn tick_app(&mut self) -> Vec<AppAction> {
        self.app.tick()
    }

    /// Advance virtual clock by N milliseconds (without generating spa data).
    pub fn advance_ms(&mut self, ms: u64) {
        self.clock.advance_ms(ms);
    }

    /// Send an MQTT command into the SpaApp command queue.
    pub fn send_command(&mut self, cmd: Command) -> Vec<AppAction> {
        self.app.on_mqtt_command(cmd)
    }

    /// Drive registration to completion by ticking the spa until registered.
    /// Returns the number of ticks needed.
    /// Panics if registration doesn't complete within `max_ticks`.
    pub fn complete_registration(&mut self, max_ticks: usize) -> usize {
        for i in 0..max_ticks {
            // Tick the spa to generate bytes
            let actions = self.tick_spa();
            // Process outgoing frames from SpaApp back through SpaSim
            self.process_outgoing(&actions);

            if self.app.is_registered() {
                return i + 1;
            }

            // After sending outgoing frames, SpaSim may have generated responses
            // (e.g., ClientIdAssignment). We need to feed those back through.
            // The SpaSim queues its response when process_incoming_bytes is called,
            // but the response bytes aren't emitted until the next tick() or we
            // explicitly call process_incoming_bytes again.
            //
            // Actually, process_incoming_bytes returns response frames immediately.
            // We need to feed those responses through the decoder and app.
            let all_actions = actions;
            for action in &all_actions {
                if let AppAction::SendFrame(bytes) = action {
                    let responses = self.sim.process_incoming_bytes(bytes);
                    if !responses.is_empty() {
                        let resp_frames = self.decoder.feed_slice(&responses);
                        for frame in &resp_frames {
                            let resp_actions = self.app.process_frame(frame);
                            // Process any outgoing from responses (e.g., ID ack)
                            for ra in &resp_actions {
                                if let AppAction::SendFrame(rbytes) = ra {
                                    self.sim.process_incoming_bytes(rbytes);
                                }
                            }
                        }
                    }
                }
            }

            if self.app.is_registered() {
                return i + 1;
            }
        }
        panic!("Registration did not complete within {} ticks", max_ticks);
    }

    /// Process outgoing SendFrame actions from SpaApp through the SpaSim.
    /// This sends controller frames back to the simulator (e.g., registration responses,
    /// commands) so the simulator can update its state.
    pub fn process_outgoing(&mut self, actions: &[AppAction]) {
        for action in actions {
            if let AppAction::SendFrame(bytes) = action {
                let _responses = self.sim.process_incoming_bytes(bytes);
                // Responses from the sim (e.g., registration assignment) will be
                // picked up on the next tick_spa() call
            }
        }
    }

    /// Execute all AppActions against the broker (record PublishState, PublishAlert, etc.).
    pub fn execute_actions_on_broker(&mut self, actions: &[AppAction]) {
        for action in actions {
            match action {
                AppAction::PublishState {
                    status,
                    fault,
                    recovering_from_stale: _,
                } => {
                    self.broker.publish_state(status);
                    if let Some(f) = fault {
                        // Could record fault, but broker doesn't have a direct method
                        let _ = f;
                    }
                }
                AppAction::PublishAvailability { online } => {
                    self.broker.publish_availability(*online);
                }
                AppAction::PublishStaleAvailability => {
                    self.broker.publish_availability(false);
                }
                AppAction::PublishAlert { level, message } => {
                    self.broker
                        .publish(&format!("launa/test_spa/alert/{}", level), message);
                }
                AppAction::PublishDiagnostics {
                    uptime_secs,
                    frames_received,
                    command_retries,
                    command_drops,
                } => {
                    let payload = format!(
                        "{{\"uptime\":{},\"frames\":{},\"retries\":{},\"drops\":{}}}",
                        uptime_secs, frames_received, command_retries, command_drops
                    );
                    self.broker.publish("launa/test_spa/diagnostics", &payload);
                }
                _ => {}
            }
        }
    }

    /// Collect and execute all actions from a single spa tick cycle.
    /// Returns all actions produced.
    pub fn collect_actions(&mut self) -> Vec<AppAction> {
        let actions = self.tick_spa();
        // Process outgoing frames through sim
        self.process_outgoing(&actions);
        // Execute publish actions on broker
        self.execute_actions_on_broker(&actions);
        actions
    }

    /// Run a full tick cycle: spa tick + app tick, process outgoing, execute on broker.
    /// Returns all actions from both tick sources.
    pub fn full_tick(&mut self) -> Vec<AppAction> {
        let mut all_actions = self.tick_spa();
        self.process_outgoing(&all_actions);
        all_actions.extend(self.tick_app());
        self.execute_actions_on_broker(&all_actions);
        all_actions
    }

    /// Helper: count how many actions of a specific type are in the list.
    pub fn count_action_type(actions: &[AppAction], matcher: impl Fn(&AppAction) -> bool) -> usize {
        let mut count = 0;
        for a in actions {
            if matcher(a) {
                count += 1;
            }
        }
        count
    }

    /// Helper: check if any SendFrame action contains an encoded toggle for the given item.
    pub fn has_toggle_for(actions: &[AppAction], item: ToggleItem) -> bool {
        let (mt, payload) = Command::ToggleItem(item).encode();
        let expected = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();
        actions.iter().any(|a| {
            if let AppAction::SendFrame(bytes) = a {
                bytes == &expected
            } else {
                false
            }
        })
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Test 1: VAL-IT-001 — Harness initial state
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_harness_initial_state() {
    let harness = SpaAppTestHarness::new();

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
    let mut harness = SpaAppTestHarness::new();

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
    let mut harness = SpaAppTestHarness::new();

    // Complete registration first
    harness.complete_registration(5);

    // Clear any broker state from registration
    harness.broker.take_all();

    // Run 5 ticks, collecting publish actions
    let mut total_publish_state = 0;
    for _ in 0..5 {
        let actions = harness.collect_actions();
        total_publish_state += SpaAppTestHarness::count_action_type(&actions, |a| {
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
    let mut harness = SpaAppTestHarness::new();

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
    let mut harness = SpaAppTestHarness::new();

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
    let has_auto_off = SpaAppTestHarness::has_toggle_for(&auto_off_actions, ToggleItem::Pump1);
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
    let mut harness = SpaAppTestHarness::new();

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
    let fired = SpaAppTestHarness::has_toggle_for(&fire_actions, ToggleItem::HoldMode);
    assert!(
        fired,
        "hold timer should fire auto-release toggle at 60min boundary"
    );

    // Advance more time — should NOT re-fire while hold is still active (fired flag)
    harness.advance_ms(5_000);
    let no_refire_actions = harness.collect_actions();
    let refired = SpaAppTestHarness::has_toggle_for(&no_refire_actions, ToggleItem::HoldMode);
    assert!(
        !refired,
        "hold timer should NOT re-fire while hold mode is still active after firing"
    );

    // Advance another full timeout — still should not re-fire
    harness.advance_ms(61 * 60 * 1000);
    let no_refire2_actions = harness.collect_actions();
    let refired2 = SpaAppTestHarness::has_toggle_for(&no_refire2_actions, ToggleItem::HoldMode);
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
    let re_fired = SpaAppTestHarness::has_toggle_for(&re_fire_actions, ToggleItem::HoldMode);
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
    let mut harness = SpaAppTestHarness::new();

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
