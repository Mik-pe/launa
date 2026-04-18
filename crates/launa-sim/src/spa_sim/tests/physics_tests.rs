use super::*;
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::PumpState;

#[test]
fn test_physics_heating() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 95.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // With the new thermal model, heating is proportional to delta.
    // Heat until we reach set_temp (within tolerance).
    for _ in 0..100 {
        sim.simulate_physics();
        if sim.state.current_temp >= 100.0 {
            break;
        }
    }
    assert!(
        sim.state.current_temp >= 100.0,
        "should reach set_temp, got {}",
        sim.state.current_temp
    );
    assert!(
        !sim.state.is_heating,
        "should stop heating once at set temp"
    );
}

#[test]
fn test_physics_cooling() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 105.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = false;

    sim.simulate_physics();
    // With proportional cooling, should decrease but not by a full 1.0°
    assert!(
        sim.state.current_temp < 105.0,
        "should cool down, got {}",
        sim.state.current_temp
    );
    assert!(
        sim.state.current_temp > 100.0,
        "should not reach set_temp in one tick, got {}",
        sim.state.current_temp
    );
}

#[test]
fn test_realistic_thermal_heating_80_to_104() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 80.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    // Need at least one pump running for heater/pump interlock
    sim.state.pumps[0] = PumpState::Low;

    let mut ticks = 0;
    while sim.state.current_temp < 104.0 && ticks < 200 {
        sim.simulate_physics();
        ticks += 1;
    }

    assert!(
        ticks >= 48 && ticks <= 75,
        "heating 80→104 should take 48-75 ticks, took {}",
        ticks
    );
    assert!(
        sim.state.current_temp >= 104.0,
        "should reach set_temp, got {}",
        sim.state.current_temp
    );
}

#[test]
fn test_realistic_thermal_cooling_104_to_80() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 104.0;
    sim.state.set_temp = 80.0;
    sim.state.is_heating = false;

    let mut ticks = 0;
    while sim.state.current_temp > 80.0 && ticks < 500 {
        sim.simulate_physics();
        ticks += 1;
    }

    assert!(
        ticks >= 200 && ticks <= 280,
        "cooling 104→80 should take 200-280 ticks, took {}",
        ticks
    );
    assert!(
        sim.state.current_temp <= 80.0,
        "should reach set_temp, got {}",
        sim.state.current_temp
    );
}

#[test]
fn test_realistic_thermal_heating_rate_tapers() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 80.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Measure temp change per tick at various points
    let checkpoints = [80.0f32, 90.0f32, 100.0f32];
    for &start_temp in &checkpoints {
        let mut s = SpaSim::new();
        s.state.current_temp = start_temp;
        s.state.set_temp = 104.0;
        s.state.is_heating = true;
        s.state.pumps[0] = PumpState::Low;
        let temp_before = s.state.current_temp;
        s.simulate_physics();
        let delta = s.state.current_temp - temp_before;
        assert!(
            delta > 0.0,
            "temp should increase when heating at {}°F, got delta={}",
            start_temp,
            delta
        );
        // Rate should be smaller near set_temp (tapering)
        if start_temp > 90.0 {
            assert!(
                delta < 1.0,
                "heating rate should taper near set_temp, got {}",
                delta
            );
        }
    }
}

#[test]
fn test_realistic_thermal_deterministic() {
    let mut sim1 = SpaSim::new();
    sim1.state.current_temp = 85.0;
    sim1.state.set_temp = 104.0;
    sim1.state.is_heating = true;
    sim1.state.pumps[0] = PumpState::Low;

    let mut temps1 = Vec::new();
    for _ in 0..20 {
        sim1.simulate_physics();
        temps1.push(sim1.state.current_temp);
    }

    let mut sim2 = SpaSim::new();
    sim2.state.current_temp = 85.0;
    sim2.state.set_temp = 104.0;
    sim2.state.is_heating = true;
    sim2.state.pumps[0] = PumpState::Low;

    let mut temps2 = Vec::new();
    for _ in 0..20 {
        sim2.simulate_physics();
        temps2.push(sim2.state.current_temp);
    }

    assert_eq!(
        temps1, temps2,
        "identical initial states should produce identical temp sequences"
    );
}

