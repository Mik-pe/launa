//! Physics simulation for the spa thermal model.
//!
//! Implements temperature changes, heater/pump interlock, hysteresis thermostat,
//! pump waste heat, sensor noise, and the unknown temperature startup period.
//!
//! All temperature comparisons use `.to_fahrenheit()` for a common scale,
//! eliminating °F/°C mismatch bugs.

use launa_protocol::status::PumpState;
use launa_protocol::Temperature;

use super::state::SpaState;
use crate::lcg::lcg_next;

/// Default hysteresis band in °F. The heater turns on when current_temp
/// drops below `set_temp - HYSTERESIS_F` and turns off when it reaches
/// `set_temp + HYSTERESIS_F`. This prevents rapid cycling.
const HYSTERESIS_F: f32 = 0.5;

/// Default heating rate in °F per tick. When heating, temperature rises by
/// this amount each tick (proportional to delta, tapering near target).
const DEFAULT_HEATING_RATE_F: f32 = 0.3;

/// Context needed to run one physics tick.
///
/// This struct bundles the configurable physics parameters so that
/// `simulate_physics` can be a standalone function rather than a method
/// with full `SpaSim` access.
pub(crate) struct PhysicsContext {
    /// Ambient temperature.
    pub ambient_temp: Temperature,
    /// Heat contribution per tick per running pump (always in Fahrenheit).
    pub pump_heat_contribution: f32,
    /// Counter for how many ticks have run since creation.
    pub physics_tick_count: u64,
    /// Heater overshoot amount (always in Fahrenheit).
    /// When > 0, heating continues past `set_temp` by this amount before stopping.
    /// Re-heating occurs when temp drops below `set_temp - overshoot/2`.
    /// Default: 0.0 (simple hysteresis with 0.5°F band).
    pub physics_overshoot: f32,
    /// Whether the heater has reached the overshoot ceiling and should stop.
    /// Re-heating only occurs when temp drops below `set_temp - overshoot/2`.
    pub heating_overshot: bool,
    /// PRNG state for physics-model sensor noise.
    pub physics_noise_rng: u64,
}

