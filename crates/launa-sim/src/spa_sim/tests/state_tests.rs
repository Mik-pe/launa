use super::*;
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::PumpState;
use launa_protocol::Temperature;

#[test]
fn test_simulate_spa_reboot_resets_registration() {
    let mut sim = SpaSim::new();

    // First, register a client
    sim.registered = true;
    sim.client_id = Some(0x05);

    // Reboot
    sim.simulate_spa_reboot();

    // Registration should be reset
    assert!(!sim.registered, "should be unregistered after reboot");
    assert!(
        sim.client_id.is_none(),
        "client_id should be cleared after reboot"
    );

    // Next tick should produce a registration query (FE BF 00)
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);

    // Should contain at least one frame with a registration query
    let has_reg_query = frames
        .iter()
        .any(|f| f.message_type == [0xFE, 0xBF] && f.payload.contains(&0x00));
    assert!(
        has_reg_query,
        "tick after reboot should produce a registration query"
    );
}

#[test]
fn test_simulate_spa_reboot_preserves_physical_state() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = Temperature::fahrenheit(98.0);
    sim.state.set_temp = Temperature::fahrenheit(102.0);
    sim.state.pumps[0] = PumpState::Low;
    sim.state.lights[0] = true;

    sim.simulate_spa_reboot();

    // Physical state should be preserved
    assert_eq!(sim.state.current_temp, Temperature::fahrenheit(98.0));
    assert_eq!(sim.state.set_temp, Temperature::fahrenheit(102.0));
    assert_eq!(sim.state.pumps[0], PumpState::Low);
    assert!(sim.state.lights[0]);
}

#[test]
fn test_simulate_spa_reboot_reregistration() {
    let mut sim = SpaSim::new();

    // Register, then reboot
    sim.registered = true;
    sim.client_id = Some(0x05);
    sim.simulate_spa_reboot();

    // Should be able to re-register via process_frame
    let _ack_frame = launa_protocol::frame::Frame {
        message_type: [0xFE, 0xBF],
        payload: vec![0x02, 0x03],
    };
    // First send the ID request
    let request_frame = launa_protocol::frame::Frame {
        message_type: [0xFE, 0xBF],
        payload: vec![0x01],
    };
    let assignment = sim.process_frame(&request_frame);
    assert!(assignment.is_some(), "should assign client ID");

    // Feed the assignment back to register
    let mut decoder = FrameDecoder::new();
    let assignment_frames = decoder.feed_slice(&assignment.unwrap());
    // The assignment frame is FE BF 02 <id>
    let id_frame = &assignment_frames[0];
    let msg = launa_protocol::dispatcher::dispatch_frame(id_frame);
    let launa_protocol::dispatcher::IncomingMessage::ClientIdAssignment { id } = msg else {
        panic!("Expected ClientIdAssignment, got {:?}", msg);
    };
    // Send ack
    let ack = launa_protocol::frame::FrameEncoder::encode([id, 0xBF], &[0x03]).unwrap();
    let ack_frames = decoder.feed_slice(&ack);
    sim.process_frame(&ack_frames[0]);

    assert!(sim.registered, "should be registered after re-registration");
    assert!(sim.client_id.is_some());
}

#[test]
fn test_spontaneous_filter_cycle_start() {
    let mut sim = SpaSim::new();
    assert_eq!(sim.state.pumps[0], PumpState::Off);

    // Schedule pump1 to turn on at tick 5
    sim.simulate_filter_cycle_start(0, 5);

    // Ticks 1-4: pump still off
    for _ in 0..4 {
        sim.tick();
    }
    // After tick 4, tick_count is 4
    let _ = sim.tick(); // tick 5: tick_count becomes 5, events at tick<=5 fire

    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "pump should start from scheduled event"
    );
}

#[test]
fn test_spontaneous_event_does_not_double_toggle() {
    let mut sim = SpaSim::new();
    // If pump is already on, filter cycle start should not change it
    sim.state.pumps[1] = PumpState::High;
    sim.simulate_filter_cycle_start(1, 1);

    sim.tick();
    // Should still be High, not cycled
    assert_eq!(sim.state.pumps[1], PumpState::High);
}

