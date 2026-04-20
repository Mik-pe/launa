//! Remote logging via MQTT.
//!
//! Warn, error, and info level log messages are captured into a ring buffer and
//! forwarded to a dedicated MQTT topic (`launa/{device_id}/log`) as JSON payloads.
//! This allows remote diagnostics of the ESP32 firmware without a serial
//! connection.
//!
//! Capture is wired into the UART logger in `logger.rs` — no separate
//! log::Log implementation is needed. The MQTT task drains the buffer
//! periodically and publishes entries.
//!
//! # Log format
//!
//! Each log message is published as a JSON object:
//! ```json
//! {"level":"warn","message":"...","ts":12345}
//! ```
//!
//! # Heap safety
//!
//! The log ring buffer is fixed-size (REMOTE_LOG_BUF_SIZE entries) to avoid
//! unbounded allocation on the 32 KiB heap. When the buffer is full, the
//! oldest entry is overwritten.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Re-export extracted types from workspace crates
pub use launa_core::{LogEntry, MAX_LOG_MESSAGE_LEN, REMOTE_LOG_BUF_SIZE};

/// Ring buffer state for captured log messages.
///
/// This is the ESP32-specific implementation that uses atomics and `UnsafeCell`
/// for safe access in cooperative async context. The core ring buffer logic is
/// tested via `launa_core::RemoteLogBuffer` on desktop.
pub struct RemoteLogBuffer {
    entries: UnsafeCell<Vec<LogEntry>>,
    head: AtomicUsize,
    len: AtomicUsize,
    enabled: AtomicBool,
}

// Safety: RemoteLogBuffer is only accessed from the cooperative async
// executor context (single-task). The UnsafeCell is safe because:
// 1. embassy uses cooperative scheduling — no preemption between awaits
// 2. push/drain are only called from the log macro context
// 3. init() is called once at startup before any push/drain
unsafe impl Sync for RemoteLogBuffer {}

impl RemoteLogBuffer {
    /// Create a new empty log buffer.
    pub fn new() -> Self {
        RemoteLogBuffer {
            entries: UnsafeCell::new(Vec::new()),
            head: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    /// Initialize the buffer with capacity. Must be called once before use.
    pub fn init(&mut self) {
        if unsafe { &mut *self.entries.get() }.is_empty() {
            unsafe { &mut *self.entries.get() }.reserve_exact(REMOTE_LOG_BUF_SIZE);
        }
    }

    /// Enable or disable log capture.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether log capture is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Push a log entry into the ring buffer.
    /// If the buffer is full, the oldest entry is overwritten.
    pub fn push(&self, level: &'static str, message: &str, timestamp_ms: u64) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        let truncated: String = message.chars().take(MAX_LOG_MESSAGE_LEN).collect();

        let head = self.head.load(Ordering::Relaxed);
        let len = self.len.load(Ordering::Relaxed);

        let entry = LogEntry {
            level,
            message: truncated,
            timestamp_ms,
        };

        // Safety: We're in single-task context (cooperative async).
        // The buffer was initialized in init() and is only accessed
        // from the log context.
        let entries = unsafe { &mut *self.entries.get() };

        if len < REMOTE_LOG_BUF_SIZE {
            entries.push(entry);
            self.len.store(entries.len(), Ordering::Relaxed);
            self.head
                .store(entries.len() % REMOTE_LOG_BUF_SIZE, Ordering::Relaxed);
        } else {
            if head < entries.len() {
                entries[head] = entry;
            }
            self.head
                .store((head + 1) % REMOTE_LOG_BUF_SIZE, Ordering::Relaxed);
        }
    }

    /// Drain all captured log entries, returning them as a Vec and clearing the buffer.
    pub fn drain(&self) -> Vec<LogEntry> {
        let entries = unsafe { &mut *self.entries.get() };
        let head = self.head.load(Ordering::Relaxed);
        let len = self.len.load(Ordering::Relaxed);

        if len == 0 {
            return Vec::new();
        }

        // Return entries in chronological order (oldest first)
        let mut result = Vec::new();
        let capacity = entries.len().min(len);
        for i in 0..capacity {
            let idx = (head + i) % entries.len();
            if idx < entries.len() {
                result.push(entries[idx].clone());
            }
        }

        entries.clear();
        self.head.store(0, Ordering::Relaxed);
        self.len.store(0, Ordering::Relaxed);

        result
    }

    /// Number of entries currently in the buffer.
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Global remote log buffer instance.
static mut REMOTE_LOG_BUFFER: Option<RemoteLogBuffer> = None;

/// Get a reference to the global remote log buffer.
///
/// Returns `None` if `init_remote_log()` has not been called yet.
/// The returned reference is `'static` and safe to use from any task
/// in the cooperative embassy executor.
pub fn remote_log_buffer() -> Option<&'static RemoteLogBuffer> {
    unsafe { REMOTE_LOG_BUFFER.as_ref() }
}

/// Initialize the remote logging system. Call once at startup.
pub fn init_remote_log() {
    unsafe {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);
        REMOTE_LOG_BUFFER = Some(buf);
    }
}

/// Capture a log message for remote forwarding.
/// Called by the custom log implementation.
pub fn capture_log(level: log::Level, message: &str) {
    let level_str = match level {
        log::Level::Error => "error",
        log::Level::Warn => "warn",
        log::Level::Info => "info",
        log::Level::Debug => "debug",
        log::Level::Trace => "trace",
    };

    let ts = unsafe {
        // Use embassy Instant if available, otherwise 0
        embassy_time::Instant::now().as_millis() as u64
    };

    unsafe {
        if let Some(ref mut buf) = REMOTE_LOG_BUFFER {
            buf.push(level_str, message, ts);
        }
    }
}