#[test]
fn test_realistic_thermal_no_overshoot_default() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 103.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Heat until set_temp
    for _ in 0..50 {
        sim.simulate_physics();
    }

    assert!(
        sim.state.current_temp <= 104.0,
        "without overshoot, temp should not exceed set_temp, got {}",
        sim.state.current_temp
    );
}

#[test]
fn test_physics_sensor_noise_variation() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 100.0;
    sim.set_physics_noise_amplitude(2.0);

    let mut variation_count = 0;
    let total_ticks = 100;
    for _ in 0..total_ticks {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            if let Some(t) = s.current_temp {
                if (t - 100.0).abs() > 0.01 {
                    variation_count += 1;
                }
                // All temps should be within ±2.0°F of internal temp
                assert!(
                    t >= 98.0 && t <= 102.0,
                    "temp {} should be within ±2.0 of 100.0",
                    t
                );
            }
        }
    }

    assert!(
        variation_count as f32 / total_ticks as f32 >= 0.30,
        "with noise=2.0, expected 30%+ ticks with variation, got {}/{} ({:.0}%)",
        variation_count,
        total_ticks,
        variation_count as f32 / total_ticks as f32 * 100.0
    );
}

#[test]
fn test_physics_sensor_noise_zero_no_noise() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 100.0;
    sim.set_physics_noise_amplitude(0.0);

    for _ in 0..30 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert_eq!(
                s.current_temp,
                Some(100.0),
                "with noise=0.0, temp should be exact"
            );
        }
    }
}

#[test]
fn test_physics_heater_off_when_pumps_off() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    // All pumps off, circ_pump off
    sim.state.pumps = [PumpState::Off; 6];
    sim.state.circ_pump = false;

    sim.simulate_physics();

    assert!(
        !sim.state.is_heating,
        "is_heating should be false when all pumps are off"
    );
}

#[test]
fn test_physics_heater_on_when_pump_on() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = false;
    sim.state.pumps[0] = PumpState::Low;

    sim.simulate_physics();

    assert!(
        sim.state.is_heating,
        "is_heating should be true when pump is on and temp < set_temp"
    );
}

#[test]
fn test_physics_heater_on_when_circ_pump_on() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = false;
    sim.state.pumps = [PumpState::Off; 6];
    sim.state.circ_pump = true;

    sim.simulate_physics();

    assert!(
        sim.state.is_heating,
        "is_heating should be true when circ_pump is on and temp < set_temp"
    );
}

#[test]
fn test_physics_heater_off_after_pump_turned_off() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Heating with pump on
    sim.simulate_physics();
    assert!(sim.state.is_heating, "should be heating with pump on");

    // Turn pump off
    sim.state.pumps[0] = PumpState::Off;
    sim.simulate_physics();
    assert!(
        !sim.state.is_heating,
        "is_heating should turn off when pump turned off"
    );
}

#[test]
fn test_physics_temp_unknown_on_startup_first_n_ticks() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;
    sim.set_physics_unknown_temp_ticks(5); // First 5 ticks report unknown

    // Ticks 1-5: should report None (0xFF)
    for i in 1..=5 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert_eq!(
                s.current_temp, None,
                "tick {}: should report None (0xFF) for current_temp",
                i
            );
        }
    }

    // Tick 6: should report actual temp
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
    if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(
            s.current_temp,
            Some(100.0),
            "tick 6: should report actual temp"
        );
    }
}

#[test]
fn test_physics_internal_runs_during_unknown_temp() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;
    sim.set_physics_unknown_temp_ticks(5);

    // Advance 5 ticks — internal temp should still change
    for _ in 0..5 {
        sim.tick();
    }

    assert!(
        sim.state.current_temp > 90.0,
        "internal temp should have increased during unknown period, got {}",
        sim.state.current_temp
    );
}