/// Simulate temperature changes and time progression.
///
/// Uses a simple on/off thermostat with hysteresis:
/// - **Heater ON** when: `current_temp < set_temp - hysteresis`
/// - **Heater OFF** when: `current_temp >= set_temp + hysteresis`
/// - **Hysteresis**: 0.5°F default (derived from `physics_overshoot/2` when overshoot is set)
/// - **Interlock**: `is_heating` is forced false whenever all pumps AND circ_pump are off.
///   Heating resumes automatically when any pump or circ_pump is running (if temp < set_point).
///
/// All temperature comparisons convert to Fahrenheit via `.to_fahrenheit()`,
/// so mixing Celsius and Fahrenheit `Temperature` values is safe.
pub(crate) fn simulate_physics(state: &mut SpaState, ctx: &mut PhysicsContext) {
    ctx.physics_tick_count += 1;

    // All comparisons in Fahrenheit for a common scale.
    let ambient_f = ctx.ambient_temp.to_fahrenheit();
    let set_temp_f = state.set_temp.to_fahrenheit();
    let current_f = state.current_temp.to_fahrenheit();
    let overshoot = ctx.physics_overshoot;

    // Hysteresis: if overshoot is configured, use overshoot/2; otherwise use default 0.5°F
    let hysteresis = if overshoot > 0.0 {
        overshoot / 2.0
    } else {
        HYSTERESIS_F
    };
    let upper_target = set_temp_f + hysteresis;
    let lower_target = set_temp_f - hysteresis;
    let overshoot_target = set_temp_f + overshoot;

    // If current_temp is above the new set_temp (e.g. user lowered target),
    // clear the overshoot flag so the heater doesn't re-engage during cooling.
    if ctx.heating_overshot && current_f > set_temp_f + 0.01 {
        ctx.heating_overshot = false;
    }

    // Check if any pump or circ_pump is running
    let any_pump_on = state.pumps.iter().any(|&p| p != PumpState::Off) || state.circ_pump;

    // Count running pumps for heat contribution
    let running_pump_count = state.pumps.iter().filter(|&&p| p != PumpState::Off).count()
        + if state.circ_pump { 1 } else { 0 };

    // Enforce interlock: no heating without pump or circ_pump
    if !any_pump_on {
        state.is_heating = false;
    }

    // If heating is on but current temp is already above the upper target
    // (e.g. user lowered set_temp while heater was running), stop heating.
    // When overshoot is configured, allow heating up to overshoot_target.
    if state.is_heating {
        let stop_threshold = if overshoot > 0.0 {
            overshoot_target
        } else {
            upper_target
        };
        if current_f > stop_threshold {
            state.is_heating = false;
        }
    }

    // Temperature physics (all in Fahrenheit)
    let mut current_f = current_f;

    if state.is_heating {
        // Heating: raise temperature toward overshoot_target
        let effective_target = if overshoot > 0.0 {
            overshoot_target
        } else {
            upper_target
        };
        let delta = (effective_target - current_f).max(0.0);
        if delta > 0.01 {
            let proportional_rate = DEFAULT_HEATING_RATE_F * (delta / 24.0);
            let base_rate = 0.2_f32.max(delta * 0.01);
            let rate = proportional_rate + base_rate;
            current_f = (current_f + rate).min(effective_target);
        }

        // Check if we've reached the target
        if current_f >= effective_target - 0.01 {
            // Only snap when we arrived from below (actively heated to target).
            if current_f <= effective_target + 0.01 {
                current_f = effective_target;
                if overshoot > 0.0 {
                    ctx.heating_overshot = true;
                }
            }
            state.is_heating = false;
        }
    } else if current_f > ambient_f {
        // Cooling: temperature drops toward ambient at a rate proportional to delta
        let delta = (current_f - ambient_f).max(0.0);
        if delta > 0.01 {
            let cooling_range = 10.0;
            let base_rate = 0.25;
            let rate = base_rate * (delta / cooling_range).max(0.1);
            current_f = (current_f - rate).max(ambient_f);
        }
    }

    // Pump waste heat contribution (applies even when not actively heating)
    if running_pump_count > 0 && ctx.pump_heat_contribution > 0.0 {
        let heat_gain = ctx.pump_heat_contribution * running_pump_count as f32;
        let upper_bound = overshoot_target.max(current_f + heat_gain);
        current_f = (current_f + heat_gain).min(upper_bound);
    }

    // Write back the updated temperature (converting from Fahrenheit to state's scale)
    state.current_temp = Temperature::fahrenheit(current_f).convert(state.temp_scale);

    // Heating control logic: decide whether to start heating
    if any_pump_on {
        if ctx.heating_overshot {
            // Overshoot mode: re-heat when temp drops below set_temp - overshoot/2
            if current_f < lower_target {
                state.is_heating = true;
                ctx.heating_overshot = false;
            }
        } else if !state.is_heating && current_f < lower_target {
            // Normal mode: start heating when temp drops below set_temp - hysteresis
            state.is_heating = true;
        }
    }

    // Advance clock
    state.minute += 1;
    if state.minute >= 60 {
        state.minute = 0;
        state.hour = (state.hour + 1) % 24;
    }
}

/// Generate a deterministic pseudo-random f32 in [-1.0, 1.0] using a PRNG state.
pub(crate) fn next_physics_noise_rand(rng: &mut u64) -> f32 {
    lcg_next(rng);
    (*rng as i64 as f64 / i64::MAX as f64) as f32
}

/// Generate a deterministic pseudo-random u64 using a PRNG state.
pub(crate) fn next_rand(rng: &mut u64) -> u64 {
    lcg_next(rng)
}
