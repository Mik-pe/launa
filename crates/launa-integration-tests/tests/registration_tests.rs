//! Registration flow integration tests.
//!
//! Tests for the client ID registration protocol:
//! - Full registration flow using RegistrationStateMachine + SpaSim
//! - RegistrationStateMachine state transitions and reset
//! - Registration race condition (commands queued before registration)
//! - SpaApp registration end-to-end with SpaSim
//! - Registration with interleaved frames
//! - Client hash validation
//! - Existing client reconnection

mod common;

use common::{
    full_registration, make_client_id_assignment_frame, make_new_client_query_frame, make_spaapp,
    make_status_frame,
};

use launa_core::AppAction;
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_protocol::registration::{
    RegistrationAction, RegistrationMessage, RegistrationState, RegistrationStateMachine,
};
use launa_sim::SpaSim;

const TEST_HASH: [u8; 2] = [0xF1, 0x73];

#[test]
fn test_full_registration_flow() {
    let mut sim = SpaSim::new();
    let mut client_sm = RegistrationStateMachine::new(TEST_HASH);
    let mut decoder = FrameDecoder::new();

    assert_eq!(client_sm.state(), &RegistrationState::WaitingForQuery);

    let query = sim.generate_registration_query();
    let query_frames = decoder.feed_slice(&query);
    assert_eq!(query_frames.len(), 1);

    let query_msg = dispatch_frame(&query_frames[0]);
    assert_eq!(
        query_msg,
        IncomingMessage::Registration(RegistrationMessage::NewClientQuery)
    );

    let action = client_sm.process(&RegistrationMessage::NewClientQuery);
    assert_eq!(action, RegistrationAction::SendNewClientResponse);
    assert_eq!(client_sm.state(), &RegistrationState::WaitingForAssignment);

    // Send client request with hash
    let client_request = RegistrationMessage::NewClientResponse {
        device_type: 0x02,
        client_hash: TEST_HASH,
    }
    .encode()
    .unwrap();
    let request_frames = decoder.feed_slice(&client_request);
    let request_frame = &request_frames[0];

    let assignment = sim.process_frame(request_frame).expect("should assign ID");

    let assignment_frames = decoder.feed_slice(&assignment);
    let assignment_frame = &assignment_frames[0];
    assert_eq!(assignment_frame.message_type, [0xFE, 0xBF]);

    let assignment_msg = dispatch_frame(assignment_frame);
    match assignment_msg {
        IncomingMessage::Registration(RegistrationMessage::ClientIdAssignment {
            channel,
            client_hash,
        }) => {
            assert_eq!(channel, 0x11);
            // Hash should be echoed back
            assert_eq!(client_hash, TEST_HASH);

            let action = client_sm.process(&RegistrationMessage::ClientIdAssignment {
                channel,
                client_hash,
            });
            assert_eq!(
                action,
                RegistrationAction::SendClientIdAck { client_id: channel }
            );
            assert!(client_sm.is_registered());
            assert_eq!(client_sm.client_id(), Some(0x11));

            let ack = RegistrationMessage::ClientIdAck { channel }
                .encode()
                .unwrap();
            let ack_frames = decoder.feed_slice(&ack);
            sim.process_frame(&ack_frames[0]);
            assert_eq!(sim.client_id, Some(0x11));
        }
        _ => panic!("Expected Registration(ClientIdAssignment)"),
    }
}

#[test]
fn test_registration_state_machine_reset() {
    let mut sm = RegistrationStateMachine::new(TEST_HASH);
    sm.process(&RegistrationMessage::NewClientQuery);
    assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

    sm.reset();
    assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
    assert!(!sm.is_registered());
}

#[test]
fn test_registration_flow_with_state_machine() {
    let mut sm = RegistrationStateMachine::new(TEST_HASH);
    assert!(!sm.is_registered());
    assert!(matches!(sm.state(), RegistrationState::WaitingForQuery));

    let action = sm.process(&RegistrationMessage::NewClientQuery);
    assert_eq!(
        action,
        RegistrationAction::SendNewClientResponse,
        "should respond to query with new client response"
    );
    assert!(matches!(
        sm.state(),
        RegistrationState::WaitingForAssignment
    ));

    let action = sm.process(&RegistrationMessage::ClientIdAssignment {
        channel: 0x03,
        client_hash: TEST_HASH,
    });
    assert_eq!(
        action,
        RegistrationAction::SendClientIdAck { client_id: 0x03 },
        "should send ack after assignment"
    );
    assert!(sm.is_registered(), "should be registered after assignment");

    let cmd = Command::NothingToSend { client_id: 0x03 };
    let (mt, _) = cmd.encode().unwrap();
    assert_eq!(mt, [0x03, 0xBF]);
}

#[test]
fn test_hash_mismatch_rejected() {
    let mut sm = RegistrationStateMachine::new(TEST_HASH);
    sm.process(&RegistrationMessage::NewClientQuery);

    // Assignment with wrong hash should be ignored
    let action = sm.process(&RegistrationMessage::ClientIdAssignment {
        channel: 0x05,
        client_hash: [0xAA, 0xBB],
    });
    assert_eq!(action, RegistrationAction::None);
    assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);
    assert!(!sm.is_registered());

    // Assignment with correct hash should succeed
    let action = sm.process(&RegistrationMessage::ClientIdAssignment {
        channel: 0x06,
        client_hash: TEST_HASH,
    });
    assert_eq!(
        action,
        RegistrationAction::SendClientIdAck { client_id: 0x06 }
    );
    assert!(sm.is_registered());
}

