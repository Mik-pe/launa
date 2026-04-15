//! Launa ESP32 spa controller firmware.
//!
//! Embassy-based async runtime over esp-hal (pure Rust, no_std).
//! Reads Balboa spa protocol over RS-485 UART, publishes state to
//! Home Assistant via MQTT over WiFi.
//!
//! Commands are only sent on the RS-485 bus when the spa sends a Ready
//! message, per the Balboa protocol requirements.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::{Read as _, Write as _};
use launa_core::{AppAction, SpaApp};
use launa_protocol::command::Command;
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_protocol::status::StatusUpdate;
use launa_mqtt::topics::TopicBuilder;
use log::{debug, error, info, warn};

mod clock;
mod config;
mod macros;
mod mqtt_client;
mod net_util;
mod ota;
mod transport;
mod wifi;

use esp_backtrace as _;

// ── Diagnostic counters (static, accessible from all tasks) ───────────

static MQTT_RECONNECT_COUNT: AtomicU32 = AtomicU32::new(0);
static MQTT_LOSS_COUNT: AtomicU32 = AtomicU32::new(0);

/// Boot timestamp in seconds (lower 32 bits of millis/1000), set once in main().
/// Used for uptime calculation. AtomicU32 is used because AtomicU64 is not
/// available on xtensa-esp32-none-elf. A u32 seconds counter wraps at ~136 years.
static DIAGNOSTICS_START_SECS: AtomicU32 = AtomicU32::new(0);

/// Compute uptime in seconds from the boot timestamp.
fn uptime_secs() -> u64 {
    let start = DIAGNOSTICS_START_SECS.load(Ordering::Relaxed);
    if start == 0 {
        return 0;
    }
    let now = (Instant::now().as_millis() / 1000) as u32;
    now.saturating_sub(start) as u64
}

// Heap allocator: 32 KiB (initialized in main)
fn init_heap() {
    const HEAP_SIZE: usize = 32 * 1024;
    static mut HEAP: core::mem::MaybeUninit<[u8; HEAP_SIZE]> = core::mem::MaybeUninit::uninit();
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            HEAP.as_mut_ptr() as *mut u8,
            HEAP_SIZE,
            esp_alloc::MemoryCapability::Internal.into(),
        ));
    }
}

// ── Inter-task channels ────────────────────────────────────────────────

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, Frame, 4> = Channel::new();
static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, Command, 4> = Channel::new();
static UART_TX_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();
static STATE_CHANNEL: Channel<CriticalSectionRawMutex, (StatusUpdate, Option<alloc::string::String>, bool), 2> = Channel::new();
static PUMP_TIMER_CHANNEL: Channel<CriticalSectionRawMutex, (u8, u32), 4> = Channel::new();
static DIAGNOSTICS_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 2> = Channel::new();
static OTA_CHANNEL: Channel<CriticalSectionRawMutex, alloc::string::String, 1> = Channel::new();

/// Signal set when WiFi reconnects after a disconnect. MQTT task checks this
/// to force a clean MQTT reconnect (old TCP socket may be stale).
pub static WIFI_RECONNECT_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Channel for sending alert payloads from the main loop to the MQTT task.
static ALERT_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();

// ── Combined UART task (reads frames + writes outgoing bytes) ──────────

#[embassy_executor::task]
async fn uart_task(mut transport: transport::Rs485Transport) {
    let mut decoder = FrameDecoder::new();
    let frame_sender = FRAME_CHANNEL.sender();
    let uart_rx = UART_TX_CHANNEL.receiver();
    let mut buf = [0u8; 128];

    info!("UART task started");

    loop {
        // Check for outgoing data first (prioritize writes)
        if let Ok(data) = uart_rx.try_receive() {
            if let Err(e) = transport.write_all(&data).await {
                error!("UART write error: {:?}", e);
            }
        }

        // Read from UART
        match transport.read(&mut buf).await {
            Ok(n) if n > 0 => {
                for &byte in &buf[..n] {
                    if let Some(frame) = decoder.feed(byte) {
                        frame_sender.send(frame).await;
                    }
                }
            }
            Ok(_) => {
                Timer::after(Duration::from_millis(1)).await;
            }
            Err(e) => {
                error!("UART read error: {:?}", e);
                Timer::after(Duration::from_millis(10)).await;
            }
        }
    }
}