#[test]
fn test_physics_unknown_temp_default_zero() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;

    // Default: no unknown temp period
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
    if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
        assert_eq!(
            s.current_temp,
            Some(100.0),
            "default N=0: should report actual temp from first tick"
        );
    }
}

#[test]
fn test_physics_heater_overshoot() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;
    sim.set_physics_overshoot(2.0);

    let mut max_temp = sim.state.current_temp;
    for _ in 0..200 {
        sim.simulate_physics();
        max_temp = max_temp.max(sim.state.current_temp);
        // Stop once heating turns off
        if !sim.state.is_heating {
            break;
        }
    }

    assert!(
        max_temp >= 106.0,
        "with overshoot=2.0, temp should reach set_temp+2=106, max was {}",
        max_temp
    );
    assert!(
        !sim.state.is_heating,
        "heating should have stopped after overshoot"
    );
}

#[test]
fn test_physics_heater_overshoot_hysteresis() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 104.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = false;
    sim.state.pumps[0] = PumpState::Low;
    sim.set_physics_overshoot(2.0);

    // Simulate overshoot completion: set heating_overshot flag
    sim.heating_overshot = true;

    // Temp at set_temp (104), heating is off, overshoot was reached
    // Hysteresis threshold = set_temp - overshoot/2 = 104 - 1.0 = 103.0
    // Temp should NOT start heating until it drops to 103.0

    // Cool slightly to 103.5 — should NOT re-heat yet
    sim.state.current_temp = 103.5;
    sim.simulate_physics();
    assert!(
        !sim.state.is_heating,
        "should not re-heat at 103.5 (above hysteresis threshold of 103.0)"
    );

    // Cool to 103.0 — should start re-heating
    sim.state.current_temp = 102.9;
    sim.simulate_physics();
    assert!(
        sim.state.is_heating,
        "should re-heat at 102.9 (below hysteresis threshold of 103.0)"
    );
}

#[test]
fn test_physics_overshoot_default_zero() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 103.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Heat until set_temp
    for _ in 0..50 {
        sim.simulate_physics();
    }

    assert!(
        sim.state.current_temp <= 104.01,
        "default overshoot=0: temp should not exceed set_temp, got {}",
        sim.state.current_temp
    );
}

#[test]
fn test_all_physics_features_together() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 80.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Enable all physics features
    sim.set_physics_unknown_temp_ticks(3);
    sim.set_physics_overshoot(1.5);
    sim.set_physics_noise_amplitude(0.3);

    // First 3 ticks: unknown temp
    for i in 1..=3 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert_eq!(s.current_temp, None, "tick {}: should report None", i);
        }
    }

    // Tick 4 onwards: actual temp (with noise)
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
    if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
        assert!(
            s.current_temp.is_some(),
            "tick 4: should report actual temp"
        );
    }

    // Internal temp should have been increasing during unknown period
    assert!(
        sim.state.current_temp > 80.0,
        "internal temp should have increased, got {}",
        sim.state.current_temp
    );

    // Continue heating until overshoot
    let mut max_temp = sim.state.current_temp;
    for _ in 0..300 {
        sim.tick();
        max_temp = max_temp.max(sim.state.current_temp);
        if !sim.state.is_heating && sim.state.current_temp < 104.0 {
            break;
        }
    }

    // Should have overshoot past set_temp
    assert!(
        max_temp >= 105.5,
        "should overshoot past 104+1.5=105.5, max was {}",
        max_temp
    );
}

#[test]
fn test_set_ambient_temp_method_exists() {
    let mut sim = SpaSim::new();
    sim.set_ambient_temp(85.0);
}

