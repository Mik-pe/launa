//! Multi-client channel allocation broker.
//!
//! When multiple devices share the same RS-485 bus, they all receive the
//! spa's `NewClientQuery` (FE BF 00) simultaneously. Without coordination,
//! two or more devices could respond with `NewClientResponse` at the same
//! time, causing bus collisions and registration failures.
//!
//! The `ChannelAllocatorBroker` uses a simple atomic token to ensure only
//! one device responds at a time. Devices that fail to acquire the token
//! yield to the token holder and wait for the next query cycle.
//!
//! This is a no-op when there is only one device on the bus — the token
//! is always available.

use core::sync::atomic::{AtomicBool, Ordering};

/// Token-based lock for coordinating channel allocation across devices.
///
/// Uses a single `AtomicBool` as a mutex. Only one device can hold the
/// token at a time. The token is released automatically when dropped.
pub struct ChannelAllocatorBroker {
    token_taken: AtomicBool,
}

impl Default for ChannelAllocatorBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelAllocatorBroker {
    /// Create a new broker with the token initially available.
    pub const fn new() -> Self {
        ChannelAllocatorBroker {
            token_taken: AtomicBool::new(false),
        }
    }

    /// Try to acquire the allocation token.
    ///
    /// Returns `Some(AllocatorToken)` if this device gets to respond to
    /// the current `NewClientQuery`, or `None` if another device is
    /// already responding (the caller should stay silent this cycle).
    pub fn try_acquire(&self) -> Option<AllocatorToken<'_>> {
        match self
            .token_taken
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => Some(AllocatorToken { broker: self }),
            Err(_) => None,
        }
    }

    /// Whether the token is currently held by any device.
    pub fn is_taken(&self) -> bool {
        self.token_taken.load(Ordering::Relaxed)
    }

    fn release(&self) {
        self.token_taken.store(false, Ordering::Release);
    }
}

/// RAII guard that releases the allocation token when dropped.
///
/// The holder has exclusive rights to respond to `NewClientQuery` for
/// as long as this token is alive. Drop it after sending the response
/// (or after a timeout if no assignment is received).
pub struct AllocatorToken<'a> {
    broker: &'a ChannelAllocatorBroker,
}

impl<'a> Drop for AllocatorToken<'a> {
    fn drop(&mut self) {
        self.broker.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_and_release() {
        let broker = ChannelAllocatorBroker::new();
        assert!(!broker.is_taken());

        let token = broker.try_acquire();
        assert!(token.is_some());
        assert!(broker.is_taken());

        drop(token);
        assert!(!broker.is_taken());
    }

    #[test]
    fn test_only_one_token_at_a_time() {
        let broker = ChannelAllocatorBroker::new();

        let token1 = broker.try_acquire();
        assert!(token1.is_some());

        // Second acquire fails while first is held
        let token2 = broker.try_acquire();
        assert!(token2.is_none());

        // After releasing first, second can acquire
        drop(token1);
        let token3 = broker.try_acquire();
        assert!(token3.is_some());
    }

    #[test]
    fn test_multiple_brokers_independent() {
        let broker1 = ChannelAllocatorBroker::new();
        let broker2 = ChannelAllocatorBroker::new();

        let t1 = broker1.try_acquire();
        let t2 = broker2.try_acquire();
        // Different brokers don't share state
        assert!(t1.is_some());
        assert!(t2.is_some());
    }
}
