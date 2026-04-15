//! Heap monitoring for ESP32.
//!
//! Periodically checks free heap and logs warnings when memory is low.
//! Designed for the 32 KiB heap used by the ESP32 firmware.

use embassy_time::{Duration, Instant};
use log::{info, warn};

/// Minimum heap before warning (4 KB).
const HEAP_WARN_THRESHOLD: usize = 4096;

/// Minimum heap before critical alert (1 KB).
const HEAP_CRIT_THRESHOLD: usize = 1024;

/// Check interval for heap monitoring.
const CHECK_INTERVAL: Duration = Duration::from_secs(60);

pub struct HeapMonitor {
    last_check: Instant,
}

impl HeapMonitor {
    pub fn new() -> Self {
        HeapMonitor {
            last_check: Instant::now(),
        }
    }

    /// Check heap usage. Should be called periodically (e.g., in the main loop).
    /// Returns true if heap is critically low.
    pub fn tick(&mut self) -> bool {
        if self.last_check.elapsed() < CHECK_INTERVAL {
            return false;
        }
        self.last_check = Instant::now();

        let free = esp_alloc::get_free_heap();
        info!("Heap free: {} bytes", free);

        if free < HEAP_CRIT_THRESHOLD {
            warn!(
                "Heap critically low: {} bytes (threshold: {})",
                free, HEAP_CRIT_THRESHOLD
            );
            return true;
        } else if free < HEAP_WARN_THRESHOLD {
            warn!(
                "Heap low: {} bytes (threshold: {})",
                free, HEAP_WARN_THRESHOLD
            );
        }

        false
    }
}
