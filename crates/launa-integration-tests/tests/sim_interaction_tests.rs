//! Simulator interaction tests.
//!
//! Tests that exercise SpaSim's thermal model and time progression:
//! tick-based time updates, heating behavior, and cooling behavior.

use launa_protocol::status::PumpState;
use launa_sim::SpaSim;

#[test]
fn test_simulator_tick_updates_time() {
    let mut sim = SpaSim::new();
    assert_eq!(sim.state.hour, 14);
    assert_eq!(sim.state.minute, 30);

    sim.tick();
    assert_eq!(sim.state.minute, 31);

    for _ in 0..29 {
        sim.tick();
    }
    assert_eq!(sim.state.minute, 0);
    assert_eq!(sim.state.hour, 15);
}

#[test]
fn test_simulator_tick_heating_approaches_set_temp() {
    let mut sim = SpaSim::new();
    sim.state.current_temp = 95.0;
    sim.state.set_temp = 100.0;
    sim.state.is_heating = true;
    sim.state.pumps[0] = PumpState::Low;

    for _ in 0..50 {
        sim.tick();
        if sim.state.current_temp >= 100.0 {
            break;
        }
    }
    assert!(
        sim.state.current_temp >= 100.0,
        "should reach set_temp after 50 ticks, got {}",
        sim.state.current_temp
    );
}
