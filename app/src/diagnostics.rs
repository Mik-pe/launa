//! Diagnostics and alert publishing helpers.
//!
//! These functions format JSON payloads and send them through the diagnostics
//! and alert channels. They perform a heap-free check to avoid OOM panics
//! when memory is critically low.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use log::debug;

use crate::*;

/// Build a diagnostics JSON payload with all counters and publish via the
/// diagnostics channel. Uses SpaApp's internal counters for frames/retries/drops
/// and the static counters for MQTT/WiFi reconnects.
pub(crate) fn publish_diagnostics(
    device_id: &str,
    uptime_secs: u64,
    frames_received: u32,
    command_retries: u32,
    command_drops: u32,
) {
    // Skip if heap is critically low to avoid OOM panic on format!
    let heap_free = esp_alloc::HEAP.free();
    if heap_free < 1024 {
        return;
    }

    let mqtt_reconnects = MQTT_RECONNECT_COUNT.load(Ordering::Relaxed);
    let mqtt_losses = MQTT_LOSS_COUNT.load(Ordering::Relaxed);

    let json = alloc::format!(
        r#"{{"device_id":"{}","uptime_secs":{},"mqtt_reconnect_count":{},"mqtt_loss_count":{},"command_retry_count":{},"command_drop_count":{},"frames_received":{},"heap_free":{},"fw_version":"{}"}}"#,
        device_id,
        uptime_secs,
        mqtt_reconnects,
        mqtt_losses,
        command_retries,
        command_drops,
        frames_received,
        heap_free,
        FIRMWARE_VERSION,
    );

    debug!("Diagnostics: {}", json);

    // Try to send non-blocking; if the channel is full, the diagnostics
    // update is simply skipped (it will be published next cycle).
    let payload = Vec::from(json.as_bytes());
    let _ = DIAGNOSTICS_CHANNEL.try_send(payload);
}

/// Format and send an alert through the alert channel.
/// Called from the main loop for conditions requiring operator attention.
pub(crate) fn send_alert(level: &str, message: &str) {
    // Skip if heap is critically low to avoid OOM panic on format!
    if esp_alloc::HEAP.free() < 1024 {
        return;
    }

    let uptime_secs = uptime_secs();

    let json = alloc::format!(
        r#"{{"level":"{}","message":"{}","timestamp":{}}}"#,
        level, message, uptime_secs
    );

    // Try to send non-blocking; if the channel is full, the alert is dropped
    // (alerts are best-effort and should not block the main loop).
    let payload = Vec::from(json.as_bytes());
    let _ = ALERT_CHANNEL.try_send(payload);
}
