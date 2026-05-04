//! Shared test harness utilities for launa-integration-tests.
//!
//! Common helpers used across multiple integration test files for creating
//! SpaApp instances, test frames, and driving registration/decode pipelines.
//!
//! Cross-reference: structurally identical helpers exist in
//! launa-core/src/spa_app.rs tests module (make_app_with_clock, status_frame,
//! ready_frame). These are NOT consolidated into a shared crate because
//! launa-core does not depend on launa-integration-tests, and extracting
//! a shared test-util crate would add a new workspace dependency for
//! test-only code.

#![allow(dead_code)]

use launa_core::{AppAction, SpaApp};
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_sim::{SpaSim, VirtualClock};

/// Create a VirtualClock and SpaApp for testing.
///
/// The clock is leaked to obtain a `'static` reference, which is required
/// by SpaApp's lifetime parameter.
pub fn make_spaapp() -> (&'static VirtualClock, SpaApp<'static>) {
    let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let app = SpaApp::new(clock);
    (clock, app)
}

/// Create a standard status update frame with current_temp=100, set_temp=104.
pub fn make_status_frame() -> Frame {
    let mut payload = vec![0u8; 24];
    payload[2] = 100; // current temp
    payload[20] = 104; // set temp
    Frame {
        message_type: [0xFF, 0xAF],
        payload,
    }
}

/// Create a standard ready frame (client ready acknowledgement).
pub fn make_ready_frame() -> Frame {
    Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    }
}

/// Create a NewClientQuery frame (registration query from spa).
pub fn make_new_client_query_frame() -> Frame {
    Frame {
        message_type: [0xFE, 0xBF],
        payload: vec![0x00],
    }
}

/// Create a ClientIdAssignment frame with the given client ID.
pub fn make_client_id_assignment_frame(id: u8) -> Frame {
    Frame {
        message_type: [0xFE, 0xBF],
        payload: vec![0x02, id],
    }
}

/// Decode the first frame from raw bytes.
///
/// Panics if no frames are decoded.
pub fn decode_first_frame(bytes: &[u8]) -> Frame {
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(bytes);
    assert!(!frames.is_empty(), "expected at least one frame");
    frames.into_iter().next().unwrap()
}

/// Run one SpaSim tick and feed all decoded frames into SpaApp.
///
/// Returns all AppActions produced.
pub fn sim_tick_to_app(sim: &mut SpaSim, app: &mut SpaApp) -> Vec<AppAction> {
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

/// Drive the full registration handshake between SpaSim and SpaApp.
///
/// Ticks the sim once to get the registration query + status + Ready frame,
/// processes them through the app. Since the SpaApp's SendNewClientResponse
/// handler is a no-op (the sync fast-path in uart_task handles it on real
/// hardware), we detect the NewClientQuery frame and generate the response
/// ourselves, then feed it to the sim to get the ClientIdAssignment.
/// Panics if any step fails.
pub fn full_registration(sim: &mut SpaSim, app: &mut SpaApp) {
    use launa_protocol::registration::RegistrationMessage;

    let raw_bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&raw_bytes);

    // First pass: process all frames through SpaApp, and detect the
    // NewClientQuery to simulate the sync fast-path registration response.
    // We process assignment frames immediately but skip status frames so
    // they don't affect the test's frame counter expectations.
    for frame in &frames {
        let is_new_client_query = frame.message_type == [0xFE, 0xBF]
            && frame.payload.len() == 1
            && frame.payload[0] == 0x00;

        if is_new_client_query {
            // Process through SpaApp (triggers SM transition, sets registration_started_at)
            app.process_frame(frame);

            // Simulate sync fast-path: generate and send NewClientResponse to sim
            let client_hash = app.client_hash();
            let response_msg = RegistrationMessage::NewClientResponse {
                device_type: 0x02,
                client_hash,
            };
            let response_bytes = response_msg.encode().expect("encode should succeed");
            let assignment_bytes = sim.process_incoming_bytes(&response_bytes);
            assert!(
                !assignment_bytes.is_empty(),
                "should return client ID assignment bytes"
            );

            let assignment_frames = decoder.feed_slice(&assignment_bytes);
            assert_eq!(
                assignment_frames.len(),
                1,
                "should produce one assignment frame"
            );

            // Process assignment through SpaApp (queues ACK)
            let _actions2 = app.process_frame(&assignment_frames[0]);
            assert!(app.is_registered(), "should be registered after assignment");
        } else if !app.is_registered() {
            // Only process non-registration frames before we're registered
            // to avoid counting status frames in frames_received
            app.process_frame(frame);
        }
        // Skip frames that arrive after registration (they'll be processed
        // by the caller if needed, preventing double-counting)
    }

    // Send a Ready frame to trigger the queued ClientIdAck
    let ready_frame = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let actions3 = app.process_frame(&ready_frame);
    let has_ack = actions3
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(
        has_ack,
        "should send ID ack on Ready frame after assignment"
    );

    let ack_bytes = actions3
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should have SendFrame for ACK");

    sim.process_incoming_bytes(&ack_bytes);
}
