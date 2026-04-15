//! Hardware abstraction layer for Launa.
//!
//! Defines traits for all hardware interactions, enabling desktop testing
//! via mock implementations.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
pub mod network;
pub mod transport;

#[cfg(feature = "std")]
pub use network::Network;
pub use transport::Transport;
