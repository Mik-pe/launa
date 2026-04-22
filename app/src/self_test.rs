//! On-device self-test simulator backed by SpaSim.
//!
//! When self-test mode is enabled via MQTT (`launa_spa/command/self_test`),
//! this module wraps a `SpaSim` instance from `launa-sim`. All commands
//! are fed through the simulator's existing frame processing pipeline
//! (encode → process_frame) so behaviour is identical to integration tests.

// Re-export SelfTestState from launa-sim for desktop testability.
pub(crate) use launa_sim::self_test::SelfTestState;
