//! Shared LCG (Linear Congruential Generator) pseudo-random number generator.
//!
//! Provides a single deterministic PRNG implementation used across the simulation:
//! - `sim_broker.rs` — loss rate rolling
//! - `spa_sim/physics.rs` — physics noise and random values
//! - `spa_sim/mod.rs` — ready interval randomization, jitter padding

/// Advance a 64-bit LCG PRNG state and return the new value.
///
/// Uses the Numerical Recipes constants:
/// - Multiplier: 6364136223846793005
/// - Increment: 1442695040888963407
pub fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}
