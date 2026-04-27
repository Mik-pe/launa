//! Rate-limited logging with suppress + count summary.
//!
//! Each log site gets a static state (last emit time + suppressed count).
//! The first occurrence logs immediately. Subsequent occurrences within the
//! cooldown window are silently counted. When the cooldown expires, a summary
//! is logged: "message (suppressed N)".
//!
//! Usage:
//! ```
//! static MY_LOG: RateLog = RateLog::new();
//! rate_warn!(MY_LOG, "something went wrong");
//! ```

use core::sync::atomic::{AtomicU32, Ordering};

/// Seconds between repeated log emissions for the same site.
const COOLDOWN_SECS: u32 = 1;

/// Tracks rate-limiting state for a single log site.
///
/// Encoding: bits 0-23 = suppressed count, bits 24-31 = unused.
/// The last-emit timestamp is stored as seconds since boot (lower 32 bits).
pub struct RateLog {
    last_emit_secs: AtomicU32,
    suppressed: AtomicU32,
}

impl RateLog {
    pub const fn new() -> Self {
        Self {
            last_emit_secs: AtomicU32::new(0),
            suppressed: AtomicU32::new(0),
        }
    }

    /// Attempt to emit a log message. Returns the number of suppressed messages
    /// (including this one) if the message should be suppressed, or 0 if it
    /// should be emitted now.
    ///
    /// When returning 0, the caller should log the message.
    /// When returning >0, the caller should append "(suppressed N)" to the message.
    pub fn check(&self, now_secs: u32) -> u32 {
        let last = self.last_emit_secs.load(Ordering::Relaxed);

        // First ever log — emit immediately
        if last == 0 {
            self.last_emit_secs.store(now_secs, Ordering::Relaxed);
            return 0;
        }

        let elapsed = now_secs.saturating_sub(last);

        if elapsed < COOLDOWN_SECS {
            // Within cooldown — suppress
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            self.suppressed.load(Ordering::Relaxed)
        } else {
            // Cooldown expired — emit with summary of suppressed count
            let count = self.suppressed.swap(0, Ordering::Relaxed);
            self.last_emit_secs.store(now_secs, Ordering::Relaxed);
            count
        }
    }
}

/// Rate-limited `log::warn!`. Logs immediately on first call, then suppresses
/// for `COOLDOWN_SECS` seconds. Subsequent calls log a count summary:
/// `"message (suppressed N)"`.
///
/// Only accepts a single string literal (no format arguments).
#[macro_export]
macro_rules! rate_warn {
    ($state:ident, $msg:literal) => {{
        let now_secs = $crate::uptime_secs() as u32;
        let suppressed = $state.check(now_secs);
        if suppressed == 0 {
            log::warn!($msg);
        } else {
            log::warn!(concat!($msg, " (suppressed {})"), suppressed);
        }
    }};
}

/// Rate-limited `log::error!`. Same suppress + count behavior as `rate_warn!`.
///
/// Only accepts a single string literal (no format arguments).
#[macro_export]
macro_rules! rate_error {
    ($state:ident, $msg:literal) => {{
        let now_secs = $crate::uptime_secs() as u32;
        let suppressed = $state.check(now_secs);
        if suppressed == 0 {
            log::error!($msg);
        } else {
            log::error!(concat!($msg, " (suppressed {})"), suppressed);
        }
    }};
}
