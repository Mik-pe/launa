//! Heap memory monitoring.
//!
//! Periodically checks free heap and reports warnings/alerts when
//! memory is running low.

use launa_hal::Timestamp;

use crate::types::{HEAP_CHECK_INTERVAL_MS, HEAP_CRIT_THRESHOLD, HEAP_WARN_THRESHOLD};

/// Heap monitoring state. The caller provides the free heap value.
pub struct HeapMonitor {
    last_check: Option<Timestamp>,
}

impl Default for HeapMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl HeapMonitor {
    pub fn new() -> Self {
        HeapMonitor { last_check: None }
    }

    /// Check heap usage. Returns `Some(critical)` when a check fires:
    /// - `Some(true)` = critically low (< 1 KiB)
    /// - `Some(false)` = warning (< 4 KiB but >= 1 KiB)
    /// - `None` = not time to check yet, or heap is fine
    pub fn tick(&mut self, now: Timestamp, free_heap: usize) -> Option<bool> {
        let should_check = self
            .last_check
            .is_none_or(|last| now.elapsed_since(last) >= HEAP_CHECK_INTERVAL_MS);
        if !should_check {
            return None;
        }
        self.last_check = Some(now);

        if free_heap < HEAP_CRIT_THRESHOLD {
            Some(true)
        } else if free_heap < HEAP_WARN_THRESHOLD {
            Some(false)
        } else {
            None
        }
    }
}
