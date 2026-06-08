//! Stale detection integration tests.
//!
//! Tests for SpaApp's stale communication detection:
//! - Full stale detection flow (normal → probe → alert → recovery)
//! - Stale detection lifecycle with multiple probe phases
//! - Exact timing boundaries (29s not stale, 30s stale)

mod common;

use common::{make_ready_frame, make_spaapp, make_status_frame};

use launa_core::AppAction;
use launa_protocol::command::Command;
use launa_protocol::frame::FrameEncoder;

#[test]
fn test_spaapp_stale_detection_flow() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());
    // Send CTS to prevent CTS loss from firing during stale probe tests
    app.process_frame(&make_ready_frame(0x03));
    assert!(!app.is_stale());

    clock.advance_ms(6_000);
    // Send CTS right before tick to keep CTS loss timer happy
    app.process_frame(&make_ready_frame(0x03));
    let actions = app.tick();
    let has_probe = actions
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(bytes) if !bytes.is_empty()));
    assert!(has_probe, "should send config probe at 5s");

    clock.advance_ms(25_000);
    let actions = app.tick();
    let has_stale_avail = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishStaleAvailability));
    let has_alert = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishAlert { message, .. } if message == "spa_communication_lost"
        )
    });
    assert!(has_stale_avail, "should publish stale availability at 30s");
    assert!(has_alert, "should publish stale alert at 30s");
    assert!(app.is_stale());
    assert!(!app.is_registered(), "stale should reset registration");

    // Re-register (stale resets registration)
    app.force_registered(0x03);

    let actions = app.process_frame(&make_status_frame());
    assert!(!app.is_stale());
    let recovering = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishState {
                recovering_from_stale: true,
                ..
            }
        )
    });
    assert!(recovering, "should indicate stale recovery");
}

#[test]
fn test_spaapp_stale_detection_lifecycle() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());
    // Send CTS to prevent CTS loss from firing during stale probe tests
    app.process_frame(&make_ready_frame(0x03));
    assert!(
        !app.is_stale(),
        "should not be stale during normal operation"
    );

    clock.advance_ms(6_000);
    app.process_frame(&make_ready_frame(0x03)); // keep CTS alive
    let actions = app.tick();
    let probe_frames: Vec<&Vec<u8>> = actions
        .iter()
        .filter_map(|a| match a {
            AppAction::SendFrame(data) => Some(data),
            _ => None,
        })
        .collect();
    assert!(!probe_frames.is_empty(), "Phase 2: should send probe at 5s");
    let nts_expected = {
        let (mt, payload) = Command::NothingToSend { client_id: 0x03 }.encode().unwrap();
        FrameEncoder::encode(mt, &payload).unwrap()
    };
    assert!(
        probe_frames.contains(&&nts_expected),
        "Phase 2: probe should be NothingToSend, not ConfigurationRequest"
    );
    assert!(!app.is_stale(), "should not be stale at 6s");

    clock.advance_ms(5_000);
    app.process_frame(&make_ready_frame(0x03)); // keep CTS alive
    let actions = app.tick();
    let probe2_frames: Vec<&Vec<u8>> = actions
        .iter()
        .filter_map(|a| match a {
            AppAction::SendFrame(data) => Some(data),
            _ => None,
        })
        .collect();
    assert!(
        !probe2_frames.is_empty(),
        "Phase 2b: should send second probe at 10s"
    );

    clock.advance_ms(5_000);
    app.process_frame(&make_ready_frame(0x03)); // keep CTS alive
    let actions = app.tick();
    let probe3_frames: Vec<&Vec<u8>> = actions
        .iter()
        .filter_map(|a| match a {
            AppAction::SendFrame(data) => Some(data),
            _ => None,
        })
        .collect();
    assert!(
        !probe3_frames.is_empty(),
        "Phase 2c: should send third probe at 16s"
    );
    assert!(!app.is_stale(), "should not be stale at 16s");

    clock.advance_ms(15_000);
    let actions = app.tick();

    let has_stale_alert = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishAlert { message, .. } if message == "spa_communication_lost"
        )
    });
    assert!(
        has_stale_alert,
        "Phase 3: should publish stale alert at 30s"
    );

    let has_stale_avail = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishStaleAvailability));
    assert!(
        has_stale_avail,
        "Phase 3: should publish stale availability at 30s"
    );
    assert!(app.is_stale(), "Phase 3: should be stale at 31s");
    assert!(!app.is_registered(), "Phase 3: stale resets registration");

    // Re-register (stale resets registration)
    app.force_registered(0x03);

    let actions = app.process_frame(&make_status_frame());
    // Send CTS to establish normal CTS tracking after re-registration
    app.process_frame(&make_ready_frame(0x03));
    assert!(!app.is_stale(), "Phase 4: should recover after status");

    let has_recovery = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishState {
                recovering_from_stale: true,
                ..
            }
        )
    });
    assert!(has_recovery, "Phase 4: should indicate stale recovery");

    clock.advance_ms(6_000);
    app.process_frame(&make_ready_frame(0x03)); // keep CTS alive
    let actions = app.tick();
    let no_stale_alert = !actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishAlert { message, .. }
            if message == "spa_communication_lost"
        )
    });
    assert!(
        no_stale_alert,
        "Phase 4: should not re-trigger stale after recovery"
    );
}

#[test]
fn test_spaapp_stale_detection_exact_timing() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    clock.advance_ms(29_000);
    let actions = app.tick();
    let no_stale = !actions.iter().any(|a| {
        matches!(a, AppAction::PublishAlert { message, .. } if message == "spa_communication_lost")
    });
    assert!(no_stale, "should NOT be stale at 29s");
    assert!(!app.is_stale());

    clock.advance_ms(1_000);
    let actions = app.tick();
    let has_stale = actions.iter().any(|a| {
        matches!(a, AppAction::PublishAlert { message, .. } if message == "spa_communication_lost")
    });
    assert!(has_stale, "should be stale at 30s");
    assert!(app.is_stale());
}
