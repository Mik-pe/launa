//! Remote log buffer for capturing log entries.
//!
//! A ring buffer that captures log messages for remote retrieval,
//! designed for use on memory-constrained ESP32 devices.

use alloc::string::String;
use alloc::vec::Vec;

/// Maximum number of log entries in the ring buffer.
/// Keep small to avoid heap pressure on 32 KiB ESP32.
pub const REMOTE_LOG_BUF_SIZE: usize = 16;

/// Maximum length of a single log message (bytes). Longer messages are truncated.
pub const MAX_LOG_MESSAGE_LEN: usize = 128;

/// A single captured log entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub level: &'static str,
    pub message: String,
    pub timestamp_ms: u64,
}

/// Ring buffer state for captured log messages.
///
/// Extracted from `app/src/remote_log.rs` with `Clock` trait injection
/// instead of `embassy_time::Instant` for desktop testability.
pub struct RemoteLogBuffer {
    entries: Vec<LogEntry>,
    head: usize,
    len: usize,
    enabled: bool,
}

impl RemoteLogBuffer {
    /// Create a new empty log buffer.
    pub fn new() -> Self {
        RemoteLogBuffer {
            entries: Vec::new(),
            head: 0,
            len: 0,
            enabled: false,
        }
    }

    /// Initialize the buffer with capacity. Must be called once before use.
    pub fn init(&mut self) {
        if self.entries.is_empty() {
            self.entries.reserve_exact(REMOTE_LOG_BUF_SIZE);
        }
    }

    /// Enable or disable log capture.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether log capture is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Push a log entry into the ring buffer.
    /// If the buffer is full, the oldest entry is overwritten.
    pub fn push(&mut self, level: &'static str, message: &str, timestamp_ms: u64) {
        if !self.enabled {
            return;
        }

        let mut truncated: String = message.chars().take(MAX_LOG_MESSAGE_LEN).collect();
        // Ensure byte length doesn't exceed limit (multi-byte UTF-8)
        while truncated.len() > MAX_LOG_MESSAGE_LEN {
            truncated.pop();
        }
        let entry = LogEntry {
            level,
            message: truncated,
            timestamp_ms,
        };

        if self.len < REMOTE_LOG_BUF_SIZE {
            self.entries.push(entry);
            self.len = self.entries.len();
            self.head = self.entries.len() % REMOTE_LOG_BUF_SIZE;
        } else {
            if self.head < self.entries.len() {
                self.entries[self.head] = entry;
            }
            self.head = (self.head + 1) % REMOTE_LOG_BUF_SIZE;
        }
    }

    /// Drain all captured log entries, returning them as a Vec and clearing the buffer.
    /// Entries are returned in chronological order (oldest first).
    pub fn drain(&mut self) -> Vec<LogEntry> {
        if self.len == 0 {
            return Vec::new();
        }

        let capacity = self.entries.len().min(self.len);
        let mut result = Vec::new();
        for i in 0..capacity {
            let idx = (self.head + i) % self.entries.len();
            if idx < self.entries.len() {
                result.push(self.entries[idx].clone());
            }
        }

        self.entries.clear();
        self.head = 0;
        self.len = 0;

        result
    }

    /// Number of entries currently in the buffer.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for RemoteLogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn test_remote_log_buffer_fifo_order() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        buf.push("warn", "first", 1000);
        buf.push("error", "second", 2000);
        buf.push("warn", "third", 3000);

