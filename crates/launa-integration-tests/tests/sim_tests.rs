//! End-to-end simulation tests using the launa-sim framework.
//!
//! These tests exercise the complete pipeline:
//!   SpaSim → SimTransport → SpaController → SimBroker
//!
//! Simulating real RS-485 byte traffic through the controller logic and
//! verifying MQTT publications, just like the real firmware would behave.

use launa_sim::{SpaSim, SpaController, SimTransport, SimBroker, ControllerEvent};
use launa_sim::spa_sim::SpaState;
use launa_sim::{PumpState, TemperatureScale, HeatingMode, TempRange, ToggleItem};
use launa_hal::transport::Transport;
use launa_protocol::command::Command;
use launa_mqtt::command_parser;
use launa_mqtt::topics::TopicBuilder;

/// Helper to run one full tick cycle: spa generates bytes → controller processes them.
fn run_tick(
    spa: &mut SpaSim,
    transport: &mut SimTransport,
    controller: &mut SpaController,
) -> Vec<ControllerEvent> {
    let spa_bytes = spa.tick();
    transport.inject_from_spa(&spa_bytes);
    let mut buf = [0u8; 512];
    let n = transport.read(&mut buf).unwrap();
    controller.process_bytes(&buf[..n])
}

/// Helper to run a tick and also process any responses.
/// Writes RegistrationSend event bytes to the transport automatically.
fn run_tick_with_responses(
    spa: &mut SpaSim,
    transport: &mut SimTransport,
    controller: &mut SpaController,
) -> Vec<ControllerEvent> {
    let events = run_tick(spa, transport, controller);

    // Write any RegistrationSend bytes to the transport
    let mut reg_bytes = Vec::new();
    for event in &events {
        if let ControllerEvent::RegistrationSend { bytes } = event {
            reg_bytes.extend_from_slice(bytes);
        }
    }

    if !reg_bytes.is_empty() {
        transport.write(&reg_bytes).unwrap();
    }

    // Process any bytes the controller wrote back (including reg bytes above)
    let controller_bytes = transport.take_from_controller();
    if !controller_bytes.is_empty() {
        let responses = spa.process_incoming_bytes(&controller_bytes);
        if !responses.is_empty() {
            transport.inject_from_spa(&responses);
            let mut buf = [0u8; 512];
            let n = transport.read(&mut buf).unwrap();
            if n > 0 {
                let mut more_events = controller.process_bytes(&buf[..n]);
                // Write any additional reg sends (e.g. ack)
                let mut more_reg_bytes = Vec::new();
                for event in &more_events {
                    if let ControllerEvent::RegistrationSend { bytes } = event {
                        more_reg_bytes.extend_from_slice(bytes);
                    }
                }
                if !more_reg_bytes.is_empty() {
                    transport.write(&more_reg_bytes).unwrap();
                    let final_bytes = transport.take_from_controller();
                    if !final_bytes.is_empty() {
                        spa.process_incoming_bytes(&final_bytes);
                    }
                }
                events.into_iter().chain(more_events.drain(..)).collect()
            } else {
                events
            }
        } else {
            events
        }
    } else {
        events
    }
}

/// Send a command through the controller → transport → spa pipeline.
fn send_command(
    cmd: &Command,
    controller: &mut SpaController,
    transport: &mut SimTransport,
    spa: &mut SpaSim,
) {
    let encoded = controller.encode_command(cmd).expect("controller should be registered");
    transport.write(&encoded).unwrap();
    let bytes = transport.take_from_controller();
    spa.process_incoming_bytes(&bytes);
}