#[test]
fn test_legacy_assignment_without_hash_accepted() {
    let mut sm = RegistrationStateMachine::new(TEST_HASH);
    sm.process(&RegistrationMessage::NewClientQuery);

    // Legacy assignment with zero hash (00 00) should be accepted
    let action = sm.process(&RegistrationMessage::ClientIdAssignment {
        channel: 0x05,
        client_hash: [0x00, 0x00],
    });
    assert_eq!(
        action,
        RegistrationAction::SendClientIdAck { client_id: 0x05 }
    );
    assert!(sm.is_registered());
}

#[test]
fn test_existing_client_reconnection() {
    let mut sm = RegistrationStateMachine::with_previous_channel(TEST_HASH, 0x05);
    assert_eq!(sm.previous_channel(), Some(0x05));

    // On NewClientQuery, SM should try existing client path
    let action = sm.process(&RegistrationMessage::NewClientQuery);
    match action {
        RegistrationAction::SendExistingClientRequest { message } => {
            let msg = message.encode().unwrap();
            assert!(!msg.is_empty());
        }
        _ => panic!("Expected SendExistingClientRequest, got {:?}", action),
    }
    assert_eq!(sm.state(), &RegistrationState::WaitingForExistingResponse);

    // Spa confirms our existing client
    let action = sm.process(&RegistrationMessage::ExistingClientResponse {
        channel: 0x05,
        client_hash: TEST_HASH,
    });
    assert_eq!(action, RegistrationAction::None);
    assert!(sm.is_registered());
    assert_eq!(sm.client_id(), Some(0x05));
}

#[test]
fn test_existing_client_fallback_to_new() {
    let mut sm = RegistrationStateMachine::with_previous_channel(TEST_HASH, 0x05);
    sm.process(&RegistrationMessage::NewClientQuery);

    // Spa sends another query (doesn't recognize existing client)
    let action = sm.process(&RegistrationMessage::NewClientQuery);
    assert_eq!(action, RegistrationAction::SendNewClientResponse);
    assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

    // Normal assignment completes the flow
    let action = sm.process(&RegistrationMessage::ClientIdAssignment {
        channel: 0x06,
        client_hash: TEST_HASH,
    });
    assert_eq!(
        action,
        RegistrationAction::SendClientIdAck { client_id: 0x06 }
    );
    assert!(sm.is_registered());
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
    // NewClientResponse is handled by the sync fast-path (no-op in SpaApp)
    assert!(
        !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
        "NewClientResponse is handled by sync fast-path, no action from SpaApp"
    );

    // Process assignment (as if the sync fast-path response was received by spa)
    let actions = app.process_frame(&make_client_id_assignment_frame(0x03));
    // ACK is sent immediately on assignment
    assert!(
        actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
        "should send ID ack immediately on assignment"
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
            message_type: [0x03, 0xBF],
            payload: vec![0x06],
        });
        let frame_data = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Ready {} should produce SendFrame", i + 1));
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
    use launa_protocol::registration::RegistrationMessage;

    let (_clock, app) = make_spaapp();
    let mut app = app;
    let mut sim = SpaSim::new();

    let raw_bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&raw_bytes);

    // Process all frames — NewClientQuery is a no-op in SpaApp (sync fast-path
    // handles it). Simulate the fast-path by detecting the query and generating
    // the response ourselves.
    let mut all_actions: Vec<AppAction> = Vec::new();
    for frame in &frames {
        let actions = app.process_frame(frame);
        all_actions.extend(actions);

        // Simulate sync fast-path: detect NewClientQuery and send response
        if frame.message_type == [0xFE, 0xBF]
            && frame.payload.len() == 1
            && frame.payload[0] == 0x00
        {
            let client_hash = app.client_hash();
            let response_msg = RegistrationMessage::NewClientResponse {
                device_type: 0x02,
                client_hash,
            };
            let response_bytes = response_msg.encode().expect("encode should succeed");
            let assignment_bytes = sim.process_incoming_bytes(&response_bytes);
            assert!(
                !assignment_bytes.is_empty(),
                "should return assignment bytes"
            );

            let assignment_frames = decoder.feed_slice(&assignment_bytes);
            // Process assignment through SpaApp — ClientIdAck sent immediately
            let actions = app.process_frame(&assignment_frames[0]);
            all_actions.extend(actions);
        }
    }
    assert!(app.is_registered(), "should be registered after assignment");
}

#[test]
fn test_spaapp_existing_client_reconnection_e2e() {
    // First, register normally
    let (_clock, app) = make_spaapp();
    let mut app = app;
    let mut sim = SpaSim::new();
    full_registration(&mut sim, &mut app);

    let _assigned_id = app.client_id().expect("should have client ID");
    assert!(app.is_registered());

    // Simulate spa reboot — spa forgets all clients
    sim.simulate_spa_reboot();
    assert!(sim.client_id.is_none());

    // Spa starts sending registration queries again
    let raw_bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&raw_bytes);

    // The app is still registered from its perspective, but the sim is not
    // In real life, the app would detect stale and reset registration
    // For this test, just verify the sim sends a registration query
    let has_query = frames
        .iter()
        .any(|f| f.message_type == [0xFE, 0xBF] && !f.payload.is_empty() && f.payload[0] == 0x00);
    assert!(has_query, "rebooted spa should send registration query");
}
