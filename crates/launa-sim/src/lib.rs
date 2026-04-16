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
//!       │ reads → controller, writes → spa
//!       ▼
//!   SpaController (simplified protocol logic for sim tests)
//!       │ emits ControllerEvents
//!       ▼
//!   SimBroker (mock MQTT broker for verification)
//! ```
//!
//! The real firmware logic lives in the `launa-core` crate (`SpaApp`).
//! The `SpaController` here is a simplified version used by sim integration tests.

pub mod clock;
pub mod controller;
pub mod sim_broker;
pub mod sim_transport;
pub mod spa_sim;

pub use clock::VirtualClock;
pub use controller::{ControllerEvent, SpaController};
pub use sim_broker::SimBroker;
pub use sim_transport::SimTransport;
pub use spa_sim::{
    FaultLogConfig, FilterCycleConfig, FilterCyclesConfig, InformationConfig, SpaConfigConfig,
    SpaSim, SpaState,
};

// Re-export protocol types commonly used with the sim
pub use launa_protocol::command::ToggleItem;
pub use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale};