#[test]
fn test_simulate_spontaneous_state_change_via_schedule_event() {
    let mut sim = SpaSim::new();
    assert_eq!(sim.state.pumps[0], PumpState::Off);

    // Schedule pump1 to turn on at tick 3
    sim.schedule_event(3, SpaEventType::FilterCycleStart { pump_index: 0 });

    // Ticks 1-2: pump still off
    sim.tick();
    sim.tick();
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Off,
        "pump should be off before event"
    );

    // Tick 3: event fires
    sim.tick();
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "pump should start from scheduled event"
    );
}

#[test]
fn test_simulate_spontaneous_state_change_filter_cycle() {
    let mut sim = SpaSim::new();
    sim.simulate_filter_cycle_start(2, 5); // pump 3 at tick 5

    for _ in 0..4 {
        sim.tick();
    }
    assert_eq!(sim.state.pumps[2], PumpState::Off);

    sim.tick(); // tick 5
    assert_eq!(
        sim.state.pumps[2],
        PumpState::Low,
        "pump 3 should start from filter cycle"
    );
}

#[test]
fn test_simulate_unknown_temp_reports_none() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = Temperature::fahrenheit(100.0); // Internal temp is known

    // Before: temp is known
    let normal_bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let normal_frames = decoder.feed_slice(&normal_bytes);
    let normal_msg = launa_protocol::dispatcher::dispatch_frame(&normal_frames[0]);
    let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = normal_msg else {
        panic!("Expected StatusUpdate, got {:?}", normal_msg);
    };
    assert!(s.current_temp.is_some(), "should have temp before unknown");

    // Enable unknown temp
    sim.simulate_unknown_temp();

    let unknown_bytes = sim.generate_status_frame();
    let unknown_frames = decoder.feed_slice(&unknown_bytes);
    let unknown_msg = launa_protocol::dispatcher::dispatch_frame(&unknown_frames[0]);
    let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = unknown_msg else {
        panic!("Expected StatusUpdate, got {:?}", unknown_msg);
    };
    assert_eq!(
        s.current_temp, None,
        "current_temp should be None after simulate_unknown_temp"
    );

    // Internal state still has the temp
    assert_eq!(
        sim.state.current_temp,
        Temperature::fahrenheit(100.0),
        "internal state should still have the real temp"
    );
}

#[test]
fn test_simulate_unknown_temp_clear_restores() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = Temperature::fahrenheit(100.0);

    sim.simulate_unknown_temp();
    sim.clear_unknown_temp();

    let bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
    let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg else {
        panic!("Expected StatusUpdate, got {:?}", msg);
    };
    assert_eq!(
        s.current_temp,
        Some(Temperature::fahrenheit(100.0)),
        "temp should be restored after clear_unknown_temp"
    );
}

#[test]
fn test_simulate_sensor_noise_with_jitter() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = Temperature::fahrenheit(100.0);
    sim.state.set_temp = Temperature::fahrenheit(100.0);
    sim.simulate_sensor_noise(2.0);

    // Collect temps from 100 ticks
    let mut temps: Vec<f32> = Vec::new();
    for _ in 0..100 {
        let bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg else {
            panic!("Expected StatusUpdate, got {:?}", msg);
        };
        if let Some(t) = s.current_temp {
            temps.push(t.to_fahrenheit());
        }
    }

    // All temps should be within ±2.0 of baseline (100.0)
    for &t in &temps {
        assert!(
            t >= 98.0 && t <= 102.0,
            "temp {} should be within ±2.0 of 100.0",
            t
        );
    }

    // With jitter=2.0, not all temps should be exactly 100.0
    let exact_count = temps.iter().filter(|&&t| t == 100.0).count();
    assert!(
        exact_count < temps.len(),
        "with jitter=2.0, not all temps should be exactly 100.0 (got {}/{})",
        exact_count,
        temps.len()
    );
}

