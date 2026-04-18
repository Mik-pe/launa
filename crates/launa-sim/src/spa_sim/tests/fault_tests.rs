use super::*;
use launa_protocol::fault::FaultCode;
use launa_protocol::frame::{FrameDecoder, FrameEncoder};
use launa_protocol::status::PumpState;

#[test]
fn test_simulate_fault_state_sets_fault_flag() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // No fault initially
    let normal_bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let normal_frames = decoder.feed_slice(&normal_bytes);
    let normal_msg = launa_protocol::dispatcher::dispatch_frame(&normal_frames[0]);
    if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = normal_msg {
        assert!(!s.is_priming, "should not be in fault initially");
    }

    // Simulate fault
    sim.simulate_fault_state(FaultCode::HeaterDry);

    // Status frame should show fault (init_mode = 0x02 in payload offset 1)
    let fault_bytes = sim.generate_status_frame();
    let fault_frames = decoder.feed_slice(&fault_bytes);
    // The raw payload byte 1 should be 0x02
    assert_eq!(
        fault_frames[0].payload[1], 0x02,
        "init_mode should be 0x02 (fault) after simulate_fault_state"
    );
}

#[test]
fn test_simulate_fault_state_fault_log_carries_code() {
    let mut sim = SpaSim::new();
    sim.simulate_fault_state(FaultCode::LowFlow);

    let response = sim.generate_fault_log_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
            assert_eq!(
                entry.message_code,
                FaultCode::LowFlow,
                "fault log should carry the simulated fault code"
            );
        }
        other => panic!("Expected FaultLogResponse, got {:?}", other),
    }
}

#[test]
fn test_simulate_fault_state_different_codes() {
    let codes = [
        FaultCode::HeaterDry,
        FaultCode::LowFlow,
        FaultCode::WaterTooHot,
        FaultCode::SensorAFault,
        FaultCode::Unknown(99),
    ];

    for code in &codes {
        let mut sim = SpaSim::new();
        sim.simulate_fault_state(*code);

        let response = sim.generate_fault_log_response();
        let msg = super::dispatch_response(&response);

        if let launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) = msg {
            assert_eq!(
                entry.message_code, *code,
                "fault log should carry {:?}",
                code
            );
        } else {
            panic!("Expected FaultLogResponse for code {:?}", code);
        }
    }
}

#[test]
fn test_clear_fault_state_restores_init_mode() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Set fault
    sim.simulate_fault_state(FaultCode::HeaterDry);
    let fault_bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let fault_frames = decoder.feed_slice(&fault_bytes);
    assert_eq!(
        fault_frames[0].payload[1], 0x02,
        "init_mode should be 0x02 during fault"
    );

    // Clear fault
    sim.clear_fault_state();

    // After clearing, init_mode should be 0x00
    let clear_bytes = sim.generate_status_frame();
    let clear_frames = decoder.feed_slice(&clear_bytes);
    assert_eq!(
        clear_frames[0].payload[1], 0x00,
        "init_mode should be 0x00 after clear_fault_state"
    );
}

#[test]
fn test_clear_fault_state_subsequent_status_no_fault() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_fault_state(FaultCode::LowFlow);
    sim.tick(); // tick during fault

    sim.clear_fault_state();

    // Multiple subsequent ticks should all show no fault
    for _ in 0..5 {
        let status = super::dispatch_status(&mut sim);
        assert!(
            !status.is_priming,
            "status should not show fault after clearing"
        );
    }
}

#[test]
fn test_transient_fault_auto_clears_after_n_ticks() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Inject transient fault that auto-clears after 3 ticks
    sim.simulate_transient_fault(FaultCode::HeaterDry, 3);

    // First 3 ticks should show fault (init_mode = 0x02)
    for i in 1..=3 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x02,
            "tick {}: init_mode should be 0x02 (fault active)",
            i
        );
    }

    // Tick 4 onwards should show no fault (init_mode = 0x00)
    for i in 4..=6 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x00,
            "tick {}: init_mode should be 0x00 (fault cleared)",
            i
        );
    }
}

