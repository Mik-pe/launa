//! Shared test harness utilities for launa-integration-tests.
//!
//! Common helpers used across multiple integration test files for creating
//! SpaApp instances and test frames.
//!
//! Cross-reference: structurally identical helpers exist in
//! launa-core/src/spa_app.rs tests module (make_app_with_clock, status_frame,
//! ready_frame). These are NOT consolidated into a shared crate because
//! launa-core does not depend on launa-integration-tests, and extracting
//! a shared test-util crate would add a new workspace dependency for
//! test-only code.

#![allow(dead_code)]

use launa_core::SpaApp;
use launa_protocol::frame::Frame;
use launa_sim::VirtualClock;

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