        assert_eq!(buf.len(), 3);
        let entries = buf.drain();
        assert_eq!(entries.len(), 3);
        // FIFO: oldest first
        assert_eq!(entries[0].level, "warn");
        assert_eq!(entries[0].message, "first");
        assert_eq!(entries[1].level, "error");
        assert_eq!(entries[1].message, "second");
        assert_eq!(entries[2].level, "warn");
        assert_eq!(entries[2].message, "third");
    }

    #[test]
    fn test_remote_log_buffer_wrap_around_overwrite() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        // Fill beyond capacity (REMOTE_LOG_BUF_SIZE = 16)
        for i in 0..REMOTE_LOG_BUF_SIZE + 4 {
            buf.push("warn", &format!("msg {}", i), i as u64 * 100);
        }

        assert_eq!(buf.len(), REMOTE_LOG_BUF_SIZE);

        let entries = buf.drain();
        assert_eq!(entries.len(), REMOTE_LOG_BUF_SIZE);
        // First entry should be msg 4 (oldest surviving after overwrite)
        assert_eq!(entries[0].message, "msg 4");
        // Last entry should be msg 19 (most recent)
        assert_eq!(entries[entries.len() - 1].message, "msg 19");
        // Entries should be in chronological order
        for i in 0..entries.len() - 1 {
            assert!(
                entries[i].timestamp_ms <= entries[i + 1].timestamp_ms,
                "entries should be in chronological order"
            );
        }
    }

    #[test]
    fn test_remote_log_buffer_drain_clears() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        buf.push("warn", "test", 1000);
        assert_eq!(buf.len(), 1);
        assert!(!buf.is_empty());

        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);

        // Second drain returns empty
        let entries2 = buf.drain();
        assert!(entries2.is_empty());
    }

    #[test]
    fn test_remote_log_buffer_enable_disable_toggle() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();

        // Disabled by default
        assert!(!buf.is_enabled());

        // Push while disabled — no effect
        buf.push("warn", "should not appear", 1000);
        assert!(buf.is_empty());

        // Enable
        buf.set_enabled(true);
        assert!(buf.is_enabled());

        buf.push("warn", "should appear", 2000);
        assert_eq!(buf.len(), 1);

        // Disable
        buf.set_enabled(false);
        buf.push("error", "should not appear either", 3000);
        assert_eq!(buf.len(), 1); // Still 1, not 2

        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "should appear");
    }

    #[test]
    fn test_remote_log_buffer_message_truncation() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        let long_msg: String = "x".repeat(200);
        buf.push("warn", &long_msg, 1000);

        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].message.len() <= MAX_LOG_MESSAGE_LEN,
            "message should be truncated to MAX_LOG_MESSAGE_LEN"
        );
        // Truncation is by chars, so len may be <= MAX_LOG_MESSAGE_LEN
        assert_eq!(entries[0].message.len(), MAX_LOG_MESSAGE_LEN);
    }

    #[test]
    fn test_remote_log_buffer_empty_drain() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        let entries = buf.drain();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_remote_log_buffer_push_after_drain() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        buf.push("warn", "first batch", 1000);
        let _ = buf.drain();

        // Push after drain should work
        buf.push("error", "second batch", 2000);
        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "second batch");
    }

    #[test]
    fn test_remote_log_buffer_default() {
        let buf = RemoteLogBuffer::default();
        assert!(buf.is_empty());
        assert!(!buf.is_enabled());
    }

    #[test]
    fn test_remote_log_buffer_exact_capacity() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        // Fill exactly to capacity
        for i in 0..REMOTE_LOG_BUF_SIZE {
            buf.push("warn", &format!("msg {}", i), i as u64);
        }
        assert_eq!(buf.len(), REMOTE_LOG_BUF_SIZE);

        let entries = buf.drain();
        assert_eq!(entries.len(), REMOTE_LOG_BUF_SIZE);
        assert_eq!(entries[0].message, "msg 0");
        assert_eq!(entries[entries.len() - 1].message, "msg 15");
    }

    #[test]
    fn test_remote_log_buffer_multiple_wrap_arounds() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        // Push 3x capacity
        for i in 0..REMOTE_LOG_BUF_SIZE * 3 {
            buf.push("warn", &format!("msg {}", i), i as u64);
        }
        assert_eq!(buf.len(), REMOTE_LOG_BUF_SIZE);

        let entries = buf.drain();
        // Should contain the last REMOTE_LOG_BUF_SIZE entries
        assert_eq!(entries[0].message, "msg 32");
        assert_eq!(entries[entries.len() - 1].message, "msg 47");
    }
}