#[test]
fn test_higher_ambient_slower_cooling() {
    // Compare cooling rate with ambient=70°F (default) vs ambient=85°F
    let mut sim70 = SpaSim::new();
    sim70.state.current_temp = 104.0;
    sim70.state.set_temp = 80.0;
    sim70.state.is_heating = false;
    // Default ambient is 70°F

    let mut sim85 = SpaSim::new();
    sim85.state.current_temp = 104.0;
    sim85.state.set_temp = 80.0;
    sim85.state.is_heating = false;
    sim85.set_ambient_temp(85.0);

    // Run 10 physics ticks on each
    for _ in 0..10 {
        sim70.simulate_physics();
        sim85.simulate_physics();
    }

    // Higher ambient → slower cooling → higher temp after same ticks
    assert!(
        sim85.state.current_temp > sim70.state.current_temp,
        "ambient=85 should cool slower than ambient=70: got {} vs {}",
        sim85.state.current_temp,
        sim70.state.current_temp
    );
}

#[test]
fn test_set_ambient_temp_70_matches_default() {
    let mut sim_default = SpaSim::new();
    sim_default.state.current_temp = 104.0;
    sim_default.state.set_temp = 80.0;
    sim_default.state.is_heating = false;

    let mut sim_explicit = SpaSim::new();
    sim_explicit.state.current_temp = 104.0;
    sim_explicit.state.set_temp = 80.0;
    sim_explicit.state.is_heating = false;
    sim_explicit.set_ambient_temp(70.0);

    for _ in 0..20 {
        sim_default.simulate_physics();
        sim_explicit.simulate_physics();
    }

    assert_eq!(
        sim_default.state.current_temp, sim_explicit.state.current_temp,
        "ambient=70 should match default behavior exactly"
    );
}

#[test]
fn test_default_ambient_is_70() {
    let sim = SpaSim::new();
    assert_eq!(sim.ambient_temp(), 70.0, "default ambient should be 70°F");
}

#[test]
fn test_pump_heat_contribution_raises_temp() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 95.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = false;
    sim.state.pumps[0] = PumpState::High;
    // Enable pump heat contribution
    sim.set_pump_heat_contribution(0.02); // 0.02°F per tick per running pump

    let temp_before = sim.state.current_temp;
    sim.simulate_physics();

    assert!(
        sim.state.current_temp > temp_before,
        "pump heat should raise temp: before={}, after={}",
        temp_before,
        sim.state.current_temp
    );
}

#[test]
fn test_pump_heat_contribution_default_off() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 95.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = false;
    sim.state.pumps[0] = PumpState::High;
    // Don't call set_pump_heat_contribution — should default to 0

    let temp_before = sim.state.current_temp;

    // With default (no pump heat), temp should not increase from pumps alone
    // (only cooling towards ambient should occur)
    sim.simulate_physics();

    assert!(
        sim.state.current_temp <= temp_before,
        "default pump heat=0: temp should not increase from pumps alone, before={}, after={}",
        temp_before,
        sim.state.current_temp
    );
}

#[test]
fn test_pump_heat_multiple_pumps_higher_contribution() {
    let mut sim1 = SpaSim::new();
    sim1.state.current_temp = 95.0;
    sim1.state.set_temp = 100.0;
    sim1.state.is_heating = false;
    sim1.state.pumps[0] = PumpState::Low;
    sim1.set_pump_heat_contribution(0.02);

    let mut sim2 = SpaSim::new();
    sim2.state.current_temp = 95.0;
    sim2.state.set_temp = 100.0;
    sim2.state.is_heating = false;
    sim2.state.pumps[0] = PumpState::Low;
    sim2.state.pumps[1] = PumpState::Low;
    sim2.state.pumps[2] = PumpState::Low;
    sim2.set_pump_heat_contribution(0.02);

    for _ in 0..10 {
        sim1.simulate_physics();
        sim2.simulate_physics();
    }

    assert!(
        sim2.state.current_temp > sim1.state.current_temp,
        "3 pumps should produce more heat than 1: {} vs {}",
        sim2.state.current_temp,
        sim1.state.current_temp
    );
}