#[test]
fn test_simulate_sensor_noise_zero_jitter() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = Temperature::fahrenheit(100.0);
    sim.state.set_temp = Temperature::fahrenheit(100.0);
    sim.simulate_sensor_noise(0.0); // No noise

    for _ in 0..20 {
        let bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg else {
            panic!("Expected StatusUpdate, got {:?}", msg);
        };
        assert_eq!(
            s.current_temp,
            Some(Temperature::fahrenheit(100.0)),
            "with jitter=0.0, temp should be exact"
        );
    }
}

#[test]
fn test_simulate_sensor_noise_deterministic() {
    let mut sim1 = SpaSim::new();
    sim1.registered = true;
    sim1.state.current_temp = Temperature::fahrenheit(100.0);
    sim1.state.set_temp = Temperature::fahrenheit(100.0);
    sim1.simulate_sensor_noise(1.5);

    let mut temps1: Vec<f32> = Vec::new();
    for _ in 0..50 {
        let bytes = sim1.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg else {
            panic!("Expected StatusUpdate, got {:?}", msg);
        };
        if let Some(t) = s.current_temp {
            temps1.push(t.to_fahrenheit());
        }
    }

    // Create identical sim
    let mut sim2 = SpaSim::new();
    sim2.registered = true;
    sim2.state.current_temp = Temperature::fahrenheit(100.0);
    sim2.state.set_temp = Temperature::fahrenheit(100.0);
    sim2.simulate_sensor_noise(1.5);

    let mut temps2: Vec<f32> = Vec::new();
    for _ in 0..50 {
        let bytes = sim2.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg else {
            panic!("Expected StatusUpdate, got {:?}", msg);
        };
        if let Some(t) = s.current_temp {
            temps2.push(t.to_fahrenheit());
        }
    }

    // Same initial state → same sequence
    assert_eq!(
        temps1, temps2,
        "identical sims should produce identical noise sequences"
    );
}

#[test]
fn test_simulate_priming_mode_sets_init_mode() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_priming_mode(10);

    let bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[2], 0x01,
        "init_mode should be 0x01 (priming) after simulate_priming_mode"
    );
}

#[test]
fn test_priming_mode_auto_exits_after_duration() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_priming_mode(5);

    // First 5 ticks should show priming
    for i in 1..=5 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[2], 0x01,
            "tick {}: init_mode should be 0x01 (priming)",
            i
        );
    }

    // Tick 6 onwards should show normal
    for i in 6..=8 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[2], 0x00,
            "tick {}: init_mode should be 0x00 (priming exited)",
            i
        );
    }
}

#[test]
fn test_priming_mode_zero_duration_exits_immediately() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_priming_mode(0);

    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[2], 0x00,
        "zero-duration priming should exit immediately"
    );
}

#[test]
fn test_clear_priming_mode_manual_exit() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_priming_mode(100);

    // Should show priming
    let bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[2], 0x01, "should be in priming mode");

    // Manually clear
    sim.clear_priming_mode();

    // Should show normal
    let bytes = sim.generate_status_frame();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[2], 0x00,
        "priming should be cleared manually"
    );
}

#[test]
fn test_spa_reboot_preserves_physics_state_after_running() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = Temperature::fahrenheit(80.0);
    sim.state.set_temp = Temperature::fahrenheit(104.0);
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;
    sim.registered = true;
    sim.client_id = Some(0x05);

    // Run 30 ticks to heat up
    for _ in 0..30 {
        sim.tick();
    }

    let temp_before_reboot = sim.state.current_temp;
    let pump_before = sim.state.pumps[0];
    let light_before = sim.state.lights[0];
    assert!(
        temp_before_reboot > Temperature::fahrenheit(80.0),
        "should have heated up before reboot"
    );

    // Reboot
    sim.simulate_spa_reboot();

    // Registration should be reset
    assert!(!sim.registered, "should be unregistered after reboot");
    assert!(
        sim.client_id.is_none(),
        "client_id should be cleared after reboot"
    );

    // Physical state preserved
    assert_eq!(
        sim.state.current_temp, temp_before_reboot,
        "temperature should survive reboot"
    );
    assert_eq!(
        sim.state.pumps[0], pump_before,
        "pump state should survive reboot"
    );
    assert_eq!(
        sim.state.lights[0], light_before,
        "light state should survive reboot"
    );

    // Physics should continue running after reboot
    let temp_after_tick = {
        sim.tick();
        sim.state.current_temp
    };
    assert_ne!(
        temp_after_tick, temp_before_reboot,
        "physics should continue after reboot"
    );
}

