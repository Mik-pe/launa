//! MQTT client wrapper with Home Assistant discovery support.
//!
//! Generates HA auto-discovery payloads and maps spa state to MQTT topics.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
pub mod discovery;
pub mod topics;

#[cfg(feature = "std")]
pub mod command_parser;

#[cfg(feature = "std")]
pub mod state;

#[cfg(feature = "std")]
pub use discovery::DiscoveryBuilder;
pub use topics::TopicBuilder;

#[cfg(feature = "std")]
pub use command_parser::parse_command;
#[cfg(feature = "std")]
pub use state::status_to_json;
