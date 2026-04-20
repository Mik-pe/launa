//! Launa Spa Simulation Framework.
//!
//! Provides a complete desktop-testable simulation of a Balboa BP6013G1 spa
//! controller communication over RS-485.
//!
//! # Architecture
//!
//! ```text
//!   SpaSim (simulates real spa hardware)
//!       │ writes bytes to SimTransport
//!       ▼
//!   SimTransport (virtual RS-485 wire)
//!       │ reads → SpaApp, writes → spa
//!       ▼
//!   SpaApp (real firmware logic from launa-core)
//!       │ emits AppActions
//!       ▼
//!   SimBroker (mock MQTT broker for verification)
//! ```
//!
//! The real firmware logic lives in the `launa-core` crate (`SpaApp`).

#![no_std]

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;

#[cfg(feature = "std")]
pub mod clock;
#[cfg(feature = "std")]
pub mod sim_broker;
#[cfg(feature = "std")]
pub mod sim_transport;
pub mod spa_sim;

#[cfg(feature = "std")]
pub use clock::VirtualClock;
#[cfg(feature = "std")]
pub use sim_broker::SimBroker;
#[cfg(feature = "std")]
pub use sim_transport::SimTransport;
pub use spa_sim::{
    FaultLogConfig, FilterCycleConfig, FilterCyclesConfig, InformationConfig, SpaConfigConfig,
    SpaSim, SpaState,
};

// Re-export protocol types commonly used with the sim
pub use launa_protocol::command::ToggleItem;
pub use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale};