/// Complete the registration flow between spa and controller.
///
/// The registration protocol requires several round-trips where the controller
/// emits `RegistrationSend` events that must be written to the transport.
fn complete_registration(
    spa: &mut SpaSim,
    transport: &mut SimTransport,
    controller: &mut SpaController,
) {
    let max_rounds = 10;
    for _ in 0..max_rounds {
        // Generate spa output and feed to controller
        let spa_bytes = spa.tick();
        transport.inject_from_spa(&spa_bytes);

        let mut buf = [0u8; 512];
        let n = transport.read(&mut buf).unwrap();
        let mut events = Vec::new();
        if n > 0 {
            events = controller.process_bytes(&buf[..n]);
        }

        // Write any RegistrationSend bytes to the transport and process responses
        for event in &events {
            if let ControllerEvent::RegistrationSend { bytes } = event {
                transport.write(bytes).unwrap();
            }
        }

        let controller_bytes = transport.take_from_controller();
        if !controller_bytes.is_empty() {
            let responses = spa.process_incoming_bytes(&controller_bytes);
            if !responses.is_empty() {
                transport.inject_from_spa(&responses);
                let n = transport.read(&mut buf).unwrap();
                if n > 0 {
                    let more_events = controller.process_bytes(&buf[..n]);
                    // Write any additional RegistrationSend (the ID ack)
                    for event in &more_events {
                        if let ControllerEvent::RegistrationSend { bytes } = event {
                            transport.write(bytes).unwrap();
                        }
                    }
                    events.extend(more_events);
                }
            }
        }

        // Send any final ack to spa
        let final_bytes = transport.take_from_controller();
        if !final_bytes.is_empty() {
            spa.process_incoming_bytes(&final_bytes);
        }

        if controller.is_registered() {
            return;
        }
    }
    panic!("Registration did not complete within {} rounds", max_rounds);
}

// ============================================================================
// Test Group 1: Full Lifecycle
// ============================================================================

#[test]
fn test_full_spa_lifecycle() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();
    let mut broker = SimBroker::new("test_spa");

    // Start at steady state so temperature doesn't change during test
    spa.state.current_temp = 100.0;
    spa.state.set_temp = 100.0;
    spa.state.is_heating = false;

    // Phase 1: Registration
    complete_registration(&mut spa, &mut transport, &mut controller);
    assert_eq!(controller.client_id(), Some(0x02));

    // Phase 2: Receive status updates
    for _ in 0..5 {
        let events = run_tick(&mut spa, &mut transport, &mut controller);
        for event in &events {
            if let ControllerEvent::StatusUpdate(status) = event {
                broker.publish_state(status);
            }
        }
    }

    let state = broker.last_state().expect("should have state");
    let parsed: serde_json::Value = serde_json::from_str(state).unwrap();
    assert_eq!(parsed["current_temp"], 100.0);
    assert_eq!(parsed["set_temp"], 100.0);
    assert_eq!(parsed["is_heating"], "false");

    // Phase 3: Send a command
    send_command(
        &Command::ToggleItem(ToggleItem::Pump1),
        &mut controller, &mut transport, &mut spa,
    );
    assert_eq!(spa.state.pump1, PumpState::Low);

    // Phase 4: Verify pump shows in next status
    let events = run_tick(&mut spa, &mut transport, &mut controller);
    let status_event = events.iter().find_map(|e| match e {
        ControllerEvent::StatusUpdate(s) => Some(s.clone()),
        _ => None,
    }).expect("should have status");
    assert_eq!(status_event.pump1, PumpState::Low);
}

// ============================================================================
// Test Group 2: Registration
// ============================================================================

#[test]
fn test_registration_trace() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    // Round 1: spa sends registration query
    let spa_bytes = spa.tick();
    transport.inject_from_spa(&spa_bytes);
    let mut buf = [0u8; 512];
    let n = transport.read(&mut buf).unwrap();
    assert!(n > 0, "round 1: controller should read bytes");
    let events = controller.process_bytes(&buf[..n]);
    assert!(!controller.is_registered(), "round 1: not registered yet");

    // Find the RegistrationSend event and write its bytes to transport
    let id_request_bytes = events.iter().find_map(|e| match e {
        ControllerEvent::RegistrationSend { bytes } => Some(bytes.clone()),
        _ => None,
    }).expect("round 1: should have RegistrationSend");

    // Send controller's ID request to spa via transport
    transport.write(&id_request_bytes).unwrap();
    let controller_bytes = transport.take_from_controller();
    assert!(!controller_bytes.is_empty(), "controller should have written bytes");
    let responses = spa.process_incoming_bytes(&controller_bytes);
    assert!(!responses.is_empty(), "spa should respond with ID assignment");

    // Feed assignment back to controller
    transport.inject_from_spa(&responses);
    let n = transport.read(&mut buf).unwrap();
    assert!(n > 0, "should read assignment bytes");
    let events = controller.process_bytes(&buf[..n]);
    assert!(controller.is_registered(), "should be registered after assignment");
    let has_registered = events.iter().any(|e| matches!(e, ControllerEvent::Registered { .. }));
    assert!(has_registered, "should have Registered event");

    // Send ack to spa
    let ack_bytes = events.iter().find_map(|e| match e {
        ControllerEvent::RegistrationSend { bytes } => Some(bytes.clone()),
        _ => None,
    }).expect("should have ack RegistrationSend");
    transport.write(&ack_bytes).unwrap();
    let controller_bytes = transport.take_from_controller();
    spa.process_incoming_bytes(&controller_bytes);
    assert_eq!(spa.client_id, Some(0x02));
}

