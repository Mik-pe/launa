//! Command retry and drop lifecycle integration tests.
//!
//! Tests for SpaApp's command retry/drop mechanism:
//! - Single command retry and drop lifecycle
//! - Command retry with SpaSim integration (command_success_rate=0)
//! - Multiple commands retry and drop independently

mod common;

use common::{make_ready_frame, make_spaapp, make_status_frame};

use launa_core::{AppAction, SpaApp};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_protocol::status::PumpState;
use launa_sim::SpaSim;

fn decode_first_frame(bytes: &[u8]) -> Frame {
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(bytes);
    assert!(!frames.is_empty(), "expected at least one frame");
    frames.into_iter().next().unwrap()
}

fn sim_tick_to_app(sim: &mut SpaSim, app: &mut SpaApp) -> Vec<AppAction> {
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

fn full_registration(sim: &mut SpaSim, app: &mut SpaApp) {
    let actions1 = sim_tick_to_app(sim, app);
    let id_request_bytes = actions1
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should have SendFrame for ID request");

    let assignment_bytes = sim.process_incoming_bytes(&id_request_bytes);
    assert!(!assignment_bytes.is_empty());

    let mut decoder = FrameDecoder::new();
    let assignment_frames = decoder.feed_slice(&assignment_bytes);
    assert_eq!(assignment_frames.len(), 1);

    let actions2 = app.process_frame(&assignment_frames[0]);
    assert!(app.is_registered());

    let ack_bytes = actions2
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should have SendFrame for ACK");

    sim.process_incoming_bytes(&ack_bytes);
}

#[test]
fn test_spaapp_command_retry_and_drop_lifecycle() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    app.process_frame(&make_ready_frame());
    assert_eq!(app.queued_command_count(), 0, "command should be dequeued");

    assert_eq!(app.total_retries(), 0);
    assert_eq!(app.total_dropped(), 0);

    clock.advance_ms(6_000);
    let actions = app.process_frame(&make_status_frame());
    let has_retry1 = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(has_retry1, "Retry 1: should resend command");
    assert_eq!(app.total_retries(), 1, "should have 1 retry");

    clock.advance_ms(6_000);
    let actions = app.process_frame(&make_status_frame());
    let has_retry2 = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(has_retry2, "Retry 2: should resend command");
    assert_eq!(app.total_retries(), 2, "should have 2 retries");

    clock.advance_ms(6_000);
    app.process_frame(&make_status_frame());
    assert!(
        app.total_dropped() > 0,
        "command should be dropped after exceeding max retries"
    );
    assert_eq!(app.total_retries(), 2, "no more retries after drop");
}

#[test]
fn test_spaapp_command_retry_with_sim_pipeline() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    let mut sim = SpaSim::new();

    full_registration(&mut sim, &mut app);

    sim.set_command_success_rate(0.0);

    let status_frame = decode_first_frame(&sim.generate_status_frame());
    app.process_frame(&status_frame);

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));

    let ready_frame = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let actions = app.process_frame(&ready_frame);
    let send_bytes = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should send command");

    sim.process_incoming_bytes(&send_bytes);

    clock.advance_ms(6_000);
    let status_bytes = sim.generate_status_frame();
    let status_frame = decode_first_frame(&status_bytes);
    let _actions = app.process_frame(&status_frame);
    assert!(app.total_retries() >= 1, "should have at least 1 retry");

    clock.advance_ms(6_000);
    let status_bytes = sim.generate_status_frame();
    let status_frame = decode_first_frame(&status_bytes);
    app.process_frame(&status_frame);
    assert!(app.total_retries() >= 2, "should have at least 2 retries");

    clock.advance_ms(6_000);
    let status_bytes = sim.generate_status_frame();
    let status_frame = decode_first_frame(&status_bytes);
    app.process_frame(&status_frame);
    assert!(
        app.total_dropped() > 0,
        "command should be dropped after max retries"
    );
}

#[test]
fn test_spaapp_multiple_command_retry_and_drop() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
    app.on_mqtt_command(Command::SetTemperature(100));

    app.process_frame(&make_ready_frame());
    app.process_frame(&make_ready_frame());
    app.process_frame(&make_ready_frame());
    assert_eq!(app.queued_command_count(), 0);

    clock.advance_ms(6_000);
    let actions = app.process_frame(&make_status_frame());
    let retry_count = actions
        .iter()
        .filter(|a| matches!(a, AppAction::SendFrame(_)))
        .count();
    assert!(
        retry_count >= 1,
        "at least one command should retry on first timeout"
    );

    for cycle in 0..10 {
        clock.advance_ms(6_000);
        app.process_frame(&make_status_frame());

        if app.total_dropped() >= 1 {
            break;
        }
        assert!(
            cycle < 9,
            "commands should have been dropped within 10 cycles"
        );
    }

    assert!(
        app.total_dropped() >= 1,
        "at least one command should be dropped"
    );
}

#[test]
fn test_spaapp_full_pipeline_register_status_command() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    let mut sim = SpaSim::new();

    full_registration(&mut sim, &mut app);
    assert!(app.is_registered());

    let status_bytes = sim.generate_status_frame();
    let status_frame = decode_first_frame(&status_bytes);
    let actions = app.process_frame(&status_frame);
    assert_eq!(app.frames_received(), 1);

    let has_state = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(has_state, "should publish state after status");

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    assert_eq!(app.queued_command_count(), 1);

    let ready_frame = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let actions = app.process_frame(&ready_frame);
    let send_bytes = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should send command on Ready");

    sim.process_incoming_bytes(&send_bytes);
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "sim should apply toggle"
    );

    let status_bytes = sim.generate_status_frame();
    let new_status_frame = decode_first_frame(&status_bytes);
    let _actions = app.process_frame(&new_status_frame);

    assert_eq!(app.total_retries(), 0, "no retries expected");
    assert_eq!(app.total_dropped(), 0, "no drops expected");

    let status = app.last_status().expect("should have status");
    assert!(
        matches!(status.pumps[0], PumpState::Low | PumpState::High),
        "pump1 should be on in app status"
    );
}