#[test]
fn test_transient_fault_zero_ticks_clears_immediately() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_transient_fault(FaultCode::FlowFailed, 0);

    // Should be cleared already on first tick
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[1], 0x00,
        "zero-tick transient should clear immediately"
    );
}

#[test]
fn test_transient_fault_one_tick() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_transient_fault(FaultCode::WaterTooHot, 1);

    // Tick 1: fault active
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[1], 0x02, "tick 1: fault should be active");

    // Tick 2: cleared
    let bytes = sim.tick();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[1], 0x00,
        "tick 2: fault should be cleared"
    );
}

#[test]
fn test_multi_entry_fault_log_distinct_entries() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Configure a multi-entry fault log
    sim.set_fault_log_entries(vec![
        FaultLogConfig {
            fault_count: 3,
            entry_number: 1,
            message_code: FaultCode::HeaterDry,
            days_ago: 2,
            hour: 14,
            minute: 30,
            flags: 0x04,
            set_temperature: 104,
            sensor_a_temp: 104,
            sensor_b_temp: 102,
        },
        FaultLogConfig {
            fault_count: 3,
            entry_number: 2,
            message_code: FaultCode::LowFlow,
            days_ago: 5,
            hour: 10,
            minute: 15,
            flags: 0x04,
            set_temperature: 100,
            sensor_a_temp: 100,
            sensor_b_temp: 98,
        },
        FaultLogConfig {
            fault_count: 3,
            entry_number: 3,
            message_code: FaultCode::WaterTooHot,
            days_ago: 10,
            hour: 8,
            minute: 0,
            flags: 0x04,
            set_temperature: 106,
            sensor_a_temp: 108,
            sensor_b_temp: 107,
        },
    ]);

    // Walk entries 1..3
    let codes = [
        FaultCode::HeaterDry,
        FaultCode::LowFlow,
        FaultCode::WaterTooHot,
    ];
    for (i, expected_code) in codes.iter().enumerate() {
        let entry_num = (i + 1) as u8;
        let response = sim.generate_fault_log_response_for_entry(entry_num);
        let msg = super::dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(
                    entry.message_code, *expected_code,
                    "entry {} should have code {:?}",
                    entry_num, expected_code
                );
                assert_eq!(
                    entry.entry_number, entry_num,
                    "entry should report entry_number = {}",
                    entry_num
                );
            }
            other => panic!(
                "Entry {}: Expected FaultLogResponse, got {:?}",
                entry_num, other
            ),
        }
    }
}

#[test]
fn test_fault_log_entry_zero_returns_sentinel() {
    let mut sim = SpaSim::new();

    sim.set_fault_log_entries(vec![FaultLogConfig {
        fault_count: 1,
        entry_number: 1,
        message_code: FaultCode::HeaterDry,
        days_ago: 1,
        hour: 12,
        minute: 0,
        flags: 0x04,
        set_temperature: 104,
        sensor_a_temp: 104,
        sensor_b_temp: 102,
    }]);

    let response = sim.generate_fault_log_response_for_entry(0);
    // Entry 0 should produce an empty/sentinel response (fault_count = 0 or entry_number = 0)
    let msg = super::dispatch_response(&response);
    match msg {
        launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
            assert_eq!(
                entry.fault_count, 0,
                "entry 0 should return sentinel with fault_count = 0"
            );
        }
        other => panic!("Expected FaultLogResponse for entry 0, got {:?}", other),
    }
}

#[test]
fn test_fault_log_past_end_returns_sentinel() {
    let mut sim = SpaSim::new();

    sim.set_fault_log_entries(vec![FaultLogConfig {
        fault_count: 1,
        entry_number: 1,
        message_code: FaultCode::HeaterDry,
        days_ago: 1,
        hour: 12,
        minute: 0,
        flags: 0x04,
        set_temperature: 104,
        sensor_a_temp: 104,
        sensor_b_temp: 102,
    }]);

    // Only 1 entry, so entry 2 is past-end
    let response = sim.generate_fault_log_response_for_entry(2);
    let msg = super::dispatch_response(&response);
    match msg {
        launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
            assert_eq!(
                entry.fault_count, 0,
                "past-end entry should return sentinel with fault_count = 0"
            );
        }
        other => panic!(
            "Expected FaultLogResponse for past-end entry, got {:?}",
            other
        ),
    }
}

