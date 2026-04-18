//! Simulator interaction tests.
//!
//! Tests that exercise SpaSim's thermal model and time progression:
//! tick-based time updates, heating behavior, and cooling behavior.
//!
//! All tests verify behavior through decoded status frames (observable protocol
//! output) rather than sim.state.* (internal simulator state).

use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::PumpState;
use launa_sim::SpaSim;

#[test]
fn test_simulator_tick_updates_time() {
    let mut sim = SpaSim::new();
    let mut decoder = FrameDecoder::new();

    // Verify initial time through decoded status frame
    let status_bytes = sim.generate_status_frame();
    let frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.hour, 14);
        assert_eq!(s.minute, 30);
    } else {
        panic!("Expected StatusUpdate");
    }

    sim.tick();
    let status_bytes = sim.generate_status_frame();
    let frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.minute, 31);
    } else {
        panic!("Expected StatusUpdate");
    }

    for _ in 0..29 {
        sim.tick();
    }
    let status_bytes = sim.generate_status_frame();
    let frames = decoder.feed_slice(&status_bytes);
    let msg = dispatch_frame(&frames[0]);
    if let IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(s.minute, 0);
        assert_eq!(s.hour, 15);
    } else {
        panic!("Expected StatusUpdate");
    }
}

#[test]
fn test_simulator_tick_heating_approaches_set_temp() {
    let mut sim = SpaSim::new();
    // Rationale: setting sim.state fields here is necessary to configure
    // the test scenario — we're testing that the sim's thermal model heats
    // correctly and the result is observable through decoded status frames.
    // The state fields are test inputs, not test assertions.
    sim.state.current_temp = 95.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    let mut decoder = FrameDecoder::new();
    let mut reached = false;

    for _ in 0..50 {
        sim.tick();
        let status_bytes = sim.generate_status_frame();
        let frames = decoder.feed_slice(&status_bytes);
        let msg = dispatch_frame(&frames[0]);
        if let IncomingMessage::StatusUpdate(s) = msg {
            if let Some(temp) = s.current_temp {
                if temp >= 100.0 {
                    reached = true;
                    break;
                }
            }
        }
    }
    assert!(
        reached,
        "should reach set_temp after 50 ticks through decoded status frames"
    );
}
