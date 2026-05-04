//! Balboa BP6013G1 spa controller RS-485 protocol parser.
//!
//! Handles frame encoding/decoding (0x7E-delimited with CRC-8, length-field framing), message
//! dispatch, status/config/fault/filter/information parsing, command
//! encoding, and the client registration state machine.
//!
//! # Modules
//!
//! - [`frame`] — Frame encode/decode with CRC-8 and length-field framing (0x7E delimited)
//! - [`status`] — Real-time spa status (temperatures, pumps, heating mode)
//! - [`command`] — Command encoding (toggle items, set temperature, settings)
//! - [`config`] — Control configuration parsing (temperature range, scale)
//! - [`fault`] — Fault log entries and fault codes
//! - [`filter`] — Filter cycle schedule parsing
//! - [`information`] — Spa information response (firmware, model, configuration)
//! - [`registration`] — Client registration state machine (discover → query → assignment)
//! - [`dispatcher`] — Frame-to-message dispatch (routes parsed frames to [`IncomingMessage`])
//! - [`crc8`] — CRC-8 lookup table for frame integrity

#![cfg_attr(not(test), no_std)]

pub mod command;
pub mod config;
pub mod crc8;
pub mod dispatcher;
pub mod fault;
pub mod filter;
pub mod frame;
pub mod hex;
pub mod information;
pub mod pump_bits;
pub mod registration;
pub mod status;
pub mod temperature;

pub use command::Command;
pub use dispatcher::{dispatch_frame, IncomingMessage};
pub use fault::{FaultCode, FaultLogEntry};
pub use filter::FilterCycles;
pub use frame::{Frame, FrameDecoder, FrameEncoder, FrameError};
pub use information::InformationResponse;
pub use registration::Channel;
pub use status::StatusUpdate;
pub use temperature::{Temperature, TemperatureError};
