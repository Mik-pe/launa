//! Registration flow integration tests.
//!
//! Tests for the client ID registration protocol:
//! - Full registration flow using RegistrationStateMachine + SpaSim
//! - RegistrationStateMachine state transitions and reset
//! - Registration race condition (commands queued before registration)
//! - SpaApp registration end-to-end with SpaSim
//! - Registration with interleaved frames

mod common;

use common::{
    full_registration, make_client_id_assignment_frame, make_new_client_query_frame, make_spaapp,
    make_status_frame,
};

use launa_core::AppAction;
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
use launa_protocol::registration::{
    RegistrationAction, RegistrationState, RegistrationStateMachine,
};
use launa_sim::SpaSim;

#[test]
fn test_full_registration_flow() {
    let mut sim = SpaSim::new();
    let mut client_sm = RegistrationStateMachine::new();
    let mut decoder = FrameDecoder::new();

    assert_eq!(client_sm.state(), &RegistrationState::WaitingForQuery);

    let query = sim.generate_registration_query();
    let query_frames = decoder.feed_slice(&query);
    assert_eq!(query_frames.len(), 1);

    let query_msg = dispatch_frame(&query_frames[0]);
    assert_eq!(query_msg, IncomingMessage::NewClientQuery);

    let action = client_sm.process([0xFE, 0xBF], &[0x00]);
    assert_eq!(action, RegistrationAction::SendIdRequest);
    assert_eq!(client_sm.state(), &RegistrationState::WaitingForAssignment);

    let client_request = FrameEncoder::encode([0xFE, 0xBF], &[0x01]).unwrap();
    let request_frames = decoder.feed_slice(&client_request);
    let request_frame = &request_frames[0];

    let assignment = sim.process_frame(request_frame).expect("should assign ID");

    let assignment_frames = decoder.feed_slice(&assignment);
    let assignment_frame = &assignment_frames[0];
    assert_eq!(assignment_frame.message_type, [0xFE, 0xBF]);

    let assignment_msg = dispatch_frame(assignment_frame);
    match assignment_msg {
        IncomingMessage::ClientIdAssignment { id } => {
            assert_eq!(id, 0x02);

            let action = client_sm.process([0xFE, 0xBF], &[0x02, id]);
            assert_eq!(action, RegistrationAction::SendIdAck { client_id: id });
            assert!(client_sm.is_registered());
            assert_eq!(client_sm.client_id(), Some(0x02));

            let ack = FrameEncoder::encode([id, 0xBF], &[0x03]).unwrap();
            let ack_frames = decoder.feed_slice(&ack);
            sim.process_frame(&ack_frames[0]);
            assert_eq!(sim.client_id, Some(0x02));
        }
        _ => panic!("Expected ClientIdAssignment"),
    }
}

#[test]
fn test_registration_state_machine_reset() {
    let mut sm = RegistrationStateMachine::new();
    sm.process([0xFE, 0xBF], &[0x00]);
    assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

    sm.reset();
    assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
    assert!(!sm.is_registered());
}

#[test]
fn test_registration_flow_with_state_machine() {
    use launa_protocol::registration::{
        RegistrationAction, RegistrationState, RegistrationStateMachine,
    };

    let mut sm = RegistrationStateMachine::new();
    assert!(!sm.is_registered());
    assert!(matches!(sm.state(), RegistrationState::WaitingForQuery));

    let action = sm.process([0xFE, 0xBF], &[0x00]);
    assert_eq!(
        action,
        RegistrationAction::SendIdRequest,
        "should respond to query with ID request"
    );
    assert!(matches!(
        sm.state(),
        RegistrationState::WaitingForAssignment
    ));

    let action = sm.process([0xFE, 0xBF], &[0x02, 0x03]);
    assert_eq!(
        action,
        RegistrationAction::SendIdAck { client_id: 0x03 },
        "should send ack after assignment"
    );
    assert!(sm.is_registered(), "should be registered after assignment");

    let cmd = Command::NothingToSend { client_id: 0x03 };
    let (mt, _) = cmd.encode();
    assert_eq!(mt, [0x03, 0xBF]);
}

#[test]
fn test_registration_race_condition() {
    let (_clock, app) = make_spaapp();
    let mut app = app;

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
    app.on_mqtt_command(Command::SetTemperature(100));
    assert_eq!(app.queued_command_count(), 3);
    assert!(!app.is_registered());

    let actions = app.process_frame(&make_new_client_query_frame());
    assert!(
        actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
        "should send ID request"
    );
    assert!(!app.is_registered());

    let actions = app.process_frame(&make_client_id_assignment_frame(0x03));
    assert!(
        actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
        "should send ID ack"
    );
    assert!(app.is_registered());
    assert_eq!(app.client_id(), Some(0x03));

    assert_eq!(
        app.queued_command_count(),
        3,
        "commands should survive registration"
    );

    app.process_frame(&make_status_frame());

    let mut sent_commands: Vec<Vec<u8>> = Vec::new();
    for i in 0..3 {
        let actions = app.process_frame(&Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        });
        let frame_data = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .expect(&format!("Ready {} should produce SendFrame", i + 1));
        sent_commands.push(frame_data);
    }

    assert_eq!(
        app.queued_command_count(),
        0,
        "all commands should be drained after 3 Ready frames"
    );
    assert_eq!(sent_commands.len(), 3);
}

#[test]
fn test_spaapp_registration_e2e() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    let mut sim = SpaSim::new();

    assert!(!app.is_registered());
    assert!(app.client_id().is_none());

    full_registration(&mut sim, &mut app);

    assert!(app.is_registered());
    assert_eq!(app.client_id(), sim.client_id);

    let raw = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&raw);
    let has_reg_query = frames
        .iter()
        .any(|f| f.message_type == [0xFE, 0xBF] && f.payload.contains(&0x00));
    assert!(
        !has_reg_query,
        "should not produce registration query after registration"
    );
}

#[test]
fn test_spaapp_registration_with_interleaved_frames() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    let mut sim = SpaSim::new();

    let raw_bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&raw_bytes);

    // Process all frames — the NewClientQuery triggers ID request
    let mut id_request_bytes = None;
    for frame in &frames {
        let actions = app.process_frame(frame);
        // Capture the ID request from the registration handler
        for a in &actions {
            if let AppAction::SendFrame(data) = a {
                id_request_bytes = Some(data.clone());
            }
        }
    }
    assert!(!app.is_registered(), "should not be registered yet");
    let id_request_bytes =
        id_request_bytes.expect("should have ID request SendFrame from NewClientQuery");

    let assignment_bytes = sim.process_incoming_bytes(&id_request_bytes);
    assert!(
        !assignment_bytes.is_empty(),
        "should return assignment bytes"
    );

    let assignment_frames = decoder.feed_slice(&assignment_bytes);
    for frame in &assignment_frames {
        app.process_frame(frame);
    }
    assert!(app.is_registered(), "should be registered after assignment");
}
