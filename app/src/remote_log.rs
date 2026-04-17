//! Optional remote logging via MQTT.
//!
//! When the `remote-log` feature is enabled, warn and error level log messages
//! are captured and forwarded to a dedicated MQTT topic as JSON payloads.
//! This allows remote diagnostics of the ESP32 firmware without a serial
//! connection.
//!
//! # Feature flag
//!
//! Enabled by `cargo +esp check --features remote-log`.
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
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Maximum number of log entries in the ring buffer.
/// Keep small to avoid heap pressure on 32 KiB ESP32.
pub const REMOTE_LOG_BUF_SIZE: usize = 16;

/// Maximum length of a single log message (bytes). Longer messages are truncated.
pub const MAX_LOG_MESSAGE_LEN: usize = 128;

/// Ring buffer state for captured log messages.
pub struct RemoteLogBuffer {
    entries: Vec<LogEntry>,
    head: AtomicUsize,
    len: AtomicUsize,
    enabled: AtomicBool,
}

/// A single captured log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: &'static str,
    pub message: String,
    pub timestamp_ms: u64,
}

impl RemoteLogBuffer {
    /// Create a new empty log buffer.
    pub fn new() -> Self {
        RemoteLogBuffer {
            entries: Vec::new(),
            head: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    /// Initialize the buffer with capacity. Must be called once before use.
    pub fn init(&mut self) {
        if self.entries.is_empty() {
            self.entries
                .reserve_exact(REMOTE_LOG_BUF_SIZE);
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

        // This is safe in single-task context (embassy cooperative scheduling).
        // The buffer is only written from the log macro context which is
        // cooperative and non-preemptive.
        let head = self.head.load(Ordering::Relaxed);
        let len = self.len.load(Ordering::Relaxed);

        let entry = LogEntry {
            level,
            message: truncated,
            timestamp_ms,
        };

        // Safety: We're in single-task context (cooperative async).
        // This is a pattern used in embedded no_std environments.
        // The buffer was initialized in init() and is only accessed
        // from the log context.
        let entries = unsafe { &mut *(&self.entries as *const Vec<LogEntry> as *mut Vec<LogEntry>) };

        if len < REMOTE_LOG_BUF_SIZE {
            entries.push(entry);
            self.len.store(entries.len(), Ordering::Relaxed);
            self.head.store(entries.len() % REMOTE_LOG_BUF_SIZE, Ordering::Relaxed);
        } else {
            if head < entries.len() {
                entries[head] = entry;
            }
            self.head.store((head + 1) % REMOTE_LOG_BUF_SIZE, Ordering::Relaxed);
        }
    }

    /// Drain all captured log entries, returning them as a Vec and clearing the buffer.
    pub fn drain(&self) -> Vec<LogEntry> {
        let entries = unsafe { &mut *(&self.entries as *const Vec<LogEntry> as *mut Vec<LogEntry>) };
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

/// Format a log entry as a JSON string suitable for MQTT publishing.
/// Manual JSON construction (no serde) for no_std compatibility.
pub fn log_entry_to_json(entry: &LogEntry) -> String {
    // Escape the message for JSON
    let mut escaped = String::new();
    for ch in entry.message.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if (c as u32) <= 0x1F => {
                escaped.push_str(&alloc::format!("\\u{:04x}", c as u32));
            }
            c => escaped.push(c),
        }
    }

    alloc::format!(
        "{{\"level\":\"{}\",\"message\":\"{}\",\"ts\":{}}}",
        entry.level, escaped, entry.timestamp_ms
    )
}

/// Global remote log buffer instance.
static mut REMOTE_LOG_BUFFER: Option<RemoteLogBuffer> = None;

/// Get a reference to the global remote log buffer.
/// Must call `init_remote_log()` first.
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

/// Custom log implementation that forwards warn/error to the remote log buffer.
pub struct RemoteLogger;

impl log::Log for RemoteLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            capture_log(record.level(), &alloc::format!("{}", record.args()));
        }
    }

    fn flush(&self) {}
}
