//! Side effects the app logic can request.
//!
//! The caller (ESP32 main loop or test harness) is responsible for executing these.

use alloc::string::String;
use alloc::vec::Vec;
use launa_protocol::status::StatusUpdate;

/// Side effects the app logic can request.
///
/// The caller (ESP32 main loop or test harness) is responsible for executing these.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    /// Write encoded frame bytes to UART.
    SendFrame(Vec<u8>),

    /// Publish status state to MQTT.
    PublishState {
        status: StatusUpdate,
        fault: Option<String>,
        recovering_from_stale: bool,
    },

    /// Publish availability status to MQTT.
    PublishAvailability { online: bool },

    /// Publish stale availability to MQTT.
    PublishStaleAvailability,

    /// Publish all HA discovery configs.
    PublishDiscovery,

    /// Publish diagnostics JSON.
    PublishDiagnostics {
        uptime_secs: u64,
        frames_received: u32,
        unregistered_frames: u32,
        command_retries: u32,
        command_drops: u32,
        registration_state: &'static str,
        frame_errors: u32,
        uart_bytes: u32,
    },

    /// Publish an alert.
    PublishAlert { level: String, message: String },

    /// Request OTA firmware update.
    RequestOta { url: String },
}