// ── MQTT task (subscribes to commands + publishes state) ──────────────

#[embassy_executor::task]
async fn mqtt_task(mut mqtt: mqtt_client::MqttClient) {
    let cmd_sender = COMMAND_CHANNEL.sender();
    let state_rx = STATE_CHANNEL.receiver();
    let diag_rx = DIAGNOSTICS_CHANNEL.receiver();
    let alert_rx = ALERT_CHANNEL.receiver();
    let ota_tx = OTA_CHANNEL.sender();
    let topics = TopicBuilder::new(&mqtt.device_id);
    let diag_topic = topics.diagnostics_topic();
    let cmd_base = topics.command_topic();
    let alert_topic = topics.alert_topic();
    let mut last_scale_range: Option<(launa_protocol::status::TemperatureScale, launa_protocol::status::TempRange)> = None;

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
                        let _ = mqtt.publish_availability(true).await;
                        let _ = mqtt.publish_discovery().await;
                        let _ = mqtt.subscribe_commands().await;
                        break;
                    }
                    Err(e) => {
                        // Exponential backoff: 5s, 10s, 20s, 40s, 60s, 60s, ...
                        let backoff_secs = if wifi_attempt > 10 { 60 } else { 5u64 << (wifi_attempt.min(4) - 1).min(4) };
                        // min of backoff and 60: the shift gives 5,10,20,40,80... so cap at 60
                        let backoff_secs = backoff_secs.min(60);
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
                        if wifi_attempt >= 10 {
                            error!("WiFi reconnect exceeded 10 attempts, continuing at max backoff");
                        }
                        Timer::after(Duration::from_secs(backoff_secs)).await;
                    }
                }
            }
        }

        // Check for diagnostics payloads to publish (non-blocking)
        if let Ok(diag_payload) = diag_rx.try_receive() {
            if let Err(e) = mqtt.publish(&diag_topic, &diag_payload, 0, false).await {
                warn!("MQTT diagnostics publish failed: {:?}", e);
            }
            continue;
        }

        // Check for alert payloads to publish (non-blocking)
        if let Ok(alert_payload) = alert_rx.try_receive() {
            if let Err(e) = mqtt.publish(&alert_topic, &alert_payload, 1, false).await {
                warn!("MQTT alert publish failed: {:?}", e);
            }
            continue;
        }

        // Check for state updates to publish (non-blocking)
        if let Ok((status, fault, is_stale)) = state_rx.try_receive() {
            last_scale_range = Some((status.temperature_scale, status.temp_range));
            if let Err(e) = mqtt.publish_state(&status, fault.as_deref()).await {
                warn!("MQTT state publish failed: {:?}", e);
            }
            if is_stale {
                if let Err(e) = mqtt.publish_availability_stale().await {
                    warn!("MQTT stale availability publish failed: {:?}", e);
                }
            } else {
                // Status received after being stale — publish recovery
                let _ = mqtt.publish_availability(true).await;
            }
            continue;
        }

        // Check for incoming MQTT messages
        match mqtt.recv().await {
            Some((topic, payload)) => {
                debug!("MQTT received: {} ({} bytes)", topic, payload.len());

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
                        let _ = mqtt.publish_discovery().await;
                        let _ = mqtt.publish_availability(true).await;
                    }
                    continue;
                }

                // Handle commands and pump timers
                let (scale, range) = match last_scale_range {
                    Some((s, r)) => (Some(s), Some(r)),
                    None => (None, None),
                };
                if let Some(action) = mqtt_client::parse_command(&cmd_base, &topic, &payload, scale, range) {
                    match action {
                        mqtt_client::MqttAction::Command(cmd) => {
                            info!("MQTT command: {:?}", cmd);
                            cmd_sender.send(cmd).await;
                        }
                        mqtt_client::MqttAction::StartPumpTimer { pump, minutes } => {
                            info!("MQTT pump timer: pump {} for {} min", pump, minutes);
                            PUMP_TIMER_CHANNEL.send((pump, minutes)).await;
                        }
                    }
                }
            }
            None => {
                warn!("MQTT connection lost, attempting reconnect...");
                MQTT_RECONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
                MQTT_LOSS_COUNT.fetch_add(1, Ordering::Relaxed);
                let mut attempt: u32 = 0;
                let mut last_alert_time: Option<Instant> = None;
                loop {
                    attempt += 1;
                    match mqtt.reconnect().await {
                        Ok(()) => {
                            info!("MQTT reconnected, re-publishing...");
                            let _ = mqtt.publish_availability(true).await;
                            let _ = mqtt.publish_discovery().await;
                            let _ = mqtt.subscribe_commands().await;
                            break;
                        }
                        Err(e) => {
                            // Exponential backoff: 5s, 10s, 20s, 40s, 60s, 60s, ...
                            let backoff_secs = (5u64 << attempt.saturating_sub(1).min(4)).min(60);
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
                            if attempt >= 10 {
                                error!("MQTT reconnect exceeded 10 attempts, continuing at max backoff");
                            }
                            Timer::after(Duration::from_secs(backoff_secs)).await;
                        }
                    }
                }
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────



/// Build a diagnostics JSON payload with all counters and publish via the
/// diagnostics channel. Uses SpaApp's internal counters for frames/retries/drops
/// and the static counters for MQTT/WiFi reconnects.
fn publish_diagnostics(
    device_id: &str,
    uptime_secs: u64,
    frames_received: u32,
    command_retries: u32,
    command_drops: u32,
) {
    let mqtt_reconnects = MQTT_RECONNECT_COUNT.load(Ordering::Relaxed);
    let mqtt_losses = MQTT_LOSS_COUNT.load(Ordering::Relaxed);
    let heap_free = esp_alloc::HEAP.free();

    let json = alloc::format!(
        r#"{{"device_id":"{}","uptime_secs":{},"mqtt_reconnect_count":{},"mqtt_loss_count":{},"command_retry_count":{},"command_drop_count":{},"frames_received":{},"heap_free":{}}}"#,
        device_id,
        uptime_secs,
        mqtt_reconnects,
        mqtt_losses,
        command_retries,
        command_drops,
        frames_received,
        heap_free,
    );

    debug!("Diagnostics: {}", json);

    // Try to send non-blocking; if the channel is full, the diagnostics
    // update is simply skipped (it will be published next cycle).
    let payload = Vec::from(json.as_bytes());
    let _ = DIAGNOSTICS_CHANNEL.try_send(payload);
}

/// Format and send an alert through the alert channel.
/// Called from the main loop for conditions requiring operator attention.
fn send_alert(level: &str, message: &str) {
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

/// Execute a batch of `AppAction` side effects from `SpaApp`.
///
/// Maps each action to the corresponding IO operation (UART send, MQTT publish, etc.).
async fn execute_actions(actions: &[AppAction], device_id: &str) {
    for action in actions {
        match action {
            AppAction::SendFrame(bytes) => {
                UART_TX_CHANNEL.send(bytes.clone()).await;
            }
            AppAction::PublishState {
                status,
                fault,
                recovering_from_stale,
            } => {
                let _ = STATE_CHANNEL.try_send((status.clone(), fault.clone(), *recovering_from_stale));
            }
            AppAction::PublishStaleAvailability => {
                // Stale availability is handled by the MQTT task when it sees
                // a STATE_CHANNEL message with is_stale=true.
            }
            AppAction::PublishAlert { level, message } => {
                send_alert(level, message);
            }
            AppAction::PublishDiagnostics {
                uptime_secs,
                frames_received,
                command_retries,
                command_drops,
            } => {
                publish_diagnostics(device_id, *uptime_secs, *frames_received, *command_retries, *command_drops);
            }
            AppAction::RequestOta { url } => {
                let _ = OTA_CHANNEL.try_send(url.clone());
            }
            AppAction::PublishAvailability { .. } | AppAction::PublishDiscovery => {
                // These are handled by the MQTT task on initial connect, not emitted by SpaApp.
            }
        }
    }
}

// ── Sniffer mode (passive RS-485 monitoring) ──────────────────────────

#[cfg(feature = "sniff")]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    init_heap();
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("Launa ESP32 sniffer mode starting...");

    // ── Load config from NVS ────────────────────────────────────────
    let mut nvs = config::AppConfig::open_nvs(peripherals.FLASH);
    let app_config = config::AppConfig::load(&mut nvs);
    let device_id = app_config.device_id.clone();
    info!("Config loaded: device_id={}", device_id);

    // ── Initialize RS-485 UART ──────────────────────────────────────
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("Failed to create UART")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    let mut transport = transport::Rs485Transport::new(uart, Some(peripherals.GPIO4.into()));
    info!("RS-485 UART initialized");

    // ── Initialize esp-radio and connect WiFi ──────────────────────
    let radio_ctrl = esp_radio::init().expect("Failed to init esp-radio");
    let wifi_stack = wifi::WifiStack::connect(
        spawner,
        radio_ctrl,
        peripherals.WIFI,
        esp_hal::rng::Rng::new(),
        &app_config.wifi_ssid,
        &app_config.wifi_password,
    )
    .await;

    // ── Connect MQTT ────────────────────────────────────────────────
    let mut mqtt = match mqtt_client::MqttClient::connect(wifi_stack.stack, &app_config).await {
        Ok(m) => m,
        Err(e) => {
            error!("MQTT connect failed: {:?}", e);
            panic!("MQTT connect failed")
        }
    };

    let _ = mqtt.publish_availability(true).await;
    let _ = mqtt.subscribe_commands().await;

    let topics = TopicBuilder::new(&device_id);
    let sniff_topic = topics.sniff_topic();

    info!("Sniffer mode active - listening passively on RS-485");

    let mut decoder = FrameDecoder::new();
    let mut buf = [0u8; 256];

    loop {
        match transport.read(&mut buf).await {
            Ok(n) if n > 0 => {
                let frames = decoder.feed_slice(&buf[..n]);
                for frame in &frames {
                    let hex: alloc::string::String = frame.payload.iter()
                        .map(|b| alloc::format!("{:02X}", b))
                        .collect();
                    let mt = alloc::format!("{:02X}{:02X}", frame.message_type[0], frame.message_type[1]);

                    // Re-parse to get CRC status
                    let crc_ok = Frame::parse(&frame.payload).is_ok();

                    let json = alloc::format!(
                        r#"{{"raw":"{}","type":"{}","len":{},"crc_ok":{}}}"#,
                        hex, mt, frame.payload.len(), crc_ok
                    );
                    info!("Sniff: {}", json);
                    let _ = mqtt.publish(&sniff_topic, json.as_bytes(), 0, false).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!("Sniffer read error: {:?}", e);
                Timer::after(Duration::from_millis(100)).await;
            }
        }
    }
}

// ── Hardware test mode ─────────────────────────────────────────────────

#[cfg(feature = "hw-test")]
#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    init_heap();
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("HW test mode");

    // Test 1: UART
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let mut uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("Failed to create UART")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();
    info!("TEST_PASS:uart_init");

    // Test 2: Timer
    Timer::after(Duration::from_millis(100)).await;
    info!("TEST_PASS:timer");

    // Test 3: Heap
    let free = esp_alloc::HEAP.free();
    if free > 1000 {
        info!("TEST_PASS:heap_free={}", free);
    } else {
        info!("TEST_FAIL:heap_low={}", free);
    }

    info!("TEST_PASS:all");

    // ── Serial config receiver ────────────────────────────────────
    // Wait for CONFIG_START over serial, parse key=value lines,
    // write to NVS on CONFIG_END. 30-second timeout.
    info!("Waiting for serial config (30s timeout)...");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut line_buf: Vec<u8> = Vec::new();
    let mut read_buf = [0u8; 64];
    let mut config_started = false;
    let mut kv_pairs: Vec<(alloc::string::String, alloc::string::String)> = Vec::new();
    let mut config_done = false;

    while Instant::now() < deadline && !config_done {
        match uart.read(&mut read_buf) {
            Ok(0) => {
                Timer::after(Duration::from_millis(10)).await;
            }
            Ok(n) => {
                for &byte in &read_buf[..n] {
                    if byte == b'\n' {
                        // Process complete line — extract as owned string before clearing buffer
                        let line = {
                            let raw = core::str::from_utf8(&line_buf).unwrap_or("");
                            // Trim CR/LF whitespace
                            let trimmed = raw.trim_start_matches('\r').trim_end_matches('\r');
                            alloc::string::String::from(trimmed)
                        };
                        line_buf.clear();

                        if !config_started {
                            if line == "CONFIG_START" {
                                config_started = true;
                                info!("Config reception started");
                            }
                        } else if line == "CONFIG_END" {
                            config_done = true;
                        } else if !line.is_empty() {
                            // Parse key=value
                            if let Some(eq_pos) = line.find('=') {
                                let key = &line[..eq_pos];
                                let value = &line[eq_pos + 1..];
                                kv_pairs.push((
                                    alloc::string::String::from(key),
                                    alloc::string::String::from(value),
                                ));
                            }
                        }
                    } else if byte != b'\r' {
                        line_buf.push(byte);
                    }
                }
            }
            Err(_) => {
                // No data available or read error — brief pause before retry
                Timer::after(Duration::from_millis(10)).await;
            }
        }
    }

    if !config_done {
        let msg: &[u8] = if !config_started {
            b"CONFIG_ERROR:timeout_no_start\n"
        } else {
            b"CONFIG_ERROR:timeout_no_end\n"
        };
        let _ = uart.write(msg);
        let _ = uart.flush();
        warn!("Config reception timed out");
        return;
    }

    // Map xtask dotted keys to AppConfig fields and save to NVS
    let mut app_config = config::AppConfig::default();

    for (key, value) in &kv_pairs {
        match key.as_str() {
            "wifi.ssid" => app_config.wifi_ssid = value.clone(),
            "wifi.password" => app_config.wifi_password = value.clone(),
            "mqtt.host" => app_config.mqtt_host = value.clone(),
            "mqtt.port" => {
                match value.parse::<u16>() {
                    Ok(p) => {
                        app_config.mqtt_port = p;
                    }
                    Err(_) => {
                        let msg = alloc::format!("CONFIG_ERROR:invalid_port={}\n", value);
                        let _ = uart.write(msg.as_bytes());
                        let _ = uart.flush();
                        warn!("Invalid port: {}", value);
                        return;
                    }
                }
            }
            "mqtt.user" => app_config.mqtt_user = value.clone(),
            "mqtt.password" => app_config.mqtt_password = value.clone(),
            "device.id" => app_config.device_id = value.clone(),
            other => {
                warn!("Unknown config key: {}", other);
            }
        }
    }

    info!(
        "Parsed config: ssid={} mqtt={}:{} device={}",
        app_config.wifi_ssid, app_config.mqtt_host, app_config.mqtt_port, app_config.device_id
    );

    // Write to NVS
    let mut nvs = config::AppConfig::open_nvs(peripherals.FLASH);
    app_config.save(&mut nvs);

    let _ = uart.write(b"CONFIG_OK\n");
    let _ = uart.flush();
    info!("Config saved to NVS successfully");
}

// ── Main entry point ──────────────────────────────────────────────────

#[cfg(not(any(feature = "sniff", feature = "hw-test")))]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    use launa_ota::OtaUpdate;
    init_heap();
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Record boot timestamp for diagnostics uptime calculation
    DIAGNOSTICS_START_SECS.store((Instant::now().as_millis() / 1000) as u32, Ordering::Relaxed);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("Launa ESP32 firmware starting...");

    // ── Load config from NVS ────────────────────────────────────────
    let mut nvs = config::AppConfig::open_nvs(peripherals.FLASH);
    let app_config = config::AppConfig::load(&mut nvs);
    info!("Config loaded: device_id={}", app_config.device_id);
    // Recover flash from NVS for OTA use
    let flash = nvs.into_inner();

    // ── Initialize RS-485 UART ──────────────────────────────────────
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("Failed to create UART")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    let uart_transport = transport::Rs485Transport::new(uart, Some(peripherals.GPIO4.into()));
    info!("RS-485 UART initialized");

    // ── Initialize esp-radio and connect WiFi ──────────────────────
    let radio_ctrl = esp_radio::init().expect("Failed to init esp-radio");
    let wifi_stack = wifi::WifiStack::connect(
        spawner,
        radio_ctrl,
        peripherals.WIFI,
        esp_hal::rng::Rng::new(),
        &app_config.wifi_ssid,
        &app_config.wifi_password,
    )
    .await;

    // ── Connect MQTT ────────────────────────────────────────────────
    let mut mqtt = match mqtt_client::MqttClient::connect(wifi_stack.stack, &app_config).await {
        Ok(m) => m,
        Err(e) => {
            error!("MQTT connect failed: {:?}", e);
            panic!("MQTT connect failed")
        }
    };

    let _ = mqtt.publish_availability(true).await;
    let _ = mqtt.publish_discovery().await;
    let _ = mqtt.subscribe_commands().await;

    // Mark firmware as valid (boot successful: WiFi + MQTT connected).
    let mut ota = ota::create_ota(flash);
    if let Err(e) = ota.mark_valid() {
        warn!("Failed to mark firmware valid: {:?}", e);
    } else {
        info!("Firmware marked valid (boot validation passed)");
    }

    // Allocate OTA TCP socket buffers once — reused across attempts to avoid
    // leaking 5 KiB of static memory on each failed OTA.
    let mut ota_buffers = ota::OtaBuffers::new();

    // Spawn background tasks
    spawner
        .spawn(mqtt_task(mqtt))
        .expect("Failed to spawn MQTT task");
    spawner
        .spawn(uart_task(uart_transport))
        .expect("Failed to spawn UART task");

    // ── Main event loop ─────────────────────────────────────────────
    info!("Entering main event loop");

    let frame_rx = FRAME_CHANNEL.receiver();
    let cmd_rx = COMMAND_CHANNEL.receiver();
    let pump_timer_rx = PUMP_TIMER_CHANNEL.receiver();
    let ota_rx = OTA_CHANNEL.receiver();

    let clock = clock::EmbassyClock::new();
    let mut app = SpaApp::new(&app_config.device_id, &clock);
    let device_id_str: &str = &app_config.device_id;

    loop {
        // Wait for a frame from the UART task
        let frame = frame_rx.receive().await;
        let actions = app.process_frame(&frame);
        execute_actions(&actions, device_id_str).await;

        // Drain all available frames
        while let Ok(frame) = frame_rx.try_receive() {
            let actions = app.process_frame(&frame);
            execute_actions(&actions, device_id_str).await;
        }

        // Drain MQTT commands into SpaApp's command queue
        while let Ok(cmd) = cmd_rx.try_receive() {
            let actions = app.on_mqtt_command(cmd);
            execute_actions(&actions, device_id_str).await;
        }

        // Periodic tick: stale detection, registration timeout, diagnostics
        let tick_actions = app.tick();
        execute_actions(&tick_actions, device_id_str).await;

        // Heap check (uses actual ESP32 free heap)
        let heap_actions = app.check_heap(esp_alloc::HEAP.free());
        execute_actions(&heap_actions, device_id_str).await;

        // ── OTA update handling ─────────────────────────────────────
        if let Ok(firmware_url) = ota_rx.try_receive() {
            info!("OTA: starting firmware download from main loop");
            if let Err(()) = ota::perform_ota_update(wifi_stack.stack, &mut ota, &firmware_url, &mut ota_buffers).await {
                error!("OTA update failed");
                send_alert("error", "ota_update_failed");
            }
            // If we get here without resetting, something went very wrong
            error!("OTA: device did not reset after update, rolling back");
            let _ = ota.rollback_and_reboot();
            esp_hal::system::software_reset();
        }

        // Drain pump timer commands
        while let Ok((pump_index, minutes)) = pump_timer_rx.try_receive() {
            let actions = app.start_pump_timer(pump_index, minutes);
            execute_actions(&actions, device_id_str).await;
            info!("Started pump {} timer for {} min", pump_index, minutes);
        }
    }
}
