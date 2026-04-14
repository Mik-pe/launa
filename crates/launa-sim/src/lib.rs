//! Launa Spa Simulation Framework.
//!
//! Provides a complete desktop-testable simulation of a Balboa BP6013G1 spa
//! controller communication over RS-485, with an extracted `SpaController`
//! that can be tested end-to-end without any ESP32 hardware.
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
//!   SpaController (protocol logic, extracted from ESP32 main loop)
//!       │ emits ControllerEvents
//!       ▼
//!   SimBroker (mock MQTT broker for verification)
//! ```

pub mod spa_sim;
pub mod controller;
pub mod sim_transport;
pub mod sim_broker;

pub use spa_sim::{SpaSim, SpaState};
pub use controller::{SpaController, ControllerEvent};
pub use sim_transport::SimTransport;
pub use sim_broker::SimBroker;

// Re-export protocol types commonly used with the sim
pub use launa_protocol::status::{HeatingMode, TemperatureScale, TempRange, PumpState};
pub use launa_protocol::command::ToggleItem;
