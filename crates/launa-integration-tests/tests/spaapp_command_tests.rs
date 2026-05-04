//! SpaApp command tracking and lifecycle integration tests.
//!
//! Tests for SpaApp's command queue, tracking, retry/drop lifecycle,
//! concurrent operations, FIFO drain ordering, pump timers, hold mode,
//! diagnostics, registration timeout, bus reset, fault log capture,
//! heap monitoring, and stress scenarios.

mod common;

use common::{
    decode_first_frame, make_new_client_query_frame, make_ready_frame, make_spaapp,
    make_status_frame,
};

use launa_core::AppAction;
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
use launa_protocol::status::{HeatingMode, PumpState};
use launa_protocol::Temperature;
use launa_sim::SpaSim;

#[test]
fn test_spaapp_command_ack_and_confirmation() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    assert_eq!(app.queued_command_count(), 1);

    let actions = app.process_frame(&make_ready_frame());
    let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(has_send, "should send command on Ready");
    assert_eq!(app.queued_command_count(), 0);

    let mut sim = SpaSim::new();
    // Rationale: sim.state is test setup to create a confirming status frame —
    // the pump being on in sim is an input, verified through decoded status.
    sim.state.pumps[0] = PumpState::Low;
    let status_frame = decode_first_frame(&sim.generate_status_frame());

    let actions = app.process_frame(&status_frame);
    let has_state = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(has_state);
    assert_eq!(
        app.total_retries(),
        0,
        "no retries expected on confirmation"
    );
    assert_eq!(app.total_dropped(), 0, "no drops expected on confirmation");
}

#[test]
fn test_spaapp_command_retry_on_ignore() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    app.process_frame(&make_ready_frame());

    clock.advance_ms(6_000);
    let _actions = app.process_frame(&make_status_frame());

    // Bug 6 fix: retries are queued, not sent immediately
    let has_retry_queued = app.queued_command_count() > 0;
    assert!(has_retry_queued, "should retry on first timeout (queued)");
    assert!(app.total_retries() > 0);
    // Dequeue the retry via Ready
    app.process_frame(&make_ready_frame());

    clock.advance_ms(6_000);
    let _actions = app.process_frame(&make_status_frame());
    let has_second_retry = app.queued_command_count() > 0;
    assert!(has_second_retry, "should retry on second timeout (queued)");

    clock.advance_ms(6_000);
    app.process_frame(&make_status_frame());
    assert!(
        app.total_dropped() > 0,
        "command should be dropped after max retries"
    );
}

#[test]
fn test_spaapp_hold_mode_safety_timeout() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    let mut hold_frame = make_status_frame();
    hold_frame.payload[0] = 0x05;
    app.process_frame(&hold_frame);

    clock.advance_ms(61 * 60 * 1000);

    let actions = app.process_frame(&hold_frame);
    // Bug 6 fix: hold timer command is queued, not sent immediately
    let has_toggle_queued = app.queued_command_count() > 0;
    assert!(
        has_toggle_queued,
        "should queue hold toggle after 60 min safety timeout"
    );
    // Verify no immediate SendFrame for the toggle
    assert!(
        !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
        "hold timer should NOT produce immediate SendFrame"
    );
    // Dequeue via Ready
    let ready_actions = app.process_frame(&make_ready_frame());
    assert!(
        ready_actions
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_))),
        "Ready should dequeue and send the hold toggle"
    );
}

#[test]
fn test_spaapp_pump_timer_expiry() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    let actions = app.start_pump_timer(1, 1);
    assert!(
        actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
        "start_pump_timer should return toggle-on action"
    );

    let mut status = make_status_frame();
    status.payload[11] = 0x01;
    app.process_frame(&status);

    clock.advance_ms(61_000);

    let actions = app.process_frame(&status);
    // Bug 6 fix: pump timer auto-off is queued, not sent immediately
    let has_auto_off_queued = app.queued_command_count() > 0;
    assert!(
        has_auto_off_queued,
        "should queue auto-off after timer expiry"
    );
    assert!(
        !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
        "pump auto-off should NOT produce immediate SendFrame"
    );
    // Dequeue via Ready
    let ready_actions = app.process_frame(&make_ready_frame());
    assert!(
        ready_actions
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_))),
        "Ready should dequeue and send the pump auto-off"
    );
}

