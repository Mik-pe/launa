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

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Timer};
use launa_protocol::status::StatusUpdate;
use log::{error, info, warn};

use crate::types::FaultBuf;
use crate::sniff::SNIFF_CHANNEL;
use crate::*;

static MQTT_PUB_WARN: launa_core::RateLog = launa_core::RateLog::new();

/// Monotonically increasing counter bumped each time the mqtt_task main loop
/// completes an iteration. The main event loop reads this periodically; if the
/// value hasn't changed in 30 seconds it means the MQTT task is frozen.
pub(crate) static MQTT_TASK_TICK: AtomicU32 = AtomicU32::new(0);

/// Compute whether the last known temperature scale is Celsius.
fn is_celsius(
    last_scale_range: &Option<(
        launa_protocol::status::TemperatureScale,
        launa_protocol::status::TempRange,
    )>,
) -> bool {
    last_scale_range.map_or(false, |(s, _)| {
        matches!(s, launa_protocol::status::TemperatureScale::Celsius)
    })
}

/// Build a `LastState` snapshot from the current tracking variables.
fn build_last_state<'a>(
    status: &'a Option<StatusUpdate>,
    fault: &'a Option<FaultBuf>,
    sniff_mode: bool,
    wifi_rssi: Option<i32>,
    registration_state: &'a str,
) -> Option<mqtt_client::LastState<'a>> {
    status.as_ref().map(|s| {
        let fault_str = fault.as_ref().and_then(|f| f.as_str());
        mqtt_client::LastState {
            status: s,
            fault: fault_str,
            sniff_mode,
            wifi_rssi,
            registration_state,
        }
    })
}

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
    let mut last_scale_range: Option<(
        launa_protocol::status::TemperatureScale,
        launa_protocol::status::TempRange,
    )> = None;
    let mut last_sniff_mode: bool = false;
    let mut last_wifi_rssi: Option<i32> = None;
    let mut last_published_status: Option<StatusUpdate> = None;
    let mut last_published_fault: Option<FaultBuf> = None;
    let mut last_registration_state: &str = "waiting_for_query";

    info!("MQTT task started");

    // Initial connection with backoff
    {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match mqtt.connect().await {
                Ok(()) => {
                    info!("MQTT connected on attempt {}", attempt);
                    break;
                }
                Err(e) => {
                    let backoff = launa_core::network::backoff_secs(attempt);
                    warn!(
                        "MQTT connect attempt {} failed: {:?}, retrying in {}s",
                        attempt, e, backoff
                    );
                    if attempt >= 10 {
                        error!("MQTT connect failed after {} attempts, resetting", attempt);
                        esp_hal::system::software_reset();
                    }
                    Timer::after(Duration::from_secs(backoff)).await;
                }
            }
        }
    }

    // Post-connect publish: discovery, availability, subscribe
    if let Err(e) = mqtt.post_connect_publish(false).await {
        warn!("Post-connect publish failed: {:?}", e);
    }

    // Signal to main task that MQTT is connected
    crate::MQTT_CONNECTED_SIGNAL.signal(());

    loop {
        // Check for WiFi reconnect signal — force MQTT reconnect
        if WIFI_RECONNECT_SIGNAL.try_take().is_some() {
            warn!("WiFi reconnect detected, forcing MQTT reconnect");
            MQTT_RECONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
            let celsius = is_celsius(&last_scale_range);
            let last_state = build_last_state(
                &last_published_status,
                &last_published_fault,
                last_sniff_mode,
                last_wifi_rssi,
                last_registration_state,
            );
            crate::net_util::reconnect_with_backoff(
                &mut mqtt,
                celsius,
                last_state.as_ref(),
                "wifi_reconnect_loop",
                "WiFi reconnect exceeded 30 attempts, resetting device",
            )
            .await;
            crate::MQTT_CONNECTED_SIGNAL.signal(());
        }

        // Skip all publish work when MQTT is disconnected. This avoids:
        // - Draining the remote log buffer only to fail the publish (logs lost)
        // - Wasting CPU on failed publish attempts for diagnostics/alerts/sniff/state
        if mqtt.is_connected() {
            const MAX_NON_CMD_RECEIVES: u8 = 5;
            let mut non_cmd_count: u8 = 0;

            // Check for diagnostics payloads to publish (non-blocking)
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(diag_payload) = diag_rx.try_receive() {
                    if mqtt.publish(&diag_topic, &diag_payload, 0, false).await.is_err() {
                        rate_warn!(MQTT_PUB_WARN, "MQTT diagnostics publish failed");
                    }
                    non_cmd_count += 1;
                    continue;
                }
            }

            // Check for alert payloads to publish (non-blocking)
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(alert_payload) = alert_rx.try_receive() {
                    if mqtt.publish(&alert_topic, &alert_payload, 1, false).await.is_err() {
                        rate_warn!(MQTT_PUB_WARN, "MQTT alert publish failed");
                    }
                    non_cmd_count += 1;
                    continue;
                }
            }

            // Check for sniff frame payloads to publish (non-blocking)
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(sniff_payload) = sniff_rx.try_receive() {
                    if mqtt.publish(&sniff_topic, &sniff_payload, 0, false).await.is_err() {
                        rate_warn!(MQTT_PUB_WARN, "MQTT sniff publish failed");
                    }
                    non_cmd_count += 1;
                    continue;
                }
            }

            // Check for state updates to publish (non-blocking).
            // Checked before remote-log drain so that state changes (which
            // drive the web UI) are never starved by a flood of log entries.
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Ok(msg) = state_rx.try_receive() {
                    let status = msg.status;
                    let fault = msg.fault;
                    let is_stale = msg.recovering_from_stale;
                    let sniff_mode = msg.sniff_mode;
                    let wifi_rssi = msg.wifi_rssi;
                    let registration_state = msg.registration_state;
                    last_scale_range = Some((status.temperature_scale, status.temp_range));
                    // Force re-publish when sniff_mode changes so the
                    // first state after mode toggle always reaches the broker.
                    let mode_changed = sniff_mode != last_sniff_mode;
                    // Also force re-publish when registration_state changes.
                    let reg_changed = registration_state != last_registration_state;
                    if mode_changed || reg_changed {
                        last_published_status = None;
                    }
                    last_sniff_mode = sniff_mode;
                    last_registration_state = registration_state;
                    // Change detection: skip publish if state is identical to last
                    let changed = launa_mqtt::state_change::status_changed(
                        last_published_status.as_ref(),
                        &status,
                    );
                    let rssi_changed = wifi_rssi != last_wifi_rssi;
                    last_wifi_rssi = wifi_rssi;
                    if is_stale || changed || rssi_changed {
                        last_published_status = Some(status.clone());
                        last_published_fault = Some(fault);
                        if mqtt
                            .publish_state(
                                &status,
                                fault.as_str(),
                                sniff_mode,
                                wifi_rssi,
                                false,
                                registration_state,
                            )
                            .await
                            .is_err()
                        {
                            rate_warn!(MQTT_PUB_WARN, "MQTT state publish failed");
                        }
                    }
                    if is_stale {
                        if mqtt.publish_availability_stale().await.is_err() {
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

            // Drain remote log buffer and publish entries (non-blocking).
            // Limited to MAX_LOG_ENTRIES_PER_ITER to prevent starving the
            // recv/select block below when log production is high.
            //
            // If publish fails, we silently stop trying this iteration to
            // avoid a feedback loop: failed publish -> log::warn -> new
            // log entry -> drain -> failed publish -> ...
            #[cfg(feature = "remote-log")]
            if non_cmd_count < MAX_NON_CMD_RECEIVES {
                if let Some(log_buf) = crate::remote_log::remote_log_buffer() {
                    if !log_buf.is_empty() {
                        const MAX_LOG_ENTRIES_PER_ITER: usize = 3;
                        let entries = log_buf.drain();
                        for (i, entry) in entries.iter().enumerate() {
                            if i >= MAX_LOG_ENTRIES_PER_ITER {
                                break;
                            }
                            let log_entry = launa_mqtt::RemoteLogEntry {
                                level: entry.level,
                                message: entry.message.clone(),
                                timestamp_ms: entry.timestamp_ms,
                            };
                            let json = launa_mqtt::log_entry_to_json(&log_entry);
                            let payload = json.as_bytes();
                            if mqtt.publish(&log_topic, payload, 0, false).await.is_err() {
                                break;
                            }
                        }
                        non_cmd_count += 1;
                        continue;
                    }
                }
            }
        } // end if connected

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
                                info!("OTA firmware URL received, forwarding to main loop");
                                // Send URL to main loop immediately — do NOT disconnect MQTT
                                // or publish offline. The main loop handles the OTA download
                                // over a new TCP connection (the MQTT socket is separate).
                                // The device will reset after OTA completes.
                                ota_tx.send(url).await;
                                // Idle until device resets. Do NOT reconnect or process
                                // more messages — the OTA URL has been consumed.
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
                                let celsius = is_celsius(&last_scale_range);
                                if let Err(e) = mqtt.publish_discovery(celsius).await {
                                    warn!("HA status: publish discovery failed: {:?}", e);
                                }
                                if let Err(e) = mqtt.publish_availability(true).await {
                                    warn!("HA status: publish availability failed: {:?}", e);
                                }
                            }
                            continue;
                        }

                        // Handle sniff mode command: JSON {"frames":N} or "OFF" to cancel
                        if topic.strip_prefix(&cmd_base) == Some("/sniff") {
                            let payload_str = core::str::from_utf8(&payload).unwrap_or("");
                            let frame_count: Option<u16> = if payload_str
                                .eq_ignore_ascii_case("OFF")
                            {
                                None
                            } else {
                                // Minimal JSON parsing for {"frames":N}
                                payload_str
                                    .find("\"frames\"")
                                    .and_then(|pos| {
                                        let rest = &payload_str[pos + 8..];
                                        rest.trim_start().strip_prefix(':').map(|s| s.trim_start())
                                    })
                                    .and_then(|s| {
                                        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
                                        if end == 0 { None } else { s[..end].parse().ok() }
                                    })
                            };
                            info!(
                                "MQTT sniff mode command: {}",
                                frame_count.map_or("OFF".into(), |n| alloc::format!("{} frames", n))
                            );
                            cmd_sender.send(Command::Sniff(frame_count)).await;
                            continue;
                        }

                        // Handle reboot command
                        if topic.strip_prefix(&cmd_base) == Some("/reboot") {
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
                            }
                        } else {
                            let payload_str =
                                core::str::from_utf8(&payload).unwrap_or("<non-utf8>");
                            warn!(
                                "MQTT command not recognized: topic={} payload={}",
                                topic, payload_str
                            );
                        }
                    }
                    None => {
                        let reason = mqtt
                            .last_disconnect
                            .take()
                            .unwrap_or_else(|| alloc::string::String::from("unknown"));
                        warn!("MQTT connection lost ({}), attempting reconnect...", reason);
                        MQTT_RECONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
                        MQTT_LOSS_COUNT.fetch_add(1, Ordering::Relaxed);
                        let celsius = is_celsius(&last_scale_range);
                        let last_state = build_last_state(
                            &last_published_status,
                            &last_published_fault,
                            last_sniff_mode,
                            last_wifi_rssi,
                            last_registration_state,
                        );
                        crate::net_util::reconnect_with_backoff(
                            &mut mqtt,
                            celsius,
                            last_state.as_ref(),
                            "mqtt_reconnect_loop",
                            "MQTT reconnect exceeded 30 attempts, resetting device",
                        )
                        .await;
                        crate::MQTT_CONNECTED_SIGNAL.signal(());
                    }
                }
            }
            Either::Second(_) => {
                // Timer expired — loop back to check channels above.
                // This ensures self-test status updates and other channel
                // data are published even when no MQTT messages arrive.

            }
        }

        // Bump the watchdog tick so the main loop can detect if this task freezes.
        MQTT_TASK_TICK.fetch_add(1, Ordering::Relaxed);
    }
}