#[test]
fn test_filter_cycle_start_turns_pump_on() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    assert_eq!(sim.state.pumps[0], PumpState::Off);

    // Schedule filter cycle start at tick 3
    sim.simulate_filter_cycle_start(0, 3);

    sim.tick(); // tick 1
    assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 1: still off");
    sim.tick(); // tick 2
    assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 2: still off");
    sim.tick(); // tick 3: event fires
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "tick 3: pump should turn on from filter cycle"
    );
}

#[test]
fn test_multiple_filter_cycles_different_pumps() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Start pump1 at tick 3, pump2 at tick 7
    sim.simulate_filter_cycle_start(0, 3);
    sim.simulate_filter_cycle_start(1, 7);

    // Tick through
    for _ in 0..2 {
        sim.tick();
    }
    assert_eq!(sim.state.pumps[0], PumpState::Off, "pump1 off before event");
    assert_eq!(sim.state.pumps[1], PumpState::Off, "pump2 off before event");

    sim.tick(); // tick 3: pump1 starts
    assert_eq!(sim.state.pumps[0], PumpState::Low, "pump1 on at tick 3");
    assert_eq!(sim.state.pumps[1], PumpState::Off, "pump2 still off");

    for _ in 0..3 {
        sim.tick();
    }
    sim.tick(); // tick 7: pump2 starts
    assert_eq!(sim.state.pumps[1], PumpState::Low, "pump2 on at tick 7");
}

#[test]
fn test_filter_cycle_pump_state_in_status_frame() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Schedule pump1 to start at tick 2
    sim.simulate_filter_cycle_start(0, 2);

    // Tick 1: pump off, status should reflect that
    let bytes1 = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames1 = decoder.feed_slice(&bytes1);
    let msg1 = launa_protocol::dispatcher::dispatch_frame(&frames1[0]);
    let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg1 else {
        panic!("Expected StatusUpdate, got {:?}", msg1);
    };
    assert_eq!(s.pumps[0], PumpState::Off, "tick 1: pump off in status");

    // Tick 2: event fires, pump starts
    let bytes2 = sim.tick();
    let mut decoder2 = FrameDecoder::new();
    let frames2 = decoder2.feed_slice(&bytes2);
    let msg2 = launa_protocol::dispatcher::dispatch_frame(&frames2[0]);
    let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg2 else {
        panic!("Expected StatusUpdate, got {:?}", msg2);
    };
    assert_eq!(
        s.pumps[0],
        PumpState::Low,
        "tick 2: pump on in status after filter cycle"
    );
}

#[test]
fn test_filter_cycle_stop_manual_toggle_off() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.set_command_success_rate(1.0);

    // Start pump via filter cycle
    sim.simulate_filter_cycle_start(0, 1);
    sim.tick(); // tick 1: event fires, pump = Low
    assert_eq!(sim.state.pumps[0], PumpState::Low);

    // Manually toggle pump off (simulating filter cycle end)
    let (mt, payload) =
        launa_protocol::command::Command::ToggleItem(launa_protocol::command::ToggleItem::Pump1)
            .encode().unwrap();
    let encoded = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();
    sim.process_incoming_bytes(&encoded);

    // Pump should cycle Low → High (not Off!)
    // Pump cycle: Off → Low → High → Off
    // Current: Low, toggle → High
    assert_eq!(
        sim.state.pumps[0],
        PumpState::High,
        "toggle from Low goes to High"
    );

    // Toggle again → Off
    sim.process_incoming_bytes(&encoded);
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Off,
        "second toggle goes to Off (filter cycle stopped)"
    );
}