#[test]
fn test_spaapp_diagnostics_periodic() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());
    app.process_frame(&make_status_frame());
    assert_eq!(app.frames_received(), 2);

    clock.advance_ms(61_000);

    let actions = app.tick();
    let diag = actions.iter().find_map(|a| match a {
        AppAction::PublishDiagnostics {
            uptime_secs,
            frames_received,
            command_retries,
            command_drops,
            ..
        } => Some((
            *uptime_secs,
            *frames_received,
            *command_retries,
            *command_drops,
        )),
        _ => None,
    });
    assert!(diag.is_some(), "should publish diagnostics at 60s");
    let (uptime, frames, retries, drops) = diag.unwrap();
    assert_eq!(uptime, 61);
    assert_eq!(frames, 2);
    assert_eq!(retries, 0);
    assert_eq!(drops, 0);
}

#[test]
fn test_spaapp_registration_timeout() {
    let (clock, app) = make_spaapp();
    let mut app = app;

    let _actions = app.process_frame(&make_new_client_query_frame());
    // SendNewClientResponse is SUPPRESSED (fast-path handles it).
    // registration_started_at is still set, so the timeout timer runs.
    let _actions = app.process_frame(&make_ready_frame());
    assert!(!app.is_registered());

    clock.advance_ms(6_000);

    let actions = app.tick();
    let has_timeout_alert = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishAlert { message, .. } if message == "registration_timeout"
        )
    });
    assert!(
        has_timeout_alert,
        "should publish registration_timeout alert"
    );
    assert!(!app.is_registered());
}

#[test]
fn test_spaapp_registered_ignores_new_client_query() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);
    assert!(app.is_registered());
    assert_eq!(app.client_id(), Some(0x03));

    app.process_frame(&make_status_frame());

    // NewClientQuery is now ignored when registered
    let actions = app.process_frame(&make_new_client_query_frame());
    assert!(app.is_registered(), "should stay registered");
    assert_eq!(app.client_id(), Some(0x03), "client_id should be preserved");
    assert!(actions.is_empty(), "no actions from ignored NewClientQuery");

    // Still registered after another query
    let actions = app.process_frame(&make_new_client_query_frame());
    assert!(
        app.is_registered(),
        "should stay registered on second query too"
    );
    assert!(actions.is_empty());
}

#[test]
fn test_spaapp_temperature_not_validated_in_app() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    app.on_mqtt_command(Command::SetTemperature(106));
    assert_eq!(app.queued_command_count(), 1);

    let actions = app.process_frame(&make_ready_frame());
    let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(
        has_send,
        "SpaApp should send SetTemperature without validation"
    );
}

#[test]
fn test_spaapp_concurrent_operations() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    app.on_mqtt_command(Command::SetTemperature(102));
    app.on_mqtt_command(Command::ToggleItem(ToggleItem::HeatingMode));
    assert_eq!(app.queued_command_count(), 3);

    let actions1 = app.process_frame(&make_ready_frame());
    assert!(actions1
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_))));
    assert_eq!(app.queued_command_count(), 2);

    let actions2 = app.process_frame(&make_ready_frame());
    assert!(actions2
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_))));
    assert_eq!(app.queued_command_count(), 1);

    let actions3 = app.process_frame(&make_ready_frame());
    assert!(actions3
        .iter()
        .any(|a| matches!(a, AppAction::SendFrame(_))));
    assert_eq!(app.queued_command_count(), 0);

    let mut sim = SpaSim::new();
    // Rationale: sim.state fields are test setup to create a confirming status
    // frame that reflects all three command results simultaneously.
    sim.state.pumps[0] = PumpState::Low;
    sim.state.set_temp = Temperature::fahrenheit(102.0);
    sim.state.heating_mode = HeatingMode::Rest;
    let status_frame = decode_first_frame(&sim.generate_status_frame());

    let actions = app.process_frame(&status_frame);
    assert_eq!(app.total_retries(), 0, "no retries expected");
    assert_eq!(app.total_dropped(), 0, "no drops expected");
    let has_state = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { .. }));
    assert!(has_state, "should publish state after confirmation");
}

#[test]
fn test_spaapp_fault_log_captured() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    let fault_frame = Frame {
        message_type: [0x0A, 0xBF],
        payload: vec![
            0x28, 0x03, 0x01, 0x1B, 0x02, 0x0E, 0x1E, 0x04, 0x68, 0x68, 0x66,
        ],
    };
    app.process_frame(&fault_frame);
    assert!(app.last_fault().is_some(), "should capture fault log");

    let actions = app.process_frame(&make_status_frame());
    let has_fault_in_state = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { fault: Some(_), .. }));
    assert!(
        has_fault_in_state,
        "next PublishState should include fault string"
    );
}

