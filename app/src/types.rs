//! Shared types used across app modules.

use launa_protocol::status::StatusUpdate;

/// State update message sent from the main loop to the MQTT task via STATE_CHANNEL.
///
/// Replaces the previous 5-tuple `(StatusUpdate, FaultBuf, bool, bool, bool)` with
/// named fields for readability.
pub(crate) struct StateMessage {
    pub status: StatusUpdate,
    pub fault: launa_core::FaultBuf,
    pub recovering_from_stale: bool,
    pub sniff_mode: bool,
    /// WiFi RSSI in dBm, or `None` if not available.
    pub wifi_rssi: Option<i32>,
    /// Current registration state as a static string.
    pub registration_state: &'static str,
}

// Re-export FaultBuf from launa-core for convenience
pub(crate) use launa_core::FaultBuf;
