//! Rate-limited logging macros for the ESP32 app.
//!
//! Delegates to [`launa_core::RateLog`] for the state tracking. These macros
//! wire up the app's `uptime_secs()` to provide the timestamp automatically.
//!
//! Usage:
//! ```
//! static MY_LOG: launa_core::RateLog = launa_core::RateLog::new();
//! rate_warn!(MY_LOG, "something went wrong");
//! ```

pub use launa_core::RATE_LOG_COOLDOWN_SECS;

/// Rate-limited `log::warn!`. Logs immediately on first call, then suppresses
/// for `RATE_LOG_COOLDOWN_SECS` seconds. Subsequent calls log a count summary:
/// `"message (suppressed N)"`.
///
/// Only accepts a single string literal (no format arguments).
#[macro_export]
macro_rules! rate_warn {
    ($state:ident, $msg:literal) => {{
        let now_secs = $crate::uptime_secs() as u32;
        match $state.check(now_secs, $crate::rate_log::RATE_LOG_COOLDOWN_SECS) {
            Ok(0) => log::warn!($msg),
            Ok(n) => log::warn!(concat!($msg, " (suppressed {})"), n),
            Err(_) => { /* suppressed */ }
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
        match $state.check(now_secs, $crate::rate_log::RATE_LOG_COOLDOWN_SECS) {
            Ok(0) => log::error!($msg),
            Ok(n) => log::error!(concat!($msg, " (suppressed {})"), n),
            Err(_) => { /* suppressed */ }
        }
    }};
}