#[test]
fn test_registration_via_transport() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    assert!(controller.is_registered());
    assert_eq!(controller.client_id(), Some(0x02));
    assert_eq!(spa.client_id, Some(0x02));
}

#[test]
fn test_registration_generates_correct_events() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    // run_tick_with_responses handles the full registration in one call
    let events = run_tick_with_responses(&mut spa, &mut transport, &mut controller);
    let has_reg_send = events.iter().any(|e| matches!(e, ControllerEvent::RegistrationSend { .. }));
    assert!(has_reg_send, "should produce RegistrationSend");
    assert!(controller.is_registered());
    let has_registered = events.iter().any(|e| matches!(e, ControllerEvent::Registered { .. }));
    assert!(has_registered, "should complete registration");
}

#[test]
fn test_second_registration_gets_next_id() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller1 = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller1);
    assert_eq!(controller1.client_id(), Some(0x02));

    let mut controller2 = SpaController::new();
    controller2.force_registered(0x03);
    assert_eq!(controller2.client_id(), Some(0x03));
}

// ============================================================================
// Test Group 3: Continuous Status Stream
// ============================================================================

#[test]
fn test_60_seconds_of_status_updates() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();
    let mut broker = SimBroker::new("test_spa");

    complete_registration(&mut spa, &mut transport, &mut controller);

    let mut status_count = 0;
    for _ in 0..60 {
        let events = run_tick(&mut spa, &mut transport, &mut controller);
        for event in &events {
            if let ControllerEvent::StatusUpdate(status) = event {
                broker.publish_state(status);
                status_count += 1;
            }
        }
    }

    assert_eq!(status_count, 60);
    assert_eq!(broker.count_topic(&TopicBuilder::new("test_spa").state_topic()), 60);
}

#[test]
fn test_ready_events_on_each_tick() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    for _ in 0..5 {
        let events = run_tick(&mut spa, &mut transport, &mut controller);
        assert!(
            events.iter().any(|e| matches!(e, ControllerEvent::Ready)),
            "each tick after registration should produce a Ready event"
        );
    }
}

// ============================================================================
// Test Group 4: Command Round-Trip
// ============================================================================

#[test]
fn test_toggle_pump1_round_trip() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);
    run_tick(&mut spa, &mut transport, &mut controller);

    assert_eq!(spa.state.pump1, PumpState::Off);

    send_command(&Command::ToggleItem(ToggleItem::Pump1), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.pump1, PumpState::Low);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    let status = events.iter().find_map(|e| match e {
        ControllerEvent::StatusUpdate(s) => Some(s.clone()),
        _ => None,
    }).unwrap();
    assert_eq!(status.pump1, PumpState::Low);

    send_command(&Command::ToggleItem(ToggleItem::Pump1), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.pump1, PumpState::High);

    send_command(&Command::ToggleItem(ToggleItem::Pump1), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.pump1, PumpState::Off);
}

#[test]
fn test_toggle_light_round_trip() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);
    run_tick(&mut spa, &mut transport, &mut controller);

    assert!(!spa.state.light1);

    send_command(&Command::ToggleItem(ToggleItem::Light1), &mut controller, &mut transport, &mut spa);
    assert!(spa.state.light1);

    send_command(&Command::ToggleItem(ToggleItem::Light1), &mut controller, &mut transport, &mut spa);
    assert!(!spa.state.light1);
}

#[test]
fn test_toggle_blower_round_trip() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);
    run_tick(&mut spa, &mut transport, &mut controller);

    send_command(&Command::ToggleItem(ToggleItem::Blower), &mut controller, &mut transport, &mut spa);
    assert!(spa.state.blower);

    send_command(&Command::ToggleItem(ToggleItem::Blower), &mut controller, &mut transport, &mut spa);
    assert!(!spa.state.blower);
}

