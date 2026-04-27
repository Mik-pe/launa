//! MQTT background task for publishing state and receiving commands.
//!
//! This task runs in a loop, multiplexing between:
//! - State updates from the main loop (via STATE_CHANNEL)
//! - Diagnostics payloads (via DIAGNOSTICS_CHANNEL)
//! - Alert payloads (via ALERT_CHANNEL)
//! - Remote log entries (drained from the global log buffer)
//! - Incoming MQTT commands and OTA requests
//!
//! It handles automatic reconnection with exponential backoff and
//! re-publishes discovery/state after reconnect.

use core::sync::atomic::Ordering;

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Timer};
use launa_protocol::status::StatusUpdate;
use log::{info, warn};

use crate::types::FaultBuf;
use crate::*;

static MQTT_PUB_WARN: launa_core::RateLog = launa_core::RateLog::new();

#[embassy_executor::task]
pub(crate) async fn mqtt_task(mut mqtt: mqtt_client::MqttClient) {
    let cmd_sender = COMMAND_CHANNEL.sender();
    let state_rx = STATE_CHANNEL.receiver();
    let diag_rx = DIAGNOSTICS_CHANNEL.receiver();
    let alert_rx = ALERT_CHANNEL.receiver();
    let sniff_rx = SNIFF_CHANNEL.receiver();
    let ota_tx = OTA_CHANNEL.sender();
    let topics = launa_mqtt::topics::TopicBuilder::new(&mqtt.device_id);
    let diag_topic = topics.diagnostics_topic();
    let cmd_base = topics.command_topic();
    let alert_topic = topics.alert_topic();
    #[cfg(feature = "remote-log")]
    let log_topic = topics.log_topic();
    let sniff_topic = topics.sniff_topic();
    let mut last_scale_range: Option<
        (
            launa_protocol::status::TemperatureScale,
            launa_protocol::status::TempRange,
        ),
    > = None;
    let mut last_self_test: bool = false;
    let mut last_sniff_mode: bool = false;
    let mut last_wifi_rssi: Option<i32> = None;
    let mut last_published_status: Option<StatusUpdate> = None;
    let mut last_published_fault: Option<FaultBuf> = None;

    info!("MQTT task started");

    #[allow(unused_assignments)]
    loop {
        // Check for WiFi reconnect signal — force MQTT reconnect
        if WIFI_RECONNECT_SIGNAL.try_take().is_some() {
            warn!("WiFi reconnect detected, forcing MQTT reconnect");
            MQTT_RECONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
            let celsius = last_scale_range.map_or(false, |(s, _)| {
                matches!(s, launa_protocol::status::TemperatureScale::Celsius)
            });
            let last_state = last_published_status.as_ref().map(|status| {
                let fault_str = last_published_fault.as_ref().and_then(|f| f.as_str());
                mqtt_client::LastState {
                    status,
                    fault: fault_str,
                    self_test: last_self_test,
                    sniff_mode: last_sniff_mode,
                    wifi_rssi: last_wifi_rssi,
                }
            });
            crate::net_util::reconnect_with_backoff(
                &mut mqtt,
                celsius,
                last_state.as_ref(),
                "wifi_reconnect_loop",
                "WiFi reconnect exceeded 30 attempts, resetting device",
            )
            .await;
        }

        // Skip all publish work when MQTT is disconnected. This avoids:
        // - Draining the remote log buffer only to fail the publish (logs lost)
        // - Wasting CPU on failed publish attempts for diagnostics/alerts/sniff/state
        if !mqtt.is_connected() {
            // Still need to drain the command channel so it doesn't fill up
            // during disconnection — but recv() below handles reconnection.
            // Jump straight to the recv/select block.
        } else {
            const MAX_NON_CMD_RECEIVES: u8 = 5;
            #[allow(unused_assignments)]
            let mut non_cmd_count: u8 = 0;

            // Check for diagnostics payloads to publish (non-blocking)
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(diag_payload) = diag_rx.try_receive() {
                    if let Err(_) = mqtt.publish(&diag_topic, &diag_payload, 0, false).await {
                        rate_warn!(MQTT_PUB_WARN, "MQTT diagnostics publish failed");
                    }
                    non_cmd_count += 1;
                    continue;
                }
            }

            // Check for alert payloads to publish (non-blocking)
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(alert_payload) = alert_rx.try_receive() {
                    if let Err(_) = mqtt.publish(&alert_topic, &alert_payload, 1, false).await {
                        rate_warn!(MQTT_PUB_WARN, "MQTT alert publish failed");
                    }
                    non_cmd_count += 1;
                    continue;
                }
            }

            // Check for sniff frame payloads to publish (non-blocking)
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(sniff_payload) = sniff_rx.try_receive() {
                    if let Err(_) = mqtt.publish(&sniff_topic, &sniff_payload, 0, false).await {
                        rate_warn!(MQTT_PUB_WARN, "MQTT sniff publish failed");
                    }
                    non_cmd_count += 1;
                    continue;
                }
            }

            // Drain remote log buffer and publish entries (non-blocking)
            #[cfg(feature = "remote-log")]
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Some(log_buf) = crate::remote_log::remote_log_buffer() {
                    if !log_buf.is_empty() {
                        let entries = log_buf.drain();
                        for entry in &entries {
                            let log_entry = launa_mqtt::RemoteLogEntry {
                                level: entry.level,
                                message: entry.message.clone(),
                                timestamp_ms: entry.timestamp_ms,
                            };
                            let json = launa_mqtt::log_entry_to_json(&log_entry);
                            let payload = json.as_bytes();
                            if let Err(_) = mqtt.publish(&log_topic, payload, 0, false).await {
                                rate_warn!(MQTT_PUB_WARN, "MQTT log publish failed");
                                break;
                            }
                        }
                        non_cmd_count += 1;
                        continue;
                    }
                }
            }

            // Check for state updates to publish (non-blocking)
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(msg) = state_rx.try_receive() {
                    let status = msg.status;
                    let fault = msg.fault;
                    let is_stale = msg.recovering_from_stale;
                    let self_test = msg.self_test;
                    let sniff_mode = msg.sniff_mode;
                    let wifi_rssi = msg.wifi_rssi;
                    last_scale_range = Some((status.temperature_scale, status.temp_range));
                    // Force re-publish when self_test or sniff_mode changes so the
                    // first state after mode toggle always reaches the broker.
                    let mode_changed = self_test != last_self_test || sniff_mode != last_sniff_mode;
                    if mode_changed {
                        last_published_status = None;
                    }
                    last_self_test = self_test;
                    last_sniff_mode = sniff_mode;
                    last_wifi_rssi = wifi_rssi;
                    // Change detection: skip publish if state is identical to last
                    let changed = launa_mqtt::state_change::status_changed(
                        last_published_status.as_ref(),
                        &status,
                    );
                    if is_stale || changed {
                        last_published_status = Some(status.clone());
                        last_published_fault = Some(fault);
                        if let Err(_) = mqtt.publish_state(&status, fault.as_str(), self_test, sniff_mode, wifi_rssi, self_test).await {
                            rate_warn!(MQTT_PUB_WARN, "MQTT state publish failed");
                        }
                    }
                    if is_stale {
                        if let Err(_) = mqtt.publish_availability_stale().await {
                            rate_warn!(MQTT_PUB_WARN, "MQTT stale availability publish failed");
                        }
                    } else {
                        // Status received after being stale — publish recovery
                        let _ = mqtt.publish_availability(true).await;
                    }
                    non_cmd_count += 1;
                    continue;
                }
            }
        } // end else (connected)

        // Check for incoming MQTT messages, with a 1-second timeout so we
        // re-check the channels above even when no MQTT messages arrive.
        // Without this timeout, self-test status updates queued in
        // STATE_CHANNEL would never be published because recv() blocks
        // until the next inbound MQTT packet.
        match select(mqtt.recv(), Timer::after(Duration::from_secs(1))).await {
            Either::First(result) => {
                // Got an MQTT message — process it below
                match result {
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

                        // Handle sniff mode toggle command
                        let sniff_subtopic = alloc::format!("{}/sniff", cmd_base);
                        if topic == sniff_subtopic {
                            let payload_str = core::str::from_utf8(&payload).unwrap_or("");
                            let enable = matches!(payload_str, "ON" | "on" | "1" | "true" | "TRUE");
                            info!("MQTT sniff mode command: {}", if enable { "ON" } else { "OFF" });
                            cmd_sender.send(Command::Sniff(enable)).await;
                            continue;
                        }

                        // Handle reboot command
                        let reboot_subtopic = alloc::format!("{}/reboot", cmd_base);
                        if topic == reboot_subtopic {
                            info!("MQTT reboot command received");
                            cmd_sender.send(Command::Reboot).await;
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
                                    // Temperature commands are idempotent — skip rate
                                    // limiting so rapid +/- presses always reach the queue.
                                    let is_temp = matches!(cmd, Command::SetTemperature(_));
                                    if is_temp || mqtt.check_rate_limit() {
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
                        let celsius = last_scale_range.map_or(false, |(s, _)| {
                            matches!(s, launa_protocol::status::TemperatureScale::Celsius)
                        });
                        let last_state = last_published_status.as_ref().map(|status| {
                            let fault_str = last_published_fault.as_ref().and_then(|f| f.as_str());
                            mqtt_client::LastState {
                                status,
                                fault: fault_str,
                                self_test: last_self_test,
                                sniff_mode: last_sniff_mode,
                                wifi_rssi: last_wifi_rssi,
                            }
                        });
                        crate::net_util::reconnect_with_backoff(
                            &mut mqtt,
                            celsius,
                            last_state.as_ref(),
                            "mqtt_reconnect_loop",
                            "MQTT reconnect exceeded 30 attempts, resetting device",
                        )
                        .await;
                    }
                }
            }
            Either::Second(_) => {
                // Timer expired — loop back to check channels above.
                // This ensures self-test status updates and other channel
                // data are published even when no MQTT messages arrive.
            }
        }
    }
}
