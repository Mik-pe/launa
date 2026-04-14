#![cfg_attr(not(test), no_std)]

pub mod crc8;
pub mod frame;
pub mod message;
pub mod status;
pub mod command;
pub mod config;
pub mod registration;
pub mod information;
pub mod fault;
pub mod filter;
pub mod dispatcher;

pub use frame::{Frame, FrameDecoder, FrameEncoder};
pub use message::MessageType;
pub use status::StatusUpdate;
pub use command::Command;
pub use information::InformationResponse;
pub use fault::{FaultLogEntry, FaultCode};
pub use filter::FilterCycles;
pub use dispatcher::{IncomingMessage, dispatch_frame};