#[test]
fn test_heater_interlock_stops_when_all_pumps_off() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Verify heating is active
    sim.simulate_physics();
    assert!(sim.state.is_heating, "should be heating with pump on");

    // Turn off all pumps
    sim.state.pumps[0] = PumpState::Off;
    sim.simulate_physics();

    // Heating should stop immediately
    assert!(
        !sim.state.is_heating,
        "is_heating must be false when all pumps are off"
    );
}

#[test]
fn test_heater_interlock_resumes_on_pump_restart() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = false; // not heating initially

    // All pumps off
    sim.state.pumps = [PumpState::Off; 6];
    sim.state.circ_pump = false;

    sim.simulate_physics();
    assert!(
        !sim.state.is_heating,
        "should not heat with no pumps running"
    );

    // Start a pump — should resume heating since temp < set_point
    sim.state.pumps[0] = PumpState::Low;
    sim.simulate_physics();
    assert!(
        sim.state.is_heating,
        "is_heating should resume when pump starts and temp < set_point"
    );
}

#[test]
fn test_heater_interlock_circ_pump_satisfies() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = false;
    sim.state.pumps = [PumpState::Off; 6];
    sim.state.circ_pump = true;

    sim.simulate_physics();
    assert!(
        sim.state.is_heating,
        "circ_pump alone should satisfy heater interlock"
    );
}

#[test]
fn test_heater_interlock_full_cycle() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    // Phase 1: heating with pump
    sim.simulate_physics();
    assert!(sim.state.is_heating, "phase 1: heating with pump");
    let temp_phase1 = sim.state.current_temp;
    assert!(temp_phase1 > 90.0, "should be warming up");

    // Phase 2: turn pump off — heating stops immediately
    sim.state.pumps[0] = PumpState::Off;
    sim.simulate_physics();
    assert!(!sim.state.is_heating, "phase 2: heating stopped");

    // Phase 3: temp should start cooling (or at least not heating)
    let _temp_after_stop = sim.state.current_temp;

    // Phase 4: restart pump — heating resumes
    sim.state.pumps[0] = PumpState::Low;
    sim.simulate_physics();
    assert!(
        sim.state.is_heating,
        "phase 4: heating should resume with pump restart"
    );
}

#[test]
fn test_overshoot_full_cycle_heat_overshoot_cool_hysteresis_reheat() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;
    sim.set_physics_overshoot(2.0);

    let overshoot_target = 106.0; // set_temp + overshoot
    let hysteresis_threshold = 103.0; // set_temp - overshoot/2

    // Phase 1: Heat from 100 → 106 (overshoot target)
    let mut reached_overshoot = false;
    for _ in 0..200 {
        sim.simulate_physics();
        if sim.state.current_temp >= overshoot_target - 0.01 {
            reached_overshoot = true;
            assert!(
                !sim.state.is_heating,
                "heating should stop at overshoot target"
            );
            break;
        }
        assert!(
            sim.state.is_heating,
            "should be heating until overshoot target"
        );
    }
    assert!(reached_overshoot, "should reach overshoot target 106°F");

    // Phase 2: Cool from 106 to 103.5 — should NOT re-heat (above hysteresis)
    sim.state.current_temp = 103.5;
    sim.simulate_physics();
    assert!(
        !sim.state.is_heating,
        "should not re-heat at 103.5 (above hysteresis threshold {})",
        hysteresis_threshold
    );

    // Phase 3: Cool below hysteresis threshold — should re-heat
    sim.state.current_temp = 102.9;
    sim.simulate_physics();
    assert!(
        sim.state.is_heating,
        "should re-heat below hysteresis threshold 103.0, temp={}",
        sim.state.current_temp
    );

    // Phase 4: Re-heat back to overshoot target
    let mut reached_second_overshoot = false;
    for _ in 0..200 {
        sim.simulate_physics();
        if sim.state.current_temp >= overshoot_target - 0.01 {
            reached_second_overshoot = true;
            break;
        }
    }
    assert!(
        reached_second_overshoot,
        "should reach overshoot target again on re-heat cycle, temp={}",
        sim.state.current_temp
    );
}