#[test]
fn test_toggle_heating_mode_round_trip() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);
    run_tick(&mut spa, &mut transport, &mut controller);

    assert_eq!(spa.state.heating_mode, HeatingMode::Ready);

    send_command(&Command::ToggleItem(ToggleItem::HeatingMode), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.heating_mode, HeatingMode::Rest);

    send_command(&Command::ToggleItem(ToggleItem::HeatingMode), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.heating_mode, HeatingMode::ReadyInRest);

    send_command(&Command::ToggleItem(ToggleItem::HeatingMode), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.heating_mode, HeatingMode::Ready);
}

#[test]
fn test_toggle_multiple_items_independently() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);
    run_tick(&mut spa, &mut transport, &mut controller);

    send_command(&Command::ToggleItem(ToggleItem::Pump1), &mut controller, &mut transport, &mut spa);
    send_command(&Command::ToggleItem(ToggleItem::Light1), &mut controller, &mut transport, &mut spa);
    send_command(&Command::ToggleItem(ToggleItem::Blower), &mut controller, &mut transport, &mut spa);

    assert_eq!(spa.state.pump1, PumpState::Low);
    assert!(spa.state.light1);
    assert!(spa.state.blower);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    let status = events.iter().find_map(|e| match e {
        ControllerEvent::StatusUpdate(s) => Some(s.clone()),
        _ => None,
    }).unwrap();
    assert_eq!(status.pump1, PumpState::Low);
    assert!(status.light1);
    assert!(status.blower);
}

// ============================================================================
// Test Group 5: Temperature
// ============================================================================

#[test]
fn test_set_temperature_round_trip() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);
    run_tick(&mut spa, &mut transport, &mut controller);

    assert_eq!(spa.state.set_temp, 104.0);

    send_command(&Command::SetTemperature(100), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.set_temp, 100.0);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    let status = events.iter().find_map(|e| match e {
        ControllerEvent::StatusUpdate(s) => Some(s.clone()),
        _ => None,
    }).unwrap();
    assert_eq!(status.set_temp, 100.0);
}

#[test]
fn test_heating_simulation() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();
    let mut broker = SimBroker::new("test_spa");

    spa.state.current_temp = 90.0;
    spa.state.set_temp = 100.0;
    spa.state.is_heating = true;

    complete_registration(&mut spa, &mut transport, &mut controller);

    for _ in 0..12 {
        let events = run_tick(&mut spa, &mut transport, &mut controller);
        for event in &events {
            if let ControllerEvent::StatusUpdate(status) = event {
                broker.publish_state(&status);
            }
        }
    }

    assert_eq!(spa.state.current_temp, 100.0);

    let state = broker.last_state().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(state).unwrap();
    assert_eq!(parsed["current_temp"], 100.0);
}

#[test]
fn test_cooling_simulation() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    spa.state.current_temp = 104.0;
    spa.state.set_temp = 100.0;
    spa.state.is_heating = false;

    complete_registration(&mut spa, &mut transport, &mut controller);

    for _ in 0..4 {
        run_tick(&mut spa, &mut transport, &mut controller);
    }

    assert_eq!(spa.state.current_temp, 100.0);
}

#[test]
fn test_celsius_mode() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();
    let mut broker = SimBroker::new("test_spa");

    spa.state.temp_scale = TemperatureScale::Celsius;
    spa.state.current_temp = 38.0;
    spa.state.set_temp = 38.0;
    spa.state.is_heating = false;

    complete_registration(&mut spa, &mut transport, &mut controller);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    for event in &events {
        if let ControllerEvent::StatusUpdate(status) = event {
            broker.publish_state(&status);
            assert_eq!(status.temperature_scale, TemperatureScale::Celsius);
            assert_eq!(status.current_temp, Some(38.0));
            assert_eq!(status.set_temp, 38.0);
        }
    }

    let state = broker.last_state().unwrap();
    assert!(state.contains("\"temp_scale\":\"celsius\""));
}

// ============================================================================
// Test Group 6: MQTT Command Parsing Pipeline
// ============================================================================

#[test]
fn test_mqtt_command_to_spa_via_controller() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    let cmd = command_parser::parse_command(
        "launa/test_spa/command",
        "launa/test_spa/command/pump1",
        b"true",
    ).expect("should parse");

    send_command(&cmd, &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.pump1, PumpState::Low);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    let status = events.iter().find_map(|e| match e {
        ControllerEvent::StatusUpdate(s) => Some(s.clone()),
        _ => None,
    }).unwrap();
    assert_eq!(status.pump1, PumpState::Low);
}

