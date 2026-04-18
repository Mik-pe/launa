mod command_tests;
mod config_tests;
mod error_injection_tests;
mod fault_tests;
mod frame_gen_tests;
mod physics_tests;
mod state_tests;

use launa_protocol::frame::{FrameDecoder, FrameEncoder};

use super::*;

/// Helper: dispatch a SpaSim response frame through the protocol decoder.
fn dispatch_response(bytes: &[u8]) -> launa_protocol::dispatcher::IncomingMessage {
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(bytes);
    assert!(
        !frames.is_empty(),
        "response should produce at least one frame"
    );
    launa_protocol::dispatcher::dispatch_frame(&frames[0])
}

/// Helper: build the 0x22 request frame that triggers a settings response.
fn build_settings_request(sub_type: u8) -> Vec<u8> {
    let payload = vec![0x22, sub_type];
    FrameEncoder::encode([0x0A, 0xBF], &payload).unwrap()
}

/// Helper: build the 0x04 config request frame.
fn build_config_request() -> Vec<u8> {
    let payload = vec![0x04];
    FrameEncoder::encode([0x0A, 0xBF], &payload).unwrap()
}

/// Helper: dispatch a status frame and return the parsed status.
fn dispatch_status(sim: &mut SpaSim) -> launa_protocol::status::StatusUpdate {
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
    match msg {
        launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) => s,
        other => panic!("Expected StatusUpdate, got {:?}", other),
    }
}
