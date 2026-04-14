//! Hardware abstraction layer for Launa.
//!
//! Defines traits for all hardware interactions, enabling desktop testing
//! via mock implementations.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod transport;
pub mod network;

pub use transport::Transport;
pub use network::Network;