#[test]
fn test_mqtt_set_temperature_pipeline() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    let cmd = command_parser::parse_command(
        "launa/test_spa/command",
        "launa/test_spa/command/set_temperature",
        b"102",
    ).expect("should parse");

    send_command(&cmd, &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.set_temp, 102.0);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    let status = events.iter().find_map(|e| match e {
        ControllerEvent::StatusUpdate(s) => Some(s.clone()),
        _ => None,
    }).unwrap();
    assert_eq!(status.set_temp, 102.0);
}

// ============================================================================
// Test Group 7: HA Discovery
// ============================================================================

#[test]
fn test_ha_discovery_via_broker() {
    let mut broker = SimBroker::new("test_spa");

    broker.publish_discovery("test_spa");
    broker.publish_availability(true);

    let discoveries = broker.discovery_payloads();
    assert_eq!(discoveries.len(), 14, "should have 14 discovery configs");

    for payload in &discoveries {
        let _: serde_json::Value = serde_json::from_str(payload)
            .expect("discovery payload should be valid JSON");
    }

    let avail_topic = TopicBuilder::new("test_spa").availability_topic();
    assert_eq!(broker.count_topic(&avail_topic), 1);
}

#[test]
fn test_full_mqtt_pipeline_with_discovery() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    spa.state.current_temp = 100.0;
    spa.state.set_temp = 100.0;
    spa.state.is_heating = false;

    let mut controller = SpaController::new();
    let mut broker = SimBroker::new("my_spa");

    broker.publish_discovery("my_spa");
    broker.publish_availability(true);

    complete_registration(&mut spa, &mut transport, &mut controller);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    for event in &events {
        if let ControllerEvent::StatusUpdate(status) = event {
            broker.publish_state(status);
        }
    }

    assert_eq!(broker.discovery_payloads().len(), 14);
    assert!(broker.last_state().is_some());

    let state = broker.last_state().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(state).unwrap();
    assert_eq!(parsed["current_temp"], 100.0);
    assert_eq!(parsed["set_temp"], 100.0);
}

// ============================================================================
// Test Group 8: Resilience
// ============================================================================

#[test]
fn test_noise_bytes_ignored() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    transport.inject_from_spa(&[0x00, 0x01, 0x02, 0x03, 0xAA, 0xBB]);
    let mut buf = [0u8; 256];
    let n = transport.read(&mut buf).unwrap();
    let events = controller.process_bytes(&buf[..n]);

    assert!(events.is_empty());

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    assert!(events.iter().any(|e| matches!(e, ControllerEvent::StatusUpdate(_))));
}

#[test]
fn test_command_before_registration_ignored() {
    let controller = SpaController::new();

    let encoded = controller.encode_command(&Command::ToggleItem(ToggleItem::Pump1));
    assert!(encoded.is_none(), "should not encode commands before registration");
}

#[test]
fn test_empty_read_no_events() {
    let mut transport = SimTransport::new();
    let mut controller = SpaController::new();

    let mut buf = [0u8; 256];
    let n = transport.read(&mut buf).unwrap();
    assert_eq!(n, 0);

    let events = controller.process_bytes(&buf[..n]);
    assert!(events.is_empty());
}

// ============================================================================
// Test Group 9: Clock Simulation
// ============================================================================

#[test]
fn test_time_advances_each_tick() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    // Capture time after registration (it will have advanced during registration ticks)
    let start_minute = spa.state.minute;

    run_tick(&mut spa, &mut transport, &mut controller);
    assert_eq!(spa.state.minute, start_minute + 1);

    for _ in 0..29 {
        run_tick(&mut spa, &mut transport, &mut controller);
    }
    assert_eq!(spa.state.minute, (start_minute + 30) % 60);
}

#[test]
fn test_time_rolls_over_midnight() {
    let mut spa = SpaSim::new();
    spa.state.hour = 23;
    spa.state.minute = 59;

    spa.tick();
    assert_eq!(spa.state.hour, 0);
    assert_eq!(spa.state.minute, 0);
}

// ============================================================================
// Test Group 10: Pump Timer (Virtual Time)
// ============================================================================

#[test]
fn test_pump_timer_starts_and_tracks() {
    let mut controller = SpaController::new();

    controller.force_registered(0x05);

    assert!(!controller.is_pump_timer_running(ToggleItem::Pump1));

    controller.start_pump_timer(ToggleItem::Pump1);
    assert!(controller.is_pump_timer_running(ToggleItem::Pump1));

    controller.cancel_pump_timer(ToggleItem::Pump1);
    assert!(!controller.is_pump_timer_running(ToggleItem::Pump1));
}

