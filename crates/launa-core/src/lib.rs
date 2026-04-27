//! Extracted application logic for the Launa spa controller.
//!
//! `SpaApp` owns all stateful firmware logic — registration, command tracking,
//! pump timers, hold timers, stale detection, diagnostics, fault handling.
//! It exposes a pure synchronous API that returns `Vec<AppAction>` side effects.
//!
//! The ESP32 `main.rs` becomes thin IO wiring: receive frame → `app.process_frame()`
//! → execute actions. Tests exercise the exact same logic.
//!
//! # Example (desktop test)
//!
//! ```
//! use launa_core::{SpaApp, AppAction};
//! use launa_sim::VirtualClock;
//! use launa_hal::Clock;
//!
//! let clock = Box::leak(Box::new(VirtualClock::new()));
//! let mut app = SpaApp::new(clock);
//!
//! // Process a tick, get actions back
//! let actions = app.tick();
//! for action in actions {
//!     // handle or assert on action
//! }
//! ```

#![no_std]

extern crate alloc;

mod actions;
mod command_tracker;
mod fault_buf;
mod heap_monitor;
mod log_buffer;
pub mod network;
mod rate_limiter;
mod rate_log;
mod spa_app;
mod timers;
mod types;

// Re-export all public items to preserve the public API
pub use actions::AppAction;
pub use command_tracker::{CommandTracker, VerifyResult};
pub use fault_buf::FaultBuf;
pub use heap_monitor::HeapMonitor;
pub use log_buffer::{LogEntry, RemoteLogBuffer, MAX_LOG_MESSAGE_LEN, REMOTE_LOG_BUF_SIZE};
pub use network::{backoff_secs, parse_ip};
pub use rate_limiter::{RateLimiter, RATE_LIMIT_MAX_COMMANDS, RATE_LIMIT_WINDOW_MS};
pub use rate_log::{RateLog, RATE_LOG_COOLDOWN_SECS};
pub use spa_app::SpaApp;
pub use timers::{HoldModeTimer, PumpTimer, PumpTimerManager};