#[test]
fn test_spaapp_ready_window_command_queuing() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
    app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump3));
    assert_eq!(app.queued_command_count(), 3);

    app.process_frame(&make_ready_frame());
    assert_eq!(app.queued_command_count(), 2);

    app.process_frame(&make_ready_frame());
    assert_eq!(app.queued_command_count(), 1);

    app.process_frame(&make_ready_frame());
    assert_eq!(app.queued_command_count(), 0);

    let actions = app.process_frame(&make_ready_frame());
    let has_nts = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
    assert!(has_nts, "should send NothingToSend when queue is empty");
    assert_eq!(app.queued_command_count(), 0);
}

#[test]
fn test_spaapp_24_hour_smoke() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    let mut diag_count: u32 = 0;
    let mut sim = SpaSim::new();

    // Rationale: sim.state.pumps is test setup — pump on creates a realistic
    // thermal model in the 24h smoke test; verified through SpaApp status.
    sim.state.pumps[0] = PumpState::Low;

    for _ in 0..1000 {
        clock.advance_ms(1_000);

        let raw_bytes = sim.tick();

        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&raw_bytes);

        for frame in &frames {
            if frame.message_type == [0xFF, 0xAF] {
                app.process_frame(frame);
            } else if frame.message_type == [0x10, 0xBF] {
                app.process_frame(frame);
            }
        }

        let actions = app.tick();
        diag_count += actions
            .iter()
            .filter(|a| matches!(a, AppAction::PublishDiagnostics { .. }))
            .count() as u32;
    }

    let remaining_secs: u64 = 86_400 - 1000;
    let jumps = remaining_secs / 60;
    for _ in 0..jumps {
        clock.advance_ms(60_000);

        let status_bytes = sim.generate_status_frame();
        let status_frame = decode_first_frame(&status_bytes);
        app.process_frame(&status_frame);

        app.process_frame(&make_ready_frame());

        let actions = app.tick();
        diag_count += actions
            .iter()
            .filter(|a| matches!(a, AppAction::PublishDiagnostics { .. }))
            .count() as u32;
    }

    let status = app.last_status().expect("should have a status");
    assert!(
        status.current_temp >= Some(Temperature::fahrenheit(104.0)),
        "temperature should have reached set point: {:?}",
        status.current_temp
    );

    assert!(
        diag_count > 1000,
        "should have many diagnostics publishes over 24h, got {}",
        diag_count
    );

    assert_eq!(app.queued_command_count(), 0);

    assert!(
        app.frames_received() > 1000,
        "should have received many frames: {}",
        app.frames_received()
    );

    assert!(!app.is_stale(), "should not be stale after 24h of frames");
}

#[test]
fn test_spaapp_stress_rapid_commands() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    let queue_cap: usize = 32;
    // Use ConfigurationRequest to avoid toggle deduplication
    for _ in 0..100 {
        app.on_mqtt_command(Command::ConfigurationRequest);
    }
    assert_eq!(
        app.queued_command_count(),
        queue_cap,
        "queue should be capped at {}",
        queue_cap
    );

    let mut send_frame_count: u32 = 0;
    let mut sim = SpaSim::new();

    // Drain all commands (original + retries) until queue is empty or we hit a limit.
    // Bug 6 fix: retries are now queued, so we may need more Ready cycles than
    // the original 32 to drain both initial commands and their retries.
    let max_drain_cycles = queue_cap * 4;
    for _ in 0..max_drain_cycles {
        if app.queued_command_count() == 0 {
            break;
        }

        clock.advance_ms(1_000);

        let actions = app.process_frame(&make_ready_frame());
        if actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))) {
            send_frame_count += 1;
        }

        let status_bytes = sim.generate_status_frame();
        let status_frame = decode_first_frame(&status_bytes);
        app.process_frame(&status_frame);
    }

    // Note: the queue may not be fully empty because retries keep getting queued.
    // The important thing is that commands flow through the queue and get sent.
    assert!(
        app.queued_command_count() < queue_cap,
        "queue should have drained significantly, got {}",
        app.queued_command_count()
    );

    assert!(
        send_frame_count >= queue_cap as u32,
        "should have sent at least {} frames, got {}",
        queue_cap,
        send_frame_count
    );

    let retries = app.total_retries();
    let drops = app.total_dropped();

    assert!(
        retries + drops > 0,
        "should have some retries or drops (spa never confirms): retries={}, drops={}",
        retries,
        drops
    );

    assert!(!app.is_stale(), "should not be stale");
}