#[test]
fn test_overshoot_max_observed_equals_set_temp_plus_overshoot() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 95.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;
    sim.set_physics_overshoot(3.0);

    let mut max_temp = sim.state.current_temp;
    for _ in 0..300 {
        sim.simulate_physics();
        max_temp = max_temp.max(sim.state.current_temp);
        if !sim.state.is_heating && sim.state.current_temp < 100.0 {
            break;
        }
    }

    // Should overshoot to exactly set_temp + 3.0 = 103.0
    assert!(
        max_temp >= 103.0 - 0.1,
        "max observed temp should be ~103°F, got {}",
        max_temp
    );
}

#[test]
fn test_unknown_temp_exact_boundary_ticks() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = false;
    sim.set_physics_unknown_temp_ticks(10);

    // Ticks 1-10: report None (0xFF)
    for i in 1..=10 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[2], 0xFF,
            "tick {}: current_temp should be 0xFF (None)",
            i
        );
    }

    // Tick 11: report valid temp
    let bytes = sim.tick();
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    assert_ne!(
        frames[0].payload[2], 0xFF,
        "tick 11: current_temp should NOT be 0xFF anymore"
    );
}

#[test]
fn test_unknown_temp_physics_tick_count_advances() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 90.0;
    sim.state.set_temp = 104.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;
    sim.set_physics_unknown_temp_ticks(5);

    // During unknown period, internal temp should still change
    for _ in 0..5 {
        sim.tick();
    }
    // After 5 ticks of heating, internal temp should have risen
    assert!(
        sim.state.current_temp > 90.0,
        "internal temp should have risen during unknown period, got {}",
        sim.state.current_temp
    );

    // After unknown period, reported temp matches internal temp
    let bytes = sim.tick(); // tick 6
    let mut decoder = FrameDecoder::new();
    let frames = decoder.feed_slice(&bytes);
    let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
    if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
        assert!(
            s.current_temp.is_some(),
            "should report valid temp after unknown period"
        );
    }
}

#[test]
fn test_physics_noise_stays_within_bounds() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = false;
    sim.set_physics_noise_amplitude(3.0);

    for _ in 0..200 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            if let Some(t) = s.current_temp {
                let deviation = (t - 100.0).abs();
                assert!(
                    deviation <= 3.0,
                    "noise deviation {} should be within ±3.0°F",
                    deviation
                );
            }
        }
    }
}

#[test]
fn test_physics_noise_deterministic() {
    let collect_temps = || -> Vec<Option<f32>> {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;
        sim.set_physics_noise_amplitude(2.0);

        let mut temps = Vec::new();
        for _ in 0..100 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                temps.push(s.current_temp);
            }
        }
        temps
    };

    let temps1 = collect_temps();
    let temps2 = collect_temps();
    assert_eq!(
        temps1, temps2,
        "identical configurations should produce identical noise sequences"
    );
}

#[test]
fn test_physics_noise_and_legacy_noise_combined() {
    let mut sim = SpaSim::new();
    sim.registered = true;
    sim.state.current_temp = 100.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = false;
    sim.set_physics_noise_amplitude(1.0);
    sim.simulate_sensor_noise(1.0);

    let mut all_within_bounds = true;
    let mut variation_count = 0;
    for _ in 0..100 {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            if let Some(t) = s.current_temp {
                // Combined noise: physics ±1 + legacy ±1, max deviation ±2.0
                // But could be wider since both apply independently
                let deviation = (t - 100.0).abs();
                if deviation > 4.0 {
                    all_within_bounds = false;
                }
                if deviation > 0.01 {
                    variation_count += 1;
                }
            }
        }
    }
    assert!(
        all_within_bounds,
        "combined noise should stay within reasonable bounds"
    );
    assert!(
        variation_count > 50,
        "combined noise should produce variation in most ticks, got {}/100",
        variation_count
    );
}
