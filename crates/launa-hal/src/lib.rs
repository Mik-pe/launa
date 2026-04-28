//! Hardware abstraction layer for Launa.
//!
//! Defines traits for all hardware interactions, enabling desktop testing
//! via mock implementations.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod clock;
pub mod transport;

pub use clock::{Clock, Timestamp};
pub use transport::Transport;
