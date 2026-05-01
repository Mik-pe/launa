//! Shared internal constants for launa-core.
//!
//! These constants are used across multiple modules within the crate.

pub(crate) const COMMAND_ACK_TIMEOUT_MS: u64 = 5_000;
pub(crate) const MAX_COMMAND_RETRIES: u8 = 2;
pub(crate) const MAX_PENDING_COMMANDS: usize = 8;
pub(crate) const MAX_COMMAND_QUEUE: usize = 32;

pub(crate) const DEFAULT_PUMP_DURATION_MS: u64 = 20 * 60 * 1000;
pub(crate) const DEFAULT_HOLD_MODE_TIMEOUT_MS: u64 = 60 * 60 * 1000;

pub(crate) const STALE_PROBE_INTERVAL_MS: u64 = 5_000;
pub(crate) const STALE_THRESHOLD_MS: u64 = 30_000;
pub(crate) const REGISTRATION_TIMEOUT_MS: u64 = 5_000;
pub(crate) const DIAGNOSTICS_INTERVAL_MS: u64 = 60_000;

pub(crate) const HEAP_CHECK_INTERVAL_MS: u64 = 30_000;
pub(crate) const HEAP_WARN_THRESHOLD: usize = 4096;
pub(crate) const HEAP_CRIT_THRESHOLD: usize = 1024;