#[test]
fn test_pump_timer_expiry() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    send_command(&Command::ToggleItem(ToggleItem::Pump1), &mut controller, &mut transport, &mut spa);

    controller.start_pump_timer(ToggleItem::Pump1);
    assert!(controller.is_pump_timer_running(ToggleItem::Pump1));

    for _ in 0..1199 {
        let events = run_tick(&mut spa, &mut transport, &mut controller);
        assert!(!events.iter().any(|e| matches!(e, ControllerEvent::PumpExpired(_))));
    }
    assert!(controller.is_pump_timer_running(ToggleItem::Pump1));

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    assert!(events.iter().any(|e| matches!(e, ControllerEvent::PumpExpired(_))));
    assert!(!controller.is_pump_timer_running(ToggleItem::Pump1));
}

// ============================================================================
// Test Group 11: SpaState Customization
// ============================================================================

#[test]
fn test_custom_spa_state() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    spa.state = SpaState {
        current_temp: 80.0,
        set_temp: 80.0,
        heating_mode: HeatingMode::Rest,
        temp_scale: TemperatureScale::Fahrenheit,
        is_heating: false,
        temp_range: TempRange::Low,
        pump1: PumpState::High,
        pump2: PumpState::Low,
        pump3: PumpState::Off,
        circ_pump: true,
        blower: true,
        light1: true,
        mister: true,
        hour: 22,
        minute: 0,
        priming: false,
        hold: true,
    };

    let mut controller = SpaController::new();
    let mut broker = SimBroker::new("custom_spa");

    complete_registration(&mut spa, &mut transport, &mut controller);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    for event in &events {
        if let ControllerEvent::StatusUpdate(status) = event {
            broker.publish_state(status);
        }
    }

    let state = broker.last_state().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(state).unwrap();
    assert_eq!(parsed["current_temp"], 80.0);
    assert_eq!(parsed["set_temp"], 80.0);
    assert_eq!(parsed["is_heating"], "false");
    assert_eq!(parsed["heating_mode"], "rest");
    assert_eq!(parsed["temp_range"], "low");
    assert_eq!(parsed["pump1_on"], true);
    assert_eq!(parsed["pump2_on"], true);
    assert_eq!(parsed["pump3_on"], false);
    assert_eq!(parsed["light1"], true);
    assert_eq!(parsed["blower"], true);
    assert_eq!(parsed["circ_pump"], true);
    assert_eq!(parsed["mister"], true);
    assert_eq!(parsed["hold_mode"], true);
}

// ============================================================================
// Test Group 12: Multi-Frame Streaming
// ============================================================================

#[test]
fn test_multiple_ticks_buffered_together() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    let mut all_bytes = Vec::new();
    for _ in 0..3 {
        all_bytes.extend_from_slice(&spa.tick());
    }
    transport.inject_from_spa(&all_bytes);

    let mut buf = [0u8; 1024];
    let n = transport.read(&mut buf).unwrap();
    let events = controller.process_bytes(&buf[..n]);

    let status_count = events.iter()
        .filter(|e| matches!(e, ControllerEvent::StatusUpdate(_)))
        .count();
    let ready_count = events.iter()
        .filter(|e| matches!(e, ControllerEvent::Ready))
        .count();

    assert_eq!(status_count, 3);
    assert_eq!(ready_count, 3);
}

#[test]
fn test_interleaved_commands_and_status() {
    let mut transport = SimTransport::new();
    let mut spa = SpaSim::new();
    let mut controller = SpaController::new();

    complete_registration(&mut spa, &mut transport, &mut controller);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    assert!(events.iter().any(|e| matches!(e, ControllerEvent::StatusUpdate(_))));

    send_command(&Command::ToggleItem(ToggleItem::Pump2), &mut controller, &mut transport, &mut spa);
    assert_eq!(spa.state.pump2, PumpState::Low);

    let events = run_tick(&mut spa, &mut transport, &mut controller);
    let status = events.iter().find_map(|e| match e {
        ControllerEvent::StatusUpdate(s) => Some(s.clone()),
        _ => None,
    }).unwrap();
    assert_eq!(status.pump2, PumpState::Low);
}