#[test]
fn test_fault_preserves_queued_commands() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.pumps[0] = PumpState::Off;
    sim.set_command_latency_ticks(3);

    // Queue a toggle pump1 command
    let toggle_cmd = FrameEncoder::encode([0x0A, 0xBF], &[0x11, 0x04]).unwrap();
    sim.process_frame(&FrameDecoder::new().feed_slice(&toggle_cmd).remove(0));

    // Command should be pending (3 ticks latency)
    assert_eq!(sim.pending_commands.len(), 1, "command should be queued");

    // Inject fault mid-command
    sim.simulate_fault_state(FaultCode::HeaterDry);

    // The queued command should NOT be lost
    assert_eq!(
        sim.pending_commands.len(),
        1,
        "fault should not discard queued commands"
    );

    // Process ticks: the pending command should still decrement and fire
    sim.tick(); // latency 3→2
    assert_eq!(sim.pending_commands.len(), 1);
    sim.tick(); // latency 2→1
    assert_eq!(sim.pending_commands.len(), 1);
    sim.tick(); // latency 1→0, command fires

    assert_eq!(
        sim.pending_commands.len(),
        0,
        "command should have been applied"
    );
    // The command should have applied despite fault
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "pump should be toggled on despite fault"
    );
}

#[test]
fn test_command_before_fault_executes_after_clear() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.pumps[0] = PumpState::Off;
    sim.set_command_latency_ticks(2);

    // Queue a command
    let toggle_cmd = FrameEncoder::encode([0x0A, 0xBF], &[0x11, 0x04]).unwrap();
    sim.process_frame(&FrameDecoder::new().feed_slice(&toggle_cmd).remove(0));

    // Inject fault
    sim.simulate_fault_state(FaultCode::LowFlow);

    sim.tick(); // latency 2→1
    sim.tick(); // latency 1→0, command fires

    // Command should have applied
    assert_eq!(sim.state.pumps[0], PumpState::Low);

    // Clear fault
    sim.clear_fault_state();

    // Status should now show no fault and pump running
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[1], 0x00, "init_mode should be 0x00");
}

#[test]
fn test_fault_overrides_priming_mode() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    sim.simulate_priming_mode(10);
    let bytes = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[1], 0x01, "should be priming first");

    // Fault overrides priming
    sim.simulate_fault_state(FaultCode::HeaterDry);
    let bytes = sim.generate_status_frame();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[1], 0x02,
        "fault should override priming mode"
    );

    // After clearing fault, priming should resume
    sim.clear_fault_state();
    let bytes = sim.generate_status_frame();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[1], 0x01,
        "priming should resume after fault cleared"
    );
}

#[test]
fn test_fault_lifecycle_defaults_off() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // No fault, no priming by default
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(
        frames[0].payload[1], 0x00,
        "init_mode should be 0x00 by default"
    );

    // Second tick also normal
    let bytes = sim.tick();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[1], 0x00, "should remain 0x00");
}

#[test]
fn test_transient_fault_with_command_latency() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.pumps[0] = PumpState::Off;
    sim.set_command_latency_ticks(2);

    // Queue command
    let toggle_cmd = FrameEncoder::encode([0x0A, 0xBF], &[0x11, 0x04]).unwrap();
    sim.process_frame(&FrameDecoder::new().feed_slice(&toggle_cmd).remove(0));

    // Inject transient fault for 2 ticks
    sim.simulate_transient_fault(FaultCode::HeaterDry, 2);

    // Tick 1: fault active, command pending (latency 2→1)
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[1], 0x02, "tick 1: fault active");

    // Tick 2: fault active, command fires (latency 1→0)
    let bytes = sim.tick();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[1], 0x02, "tick 2: fault still active");
    assert_eq!(
        sim.state.pumps[0],
        PumpState::Low,
        "command should have applied"
    );

    // Tick 3: fault cleared
    let bytes = sim.tick();
    let frames = decoder.feed_slice(&bytes);
    assert_eq!(frames[0].payload[1], 0x00, "tick 3: fault cleared");
}