#[test]
fn test_multi_command_fifo_drain() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    let commands = [
        Command::ToggleItem(ToggleItem::Pump1),
        Command::ToggleItem(ToggleItem::Pump2),
        Command::ToggleItem(ToggleItem::Pump3),
        Command::SetTemperature(100),
        Command::ToggleItem(ToggleItem::Light1),
    ];

    for cmd in &commands {
        app.on_mqtt_command(cmd.clone());
    }
    assert_eq!(app.queued_command_count(), 5);

    let expected_frames: Vec<Vec<u8>> = commands
        .iter()
        .map(|cmd| {
            let (mt, payload) = cmd.encode().unwrap();
            FrameEncoder::encode(mt, &payload).unwrap()
        })
        .collect();

    let mut actual_frames: Vec<Vec<u8>> = Vec::new();
    for i in 0..5 {
        let actions = app.process_frame(&make_ready_frame());
        let frame_data = actions
            .iter()
            .find_map(|a| match a {
                AppAction::SendFrame(data) => Some(data.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Ready {} should produce SendFrame", i + 1));
        actual_frames.push(frame_data);
    }

    assert_eq!(actual_frames.len(), 5, "should have sent exactly 5 frames");
    for (i, (actual, expected)) in actual_frames.iter().zip(expected_frames.iter()).enumerate() {
        assert_eq!(
            actual, expected,
            "command {} should match drain order (FIFO)",
            i
        );
    }

    assert_eq!(app.queued_command_count(), 0);

    let actions = app.process_frame(&make_ready_frame());
    let nts_frame = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should send NothingToSend when queue empty");
    let expected_nts = {
        let (mt, payload) = Command::NothingToSend { client_id: 0x03 }.encode().unwrap();
        FrameEncoder::encode(mt, &payload).unwrap()
    };
    assert_eq!(nts_frame, expected_nts, "should send NothingToSend");
}

#[test]
fn test_bounded_command_queue_cap() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    // Use ConfigurationRequest to avoid toggle deduplication
    for _ in 0..9 {
        app.on_mqtt_command(Command::ConfigurationRequest);
    }
    assert_eq!(app.queued_command_count(), 9);

    let mut send_count: usize = 0;
    for _ in 0..9 {
        let actions = app.process_frame(&make_ready_frame());
        if actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))) {
            send_count += 1;
        }
    }

    assert_eq!(send_count, 9, "all 9 commands should be sent");
    assert_eq!(app.queued_command_count(), 0, "queue should be empty");
}

#[test]
fn test_spaapp_tick_virtual_clock_diagnostics() {
    let (clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    let actions = app.tick();
    let has_diag = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishDiagnostics { .. }));
    assert!(has_diag, "should publish diagnostics on first tick");

    clock.advance_ms(59_000);
    let actions = app.tick();
    let no_diag = !actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishDiagnostics { .. }));
    assert!(no_diag, "should NOT publish diagnostics at 59s");

    clock.advance_ms(1_000);
    let actions = app.tick();
    let has_diag2 = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishDiagnostics { .. }));
    assert!(has_diag2, "should publish diagnostics at 60s");
}

#[test]
fn test_spaapp_heap_monitoring() {
    let (clock, app) = make_spaapp();
    let mut app = app;

    clock.advance_ms(31_000);

    let actions = app.check_heap(8192);
    let no_alert = !actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishAlert { .. }));
    assert!(no_alert, "should not alert on normal heap");

    clock.advance_ms(31_000);
    let actions = app.check_heap(500);
    let has_critical = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishAlert { message, .. } if message == "heap_critically_low"
        )
    });
    assert!(has_critical, "should alert on critically low heap");
}

#[test]
fn test_spaapp_fault_log_with_sim() {
    let (_clock, app) = make_spaapp();
    let mut app = app;
    app.force_registered(0x03);

    app.process_frame(&make_status_frame());

    let mut sim = SpaSim::new();
    let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode().unwrap();
    let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
    let mut decoder = FrameDecoder::new();
    let request_frames = decoder.feed_slice(&request_encoded);
    let response_bytes = sim
        .process_frame(&request_frames[0])
        .expect("should return fault log response");
    let response_frames = decoder.feed_slice(&response_bytes);

    app.process_frame(&response_frames[0]);
    assert!(
        app.last_fault().is_some(),
        "should capture fault log from SpaSim"
    );

    let actions = app.process_frame(&make_status_frame());
    let has_fault = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishState { fault: Some(_), .. }));
    assert!(has_fault, "should include fault in state publish");
}
