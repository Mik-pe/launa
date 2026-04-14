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
pub use discovery::{DiscoveryBuilder, DiscoveryMessage};
pub use topics::{TopicBuilder, LwtConfig, BirthConfig, lwt_config, birth_config, AVAILABILITY_ONLINE, AVAILABILITY_OFFLINE};

#[cfg(feature = "std")]
pub use command_parser::{parse_command, parse_command_ok, parse_set_temperature_validated, ParseResult};
#[cfg(feature = "std")]
pub use state::status_to_json;
