//! MQTT background task for publishing state and receiving commands.
//!
//! This task runs in a loop, multiplexing between:
//! - State updates from the main loop (via STATE_CHANNEL)
//! - Diagnostics payloads (via DIAGNOSTICS_CHANNEL)
//! - Alert payloads (via ALERT_CHANNEL)
//! - Incoming MQTT commands and OTA requests
//!
//! It handles automatic reconnection with exponential backoff and
//! re-publishes discovery/state after reconnect.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use embassy_time::{Duration, Instant, Timer};
use launa_protocol::status::StatusUpdate;
use log::{error, info, warn};

use crate::types::FaultBuf;
use crate::*;

#[embassy_executor::task]
pub(crate) async fn mqtt_task(mut mqtt: mqtt_client::MqttClient) {
    let cmd_sender = COMMAND_CHANNEL.sender();
    let state_rx = STATE_CHANNEL.receiver();
    let diag_rx = DIAGNOSTICS_CHANNEL.receiver();
    let alert_rx = ALERT_CHANNEL.receiver();
    let ota_tx = OTA_CHANNEL.sender();
    let topics = launa_mqtt::topics::TopicBuilder::new(&mqtt.device_id);
    let diag_topic = topics.diagnostics_topic();
    let cmd_base = topics.command_topic();
    let alert_topic = topics.alert_topic();
    let mut last_scale_range: Option<
        (
            launa_protocol::status::TemperatureScale,
            launa_protocol::status::TempRange,
        ),
    > = None;
    let mut last_published_status: Option<StatusUpdate> = None;
    let mut last_published_fault: Option<FaultBuf> = None;

    info!("MQTT task started");

    loop {
        // Check for WiFi reconnect signal — force MQTT reconnect
        if WIFI_RECONNECT_SIGNAL.try_take().is_some() {
            warn!("WiFi reconnect detected, forcing MQTT reconnect");
            MQTT_RECONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
            let mut wifi_attempt: u32 = 0;
            let mut last_wifi_alert: Option<Instant> = None;
            loop {
                wifi_attempt += 1;
                match mqtt.reconnect().await {
                    Ok(()) => {
                        let celsius = last_scale_range
                            .map_or(false, |(s, _)| {
                                matches!(s, launa_protocol::status::TemperatureScale::Celsius)
                            });
                        if let Err(e) = mqtt.publish_availability(true).await {
                            warn!("WiFi-reconnect: publish availability failed: {:?}", e);
                        }
                        if let Err(e) = mqtt.publish_discovery(celsius).await {
                            warn!("WiFi-reconnect: publish discovery failed: {:?}", e);
                        }
                        if let Err(e) = mqtt.subscribe_commands().await {
                            warn!("WiFi-reconnect: subscribe commands failed: {:?}", e);
                        }
                        // Re-publish last known state
                        if let Some(ref status) = last_published_status {
                            let fault_str =
                                last_published_fault.as_ref().and_then(|f| f.as_str());
                            if let Err(e) = mqtt.publish_state(status, fault_str).await {
                                warn!("WiFi-reconnect: publish state failed: {:?}", e);
                            }
                        }
                        break;
                    }
                    Err(e) => {
                        // Exponential backoff: 5s, 10s, 20s, 40s, 60s, 60s, ...
                        let backoff_secs = if wifi_attempt > 10 {
                            60
                        } else {
                            crate::net_util::backoff_secs(wifi_attempt)
                        };
                        error!(
                            "WiFi-reconnect MQTT attempt {} failed: {:?}, retrying in {}s",
                            wifi_attempt, e, backoff_secs
                        );
                        // Publish alert after 3 attempts, throttled to once per 60s
                        if wifi_attempt > 3 {
                            let now = Instant::now();
                            let should_alert = last_wifi_alert
                                .map(|t| t.elapsed() >= Duration::from_secs(60))
                                .unwrap_or(true);
                            if should_alert {
                                let json = alloc::format!(
                                    r#"{{"level":"error","message":"wifi_reconnect_loop","attempts":{},"timestamp":{}}}"#,
                                    wifi_attempt,
                                    uptime_secs()
                                );
                                let payload = Vec::from(json.as_bytes());
                                let _ = ALERT_CHANNEL.try_send(payload);
                                last_wifi_alert = Some(now);
                            }
                        }
                        if wifi_attempt >= 30 {
                            error!(
                                "WiFi reconnect exceeded 30 attempts, resetting device"
                            );
                            esp_hal::system::software_reset();
                        }
                        Timer::after(Duration::from_secs(backoff_secs)).await;
                    }
                }
            }
        }

        // Drain non-command channels with a bounding counter to prevent
        // starving command processing. Without this limit, a continuous
        // stream of diagnostics/alerts/state updates with `continue` could
        // indefinitely delay `mqtt.recv()`, causing missed commands.
        //
        // The counter resets to 0 each loop iteration (each iteration gets
        // its own budget), so increments in `continue` branches are never
        // read across iterations — hence the unused_assignments allow on
        // the increment lines below.
        const MAX_NON_CMD_RECEIVES: u8 = 5;
        let mut non_cmd_count: u8 = 0;

        // Check for diagnostics payloads to publish (non-blocking)
        if non_cmd_count < MAX_NON_CMD_RECEIVES {
            if let Ok(diag_payload) = diag_rx.try_receive() {
                if let Err(e) = mqtt.publish(&diag_topic, &diag_payload, 0, false).await {
                    warn!("MQTT diagnostics publish failed: {:?}", e);
                }
                non_cmd_count += 1;
                continue;
            }
        }

        // Check for alert payloads to publish (non-blocking)
        if non_cmd_count < MAX_NON_CMD_RECEIVES {
            if let Ok(alert_payload) = alert_rx.try_receive() {
                if let Err(e) = mqtt.publish(&alert_topic, &alert_payload, 1, false).await {
                    warn!("MQTT alert publish failed: {:?}", e);
                }
                non_cmd_count += 1;
                continue;
            }
        }

        // Check for state updates to publish (non-blocking)
        if non_cmd_count < MAX_NON_CMD_RECEIVES {
            if let Ok((status, fault, is_stale)) = state_rx.try_receive() {
                last_scale_range = Some((status.temperature_scale, status.temp_range));
                // Change detection: skip publish if state is identical to last
                let changed = last_published_status.as_ref().map_or(true, |prev| {
                    prev.current_temp != status.current_temp
                        || prev.set_temp != status.set_temp
                        || prev.is_heating != status.is_heating
                        || prev.pumps != status.pumps
                        || prev.lights != status.lights
                        || prev.blower != status.blower
                        || prev.circ_pump != status.circ_pump
                        || prev.mister != status.mister
                        || prev.is_hold != status.is_hold
                        || prev.heating_mode != status.heating_mode
                        || prev.temp_range != status.temp_range
                        || prev.hold_timer_minutes != status.hold_timer_minutes
                });
                if is_stale || changed {
                    last_published_status = Some(status.clone());
                    last_published_fault = Some(fault);
                    if let Err(e) = mqtt.publish_state(&status, fault.as_str()).await {
                        warn!("MQTT state publish failed: {:?}", e);
                    }
                }
                if is_stale {
                    if let Err(e) = mqtt.publish_availability_stale().await {
                        warn!("MQTT stale availability publish failed: {:?}", e);
                    }
                } else {
                    // Status received after being stale — publish recovery
                    let _ = mqtt.publish_availability(true).await;
                }
                non_cmd_count += 1;
                continue;
            }
        }

        // Check for incoming MQTT messages
        match mqtt.recv().await {
            Some((topic, payload)) => {
                info!("MQTT received: {} ({} bytes)", topic, payload.len());

                // Handle OTA commands
                if mqtt.is_ota_topic(&topic) {
                    if let Some(url) = mqtt_client::MqttClient::parse_ota_url(&payload) {
                        info!("OTA firmware URL: {}", url);
                        // Graceful shutdown before OTA reboot
                        info!("OTA: graceful shutdown — publishing offline...");
                        let _ = mqtt.publish_availability(false).await;
                        info!("OTA: sending MQTT DISCONNECT...");
                        mqtt.disconnect().await;
                        info!("OTA: draining UART TX channel...");
                        while UART_TX_CHANNEL.try_receive().is_ok() {
                            // Drain pending UART writes
                        }
                        // Allow time for in-flight UART bytes to complete
                        Timer::after(Duration::from_millis(50)).await;
                        info!("OTA: shutdown complete, sending URL to main loop");
                        ota_tx.send(url).await;
                        // Do NOT reconnect — the main loop needs the TCP socket for OTA download.
                        // The device will reset after OTA completes (or the main loop handles failure).
                        info!("OTA: MQTT task idle, waiting for device reset");
                        loop {
                            Timer::after(Duration::from_secs(60)).await;
                        }
                    } else {
                        warn!("Invalid OTA payload");
                    }
                    continue;
                }

                // Handle HA status (re-publish discovery when HA restarts)
                if mqtt.is_ha_status_topic(&topic) {
                    let status = core::str::from_utf8(&payload).unwrap_or("");
                    if status == "online" {
                        info!("Home Assistant came online, re-publishing discovery");
                        let celsius = last_scale_range.map_or(false, |(s, _)| {
                            matches!(s, launa_protocol::status::TemperatureScale::Celsius)
                        });
                        if let Err(e) = mqtt.publish_discovery(celsius).await {
                            warn!("HA status: publish discovery failed: {:?}", e);
                        }
                        if let Err(e) = mqtt.publish_availability(true).await {
                            warn!("HA status: publish availability failed: {:?}", e);
                        }
                    }
                    continue;
                }

                // Handle self-test toggle command
                let self_test_subtopic = alloc::format!("{}/self_test", cmd_base);
                if topic == self_test_subtopic {
                    let payload_str = core::str::from_utf8(&payload).unwrap_or("");
                    let enable = matches!(payload_str, "ON" | "on" | "1" | "true" | "TRUE");
                    info!("MQTT self-test command: {}", if enable { "ON" } else { "OFF" });
                    cmd_sender.send(Command::SelfTest(enable)).await;
                    continue;
                }

                // Handle commands and pump timers (with rate limiting)
                let (scale, range) = match last_scale_range {
                    Some((s, r)) => (Some(s), Some(r)),
                    None => (None, None),
                };
                if let Some(action) =
                    mqtt_client::parse_command(&cmd_base, &topic, &payload, scale, range)
                {
                    match action {
                        mqtt_client::MqttAction::Command(cmd) => {
                            if mqtt.check_rate_limit() {
                                info!("MQTT command: {:?}", cmd);
                                cmd_sender.send(cmd).await;
                            }
                            // Rate-limited commands are silently dropped (warn logged in check_rate_limit)
                        }
                        mqtt_client::MqttAction::StartPumpTimer { pump, minutes } => {
                            info!("MQTT pump timer: pump {} for {} min", pump, minutes);
                            PUMP_TIMER_CHANNEL.send((pump, minutes)).await;
                        }
                        mqtt_client::MqttAction::SelfTest(_) => {
                            // Handled above before parse_command; unreachable here
                        }
                    }
                } else {
                    let payload_str = core::str::from_utf8(&payload).unwrap_or("<non-utf8>");
                    warn!("MQTT command not recognized: topic={} payload={}", topic, payload_str);
                }
            }
            None => {
                let reason = mqtt.last_disconnect.take().unwrap_or_else(|| alloc::string::String::from("unknown"));
                warn!("MQTT connection lost ({}), attempting reconnect...", reason);
                MQTT_RECONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
                MQTT_LOSS_COUNT.fetch_add(1, Ordering::Relaxed);
                let mut attempt: u32 = 0;
                let mut last_alert_time: Option<Instant> = None;
                loop {
                    attempt += 1;
                    match mqtt.reconnect().await {
                        Ok(()) => {
                            info!("MQTT reconnected, re-publishing...");
                            let celsius = last_scale_range.map_or(false, |(s, _)| {
                                matches!(s, launa_protocol::status::TemperatureScale::Celsius)
                            });
                            if let Err(e) = mqtt.publish_availability(true).await {
                                warn!("MQTT reconnect: publish availability failed: {:?}", e);
                            }
                            if let Err(e) = mqtt.publish_discovery(celsius).await {
                                warn!("MQTT reconnect: publish discovery failed: {:?}", e);
                            }
                            if let Err(e) = mqtt.subscribe_commands().await {
                                warn!("MQTT reconnect: subscribe commands failed: {:?}", e);
                            }
                            // Re-publish last known state after reconnect
                            if let Some(ref status) = last_published_status {
                                let fault_str =
                                    last_published_fault.as_ref().and_then(|f| f.as_str());
                                if let Err(e) = mqtt.publish_state(status, fault_str).await {
                                    warn!("MQTT reconnect: publish state failed: {:?}", e);
                                }
                            }
                            break;
                        }
                        Err(e) => {
                            // Exponential backoff: 5s, 10s, 20s, 40s, 60s, 60s, ...
                            let backoff_secs =
                                crate::net_util::backoff_secs(attempt);
                            error!(
                                "MQTT reconnect attempt {} failed: {:?}, retrying in {}s",
                                attempt, e, backoff_secs
                            );
                            // Publish alert after 3 attempts, throttled to once per 60s
                            if attempt > 3 {
                                let now = Instant::now();
                                let should_alert = last_alert_time
                                    .map(|t| t.elapsed() >= Duration::from_secs(60))
                                    .unwrap_or(true);
                                if should_alert {
                                    let json = alloc::format!(
                                        r#"{{"level":"error","message":"mqtt_reconnect_loop","attempts":{},"timestamp":{}}}"#,
                                        attempt,
                                        uptime_secs()
                                    );
                                    let payload = Vec::from(json.as_bytes());
                                    let _ = ALERT_CHANNEL.try_send(payload);
                                    last_alert_time = Some(now);
                                }
                            }
                            if attempt >= 30 {
                                error!(
                                    "MQTT reconnect exceeded 30 attempts, resetting device"
                                );
                                esp_hal::system::software_reset();
                            }
                            Timer::after(Duration::from_secs(backoff_secs)).await;
                        }
                    }
                }
            }
        }
    }
}
