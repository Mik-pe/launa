//! Physics simulation for the spa thermal model.
//!
//! Implements temperature changes, heater/pump interlock, overshoot/hysteresis,
//! pump waste heat, sensor noise, and the unknown temperature startup period.

use launa_protocol::status::PumpState;

use super::state::SpaState;

/// Context needed to run one physics tick.
///
/// This struct bundles the configurable physics parameters so that
/// `simulate_physics` can be a standalone function rather than a method
/// with full `SpaSim` access.
pub(crate) struct PhysicsContext {
    /// Ambient temperature in °F.
    pub ambient_temp: f32,
    /// Heat contribution per tick per running pump (in °F).
    pub pump_heat_contribution: f32,
    /// Counter for how many ticks have run since creation.
    pub physics_tick_count: u64,
    /// Heater overshoot amount in °F.
    pub physics_overshoot: f32,
    /// Whether the heater has reached the overshoot ceiling.
    pub heating_overshot: bool,
    /// PRNG state for physics-model sensor noise.
    pub physics_noise_rng: u64,
}

/// Simulate temperature changes and time progression.
///
/// Uses a realistic thermal model:
/// - **Heating**: Rate proportional to `(set_temp + overshoot - current_temp) / delta_range`.
///   Base rate ~0.5°F/tick at full delta, tapering as temp approaches target.
///   Heating only active when at least one pump or circ_pump is running.
/// - **Cooling**: Rate proportional to `(current_temp - ambient_temp) / cooling_range`.
///   Base rate ~0.1°F/tick when well above ambient, tapering near ambient.
/// - **Pump heat**: Running pumps contribute waste heat (configurable via
///   `set_pump_heat_contribution()`). Each running pump adds the configured amount
///   per tick. Default: 0.0 (off).
/// - **Overshoot**: If `physics_overshoot > 0`, heating continues past `set_temp`
///   by the overshoot amount before stopping. Re-heats at `set_temp - overshoot/2`.
/// - **Interlock**: `is_heating` is forced false whenever all pumps and circ_pump
///   are off. Heating resumes automatically when a pump restarts (if temp < set_point).
///
/// Assumptions: ambient temperature defaults to 70°F (configurable via `set_ambient_temp()`).
pub(crate) fn simulate_physics(state: &mut SpaState, ctx: &mut PhysicsContext) {
    ctx.physics_tick_count += 1;

    let ambient_temp = ctx.ambient_temp;
    let set_temp = state.set_temp;
    let overshoot = ctx.physics_overshoot;
    let overshoot_target = set_temp + overshoot;
    let hysteresis = overshoot / 2.0;

    // Check if any pump or circ_pump is running
    let any_pump_on = state.pumps.iter().any(|&p| p != PumpState::Off) || state.circ_pump;

    // Count running pumps for heat contribution
    let running_pump_count = state.pumps.iter().filter(|&&p| p != PumpState::Off).count()
        + if state.circ_pump { 1 } else { 0 };

    // Enforce interlock: no heating without pump
    if !any_pump_on {
        state.is_heating = false;
    }

    // Temperature physics
    if state.is_heating {
        // Heating: base rate ~0.5°F/tick when delta is large, tapering to ~0.3 when close.
        // Uses a combination of base rate and proportional rate for realistic behavior.
        let delta = (overshoot_target - state.current_temp).max(0.0);
        if delta > 0.01 {
            // Base rate ensures minimum heating speed even when close to target.
            // Proportional component adds speed when far from target.
            let proportional_rate = 0.3 * (delta / 24.0);
            let base_rate = 0.2_f32.max(delta * 0.01);
            let rate = proportional_rate + base_rate;
            let new_temp = state.current_temp + rate;
            state.current_temp = new_temp.min(overshoot_target);
        }

        // Check if we've reached the overshoot target
        if state.current_temp >= overshoot_target - 0.01 {
            state.current_temp = overshoot_target;
            state.is_heating = false;
            ctx.heating_overshot = true;
        }
    } else if state.current_temp > ambient_temp.max(set_temp) {
        // Cooling: rate proportional to delta from ambient
        let effective_min = if set_temp > ambient_temp {
            set_temp
        } else {
            ambient_temp
        };
        let delta = (state.current_temp - ambient_temp).max(0.0);
        if delta > 0.01 {
            let cooling_range = 10.0; // Tuned for faster cooling
            let base_rate = 0.25;
            let rate = base_rate * (delta / cooling_range).max(0.1);
            let new_temp = state.current_temp - rate;
            state.current_temp = new_temp.max(effective_min);
        }
    }

    // Pump waste heat contribution (applies even when not actively heating)
    if running_pump_count > 0 && ctx.pump_heat_contribution > 0.0 {
        let heat_gain = ctx.pump_heat_contribution * running_pump_count as f32;
        // Don't exceed a reasonable upper bound (set_temp + overshoot if heating,
        // or current_temp if above set_temp already)
        let upper_bound = overshoot_target.max(state.current_temp + heat_gain);
        state.current_temp = (state.current_temp + heat_gain).min(upper_bound);
    }

    // Heating control logic: decide whether to start heating
    if any_pump_on {
        if ctx.heating_overshot {
            // After overshoot, only re-heat when temp drops below hysteresis
            if state.current_temp < set_temp - hysteresis {
                state.is_heating = true;
                ctx.heating_overshot = false;
            }
        } else if state.current_temp < set_temp && !state.is_heating {
            // Only start heating when below set point (not above it)
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
    *rng = rng
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    // Map u64 to [-1.0, 1.0]
    let normalized = (*rng as i64 as f64 / i64::MAX as f64) as f32;
    normalized
}

/// Generate a deterministic pseudo-random u64 using a PRNG state.
pub(crate) fn next_rand(rng: &mut u64) -> u64 {
    *rng = rng
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *rng
}
