//! Partial frame splitting for the spa simulator.
//!
//! Manages the injection of partial frames split across two ticks,
//! simulating real-world conditions where a frame may be received
//! in multiple chunks.

use alloc::vec::Vec;

/// Partial frame splitting subsystem.
///
/// Controls one-shot partial frame injection: the next `tick()` will emit only
/// the first N bytes of the status frame, and the tick after that will emit
/// the remainder plus a Ready frame.
pub struct FrameSplitter {
    /// If set, the next tick() will emit only the first N bytes of the status frame.
    /// One-shot: resets after firing.
    pub(crate) split_point: Option<usize>,
    /// If set, contains the remainder bytes from a partial frame split that should be
    /// emitted at the beginning of the next tick() output.
    pub(crate) remainder: Option<Vec<u8>>,
}

impl Default for FrameSplitter {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSplitter {
    pub fn new() -> Self {
        FrameSplitter {
            split_point: None,
            remainder: None,
        }
    }

    /// Configure a partial frame split at the given byte position.
    ///
    /// Causes the next `tick()` to emit only the first `split_point` bytes of the
    /// status frame. The following `tick()` emits the remainder plus a Ready frame.
    /// One-shot — resets after firing.
    pub fn inject_partial_frame_at(&mut self, split_point: usize) {
        self.split_point = Some(split_point);
        self.remainder = None;
    }

    /// Take the current remainder (if any), clearing it.
    /// Returns Some(remainder_bytes) if a remainder was pending.
    pub fn take_remainder(&mut self) -> Option<Vec<u8>> {
        self.remainder.take()
    }

    /// Take the split point (if any), clearing it.
    /// Returns Some(split_point) if a split was configured.
    pub fn take_split_point(&mut self) -> Option<usize> {
        self.split_point.take()
    }

    /// Store a remainder for the next tick.
    pub fn set_remainder(&mut self, bytes: Vec<u8>) {
        self.remainder = Some(bytes);
    }

    /// Check if there is a pending remainder.
    pub fn has_remainder(&self) -> bool {
        self.remainder.is_some()
    }
}
