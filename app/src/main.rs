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

use esp_alloc as _;
use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::ram;
use launa_core::{AppAction, SpaApp};
use launa_hal::Transport as _;
use launa_ota::OtaUpdate;
use launa_protocol::command::Command;
use launa_protocol::frame::{Frame, FrameDecoder};
use log::{debug, error, info, warn};

use diagnostics::{publish_diagnostics, send_alert};
use types::FaultBuf;

mod clock;
mod config;
mod crash_info;
mod crypto;
mod diagnostics;
mod logger;
mod macros;
mod mqtt_client;
mod net_util;
mod ota;
#[cfg(feature = "remote-log")]
mod remote_log;
mod serial_config;
mod sniff;
mod transport;
mod types;
mod uart_raw;
mod wifi;

mod panic;
mod rate_log;

/// Firmware version embedded at compile time from Cargo.toml [package].version
/// plus the Git short SHA from build.rs. Produces e.g. `"0.1.0 (abc1234)"`.
/// Used in HA discovery (sw_version), MQTT state JSON, and diagnostics payload.
const FIRMWARE_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_SHORT_SHA"), ")");

/// Random boot identifier generated once per boot. Published in the availability
/// payload so the web GUI can detect device reboots and clear stale state.
static BOOT_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Get the boot_id, generating it on first access from the hardware RNG.
fn boot_id() -> u32 {
    let id = BOOT_ID.load(Ordering::Relaxed);
    if id != 0 {
        return id;
    }
    // Generate a random non-zero boot_id
    let rng = esp_hal::rng::Rng::new();
    let mut new_id: u32;
    loop {
        new_id = rng.random();
        if new_id != 0 {
            break;
        }
    }
    BOOT_ID.store(new_id, Ordering::Relaxed);
    new_id
}

static MQTT_RECONNECT_COUNT: AtomicU32 = AtomicU32::new(0);
static MQTT_LOSS_COUNT: AtomicU32 = AtomicU32::new(0);
static FRAME_ERROR_COUNT: AtomicU32 = AtomicU32::new(0);
static UART_BYTES_RECEIVED: AtomicU32 = AtomicU32::new(0);
static UART_FIRST_BYTE_SEEN: AtomicBool = AtomicBool::new(false);
static UART_LAST_NO_BYTE_ALERT_SECS: AtomicU32 = AtomicU32::new(0); // uptime when last alert sent

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

/// Pre-computed client hash for registration fast-path (stored as big-endian u16).
/// Set once at startup from device_id via FNV-1a.
static REG_CLIENT_HASH: AtomicU16 = AtomicU16::new(0);

/// Whether the SpaApp is currently registered with the spa controller.
/// Updated by the main loop after processing frames/ticks; read by the
/// UART fast-path to avoid sending NewClientResponse when already registered
/// (which would cause the spa to assign a new channel every ~1 second).
static APP_REGISTERED: AtomicBool = AtomicBool::new(false);

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, Frame, 4> = Channel::new();
static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, Command, 4> = Channel::new();
static UART_TX_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();
static STATE_CHANNEL: Channel<CriticalSectionRawMutex, types::StateMessage, 4> = Channel::new();
static PUMP_TIMER_CHANNEL: Channel<CriticalSectionRawMutex, (u8, u32), 4> = Channel::new();
static DIAGNOSTICS_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 2> = Channel::new();
static OTA_CHANNEL: Channel<CriticalSectionRawMutex, alloc::string::String, 1> = Channel::new();

/// Signal set when WiFi reconnects after a disconnect. MQTT task checks this
/// to force a clean MQTT reconnect (old TCP socket may be stale).
pub static WIFI_RECONNECT_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Signal set by mqtt_task after the first successful connect+sync.
/// Main loop waits on this before proceeding to normal operation.
pub static MQTT_CONNECTED_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Channel for sending alert payloads from the main loop to the MQTT task.
static ALERT_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();

#[embassy_executor::task]
async fn uart_task(mut transport: transport::Rs485Transport) {
    static UART_READ_ERR: launa_core::RateLog = launa_core::RateLog::new();
    static UART_WRITE_ERR: launa_core::RateLog = launa_core::RateLog::new();

    let mut decoder = FrameDecoder::new();
    let frame_sender = FRAME_CHANNEL.sender();
    let uart_rx = UART_TX_CHANNEL.receiver();
    let mut buf = [0u8; 128];
    let mut first_bytes_logged = false;
    let mut sniff = sniff::SniffState::new();

    info!("UART task started (async half-duplex)");

    loop {
        sniff.check_start();

        // ── RECEIVE ────────────────────────────────────────────────────────
        // Half-duplex RS-485: read with short timeout. If bytes arrive, decode.
        // On timeout, bus is idle — fall through to TX.
        let read_result = select(
            transport.read(&mut buf),
            Timer::after(Duration::from_micros(200)),
        )
        .await;

        match read_result {
            Either::First(Ok(n)) if n > 0 => {
                UART_BYTES_RECEIVED.fetch_add(n as u32, Ordering::Relaxed);

                if !first_bytes_logged {
                    first_bytes_logged = true;
                    UART_FIRST_BYTE_SEEN.store(true, Ordering::Relaxed);
                    let hex_dump: Vec<u8> = buf[..n.min(16)].to_vec();
                    let hex_str = launa_protocol::hex::to_hex(&hex_dump);
                    info!("UART: first {} bytes from spa bus: {}", n, hex_str);
                }

                // Record raw RX bytes for sniff capture before decoding
                if sniff.record_chunk(sniff::Direction::Rx, &buf[..n]) {
                    sniff.finish();
                }

                let prev_errors = decoder.frame_error_count();
                for &byte in &buf[..n] {
                    if let Some(frame) = decoder.feed(byte) {
                        // Fast-path: when we see a NewClientQuery (FE BF 00),
                        // send the NewClientResponse IMMEDIATELY. This bypasses
                        // the idle-gap queue and avoids the ~20ms latency from
                        // the 200us read timeout cycle. Critical for RS-485
                        // half-duplex where the emulator only listens briefly.
                        //
                        // Only respond when NOT already registered — the spa
                        // sends FE BF 00 every ~1s to discover new clients.
                        // If we're registered, responding would cause the spa
                        // to assign a new channel every cycle.
                        if frame.message_type == [0xFE, 0xBF]
                            && frame.payload.first() == Some(&0x00)
                            && !APP_REGISTERED.load(Ordering::Relaxed)
                        {
                            let hash_be = REG_CLIENT_HASH.load(Ordering::Relaxed);
                            let hash_hi = (hash_be >> 8) as u8;
                            let hash_lo = (hash_be & 0xFF) as u8;
                            let response = launa_protocol::registration::RegistrationMessage::NewClientResponse {
                                device_type: 0x02,
                                client_hash: [hash_hi, hash_lo],
                            };
                            if let Ok(encoded) = response.encode() {
                                info!("REG fast-path: sending NewClientResponse immediately");
                                // Record the fast-path TX response in sniff capture
                                if sniff.record_chunk(sniff::Direction::Tx, &encoded) {
                                    sniff.finish();
                                }
                                if transport.write(&encoded).await.is_err() {
                                    rate_error!(UART_WRITE_ERR, "UART write error: Io");
                                }
                            }
                        }

                        if frame.message_type != [0x10, 0xBF] {
                            info!(
                                "UART: decoded frame type={:02X}{:02X} len={}",
                                frame.message_type[0],
                                frame.message_type[1],
                                frame.payload.len()
                            );
                        }

                        frame_sender.send(frame).await;
                    }
                }
                let new_errors = decoder.frame_error_count();
                if new_errors > prev_errors {
                    FRAME_ERROR_COUNT
                        .fetch_add(new_errors - prev_errors, Ordering::Relaxed);
                }

                continue;
            }
            Either::First(Ok(_)) => {} // 0 bytes — proceed to TX check
            Either::First(Err(_)) => {
                rate_error!(UART_READ_ERR, "UART read error: Io");
                Timer::after(Duration::from_millis(1)).await;
                continue;
            }
            Either::Second(_) => {} // Timeout — bus is idle, proceed to TX
        }

        // ── TRANSMIT (bus idle) ────────────────────────────────────────────
        // TX from main loop (commands, registration responses via SpaApp)
        if let Ok(data) = uart_rx.try_receive() {
            // Record raw TX bytes for sniff capture before writing
            if sniff.record_chunk(sniff::Direction::Tx, &data) {
                sniff.finish();
            }
            let result = transport.write(&data).await;
            info!("UART TX: {} bytes", data.len());
            if result.is_err() {
                rate_error!(UART_WRITE_ERR, "UART write error: Io");
            }
        }

        Timer::after(Duration::from_micros(100)).await;
    }
}

mod mqtt_task;

/// Read the current WiFi RSSI from the shared atomic.
/// Returns `None` if not connected (value is `i32::MIN`).
fn read_wifi_rssi() -> Option<i32> {
    let rssi = wifi::WIFI_RSSI.load(Ordering::Relaxed);
    if rssi == i32::MIN {
        None
    } else {
        Some(rssi)
    }
}

/// Execute a batch of `AppAction` side effects from `SpaApp`.
///
/// Maps each action to the corresponding IO operation (UART send, MQTT publish, etc.).
async fn execute_actions(
    actions: &[AppAction],
    device_id: &str,
    sniff_mode: bool,
    wifi_rssi: Option<i32>,
) {
    for action in actions {
        match action {
            AppAction::SendFrame(bytes) => {
                let _ = UART_TX_CHANNEL.try_send(bytes.clone());
            }
            AppAction::PublishState {
                status,
                fault,
                recovering_from_stale,
                registration_state,
            } => {
                let fb = fault
                    .as_ref()
                    .map_or(FaultBuf::EMPTY, |s| FaultBuf::from_string(s));
                if STATE_CHANNEL
                    .try_send(types::StateMessage {
                        status: status.clone(),
                        fault: fb,
                        recovering_from_stale: *recovering_from_stale,
                        sniff_mode,
                        wifi_rssi,
                        registration_state,
                    })
                    .is_err()
                {
                    debug!("STATE_CHANNEL full, dropping stale state update");
                }
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
                unregistered_frames,
                command_retries,
                command_drops,
                registration_state,
                frame_errors: _,
                uart_bytes: _,
            } => {
                let frame_errors = FRAME_ERROR_COUNT.load(Ordering::Relaxed);
                let uart_bytes = UART_BYTES_RECEIVED.load(Ordering::Relaxed);
                let uart_active = u32::from(UART_FIRST_BYTE_SEEN.load(Ordering::Relaxed));
                publish_diagnostics(
                    device_id,
                    *uptime_secs,
                    *frames_received,
                    *unregistered_frames,
                    *command_retries,
                    *command_drops,
                    frame_errors,
                    uart_bytes,
                    registration_state,
                    uart_active,
                );
            }
            AppAction::RequestOta { url } => {
                if OTA_CHANNEL.try_send(url.clone()).is_err() {
                    warn!("OTA channel full, dropping URL: {:?}", url);
                    send_alert("error", "ota_channel_full");
                }
            }
            AppAction::PublishAvailability { .. } | AppAction::PublishDiscovery => {
                // These are handled by the MQTT task on initial connect, not emitted by SpaApp.
            }
        }
    }
}

/// Handle an incoming MQTT command, routing through SpaApp.
async fn handle_mqtt_command(
    cmd: Command,
    app: &mut SpaApp<'_>,
    sniff_mode: &mut bool,
    device_id: &str,
) {
    match cmd {
        Command::Sniff(frame_count) => {
            if let Some(n) = frame_count {
                info!("Sniff burst capture requested: {} target chunks", n);
                *sniff_mode = true;
                sniff::SNIFF_CAPTURE.store(n, Ordering::Relaxed);
            } else {
                info!("Sniff burst capture OFF requested via MQTT");
                *sniff_mode = false;
                sniff::SNIFF_CAPTURE.store(0, Ordering::Relaxed);
            }
            let actions = app.force_publish();
            execute_actions(
                &actions,
                device_id,
                *sniff_mode,
                read_wifi_rssi(),
            )
            .await;
        }
        Command::Reboot => {
            info!("Remote reboot requested via MQTT, resetting in 1s...");
            // Brief delay to allow the log message and any pending MQTT acks to flush
            Timer::after(Duration::from_secs(1)).await;
            esp_hal::system::software_reset();
        }
        _ => {
            let actions = app.on_mqtt_command(cmd);
            execute_actions(
                &actions,
                device_id,
                *sniff_mode,
                read_wifi_rssi(),
            )
            .await;
        }
    }
}

/// Handle an OTA firmware request from the MQTT task.
///
/// Checks for a pending OTA URL and either runs a TCP connectivity test
/// (if the URL contains `?test=1`) or performs the full OTA update.
/// On successful OTA, the device resets. On failure, rolls back and resets.
async fn handle_ota_request<TG: esp_hal::timer::timg::TimerGroupInstance>(
    wifi_stack: &crate::wifi::WifiStack,
    ota: &mut Option<ota::EspOta>,
    ota_buffers: &mut Option<ota::OtaBuffers>,
    ota_rx: &embassy_sync::channel::Receiver<
        'static,
        CriticalSectionRawMutex,
        alloc::string::String,
        1,
    >,
    wdt: &mut esp_hal::timer::timg::Wdt<TG>,
) {
    let Ok(firmware_url) = ota_rx.try_receive() else {
        return;
    };

    if firmware_url.contains("?test=1") {
        info!("OTA: TCP test requested via ?test=1 parameter");
        match ota_buffers.as_mut() {
            Some(b) => match ota::tcp_test(wifi_stack.stack, &firmware_url, b).await {
                Ok(()) => info!("OTA: TCP test PASSED"),
                Err(()) => {
                    error!("OTA: TCP test FAILED");
                    send_alert("error", "tcp_test_failed");
                }
            },
            None => {
                warn!("TCP test requested but buffers unavailable");
                send_alert("error", "ota_unavailable_no_flash");
            }
        }
    } else {
        match (ota.as_mut(), ota_buffers.as_mut()) {
            (Some(o), Some(b)) => {
                info!("OTA: starting firmware download from main loop");
                if let Err(()) =
                    ota::perform_ota_update(wifi_stack.stack, o, &firmware_url, b, || wdt.feed())
                        .await
                {
                    error!("OTA update failed");
                    send_alert("error", "ota_update_failed");
                }
                error!("OTA: device did not reset after update, rolling back");
                let _ = o.rollback_and_reboot();
            }
            _ => {
                warn!("OTA requested but NVS/flash unavailable — cannot update");
                send_alert("error", "ota_unavailable_no_flash");
            }
        }
        esp_hal::system::software_reset();
    }
}

/// Process UART frames received during the event loop.
///
/// Process incoming UART frames through SpaApp for state updates and commands.
async fn process_uart_frames(
    frame: Frame,
    app: &mut SpaApp<'_>,
    device_id: &str,
    sniff_mode: bool,
    frame_rx: &embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, Frame, 4>,
) {
    let actions = app.process_frame(&frame);

    execute_actions(
        &actions,
        device_id,
        sniff_mode,
        read_wifi_rssi(),
    )
    .await;

    // Update registration state for UART fast-path
    APP_REGISTERED.store(app.is_registered(), Ordering::Relaxed);

    while let Ok(frame) = frame_rx.try_receive() {
        let actions = app.process_frame(&frame);
        execute_actions(
            &actions,
            device_id,
            sniff_mode,
            read_wifi_rssi(),
        )
        .await;
    }

    APP_REGISTERED.store(app.is_registered(), Ordering::Relaxed);
}

/// Connect to WiFi with fatal error handling.
///
/// On failure, logs the error, waits 5 seconds, and triggers a software reset.
async fn init_wifi(
    spawner: Spawner,
    wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
    rng: esp_hal::rng::Rng,
    app_config: &config::AppConfig,
) -> crate::wifi::WifiStack {
    match wifi::WifiStack::connect(
        spawner,
        wifi_peripheral,
        rng,
        &app_config.wifi_ssid,
        &app_config.wifi_password,
        &app_config.device_id,
    )
    .await
    {
        Ok(stack) => stack,
        Err(e) => {
            error!(
                "WiFi init failed: {:?} (free heap: {} bytes), resetting in 5s",
                e,
                esp_alloc::HEAP.free()
            );
            Timer::after(Duration::from_secs(5)).await;
            esp_hal::system::software_reset();
        }
    }
}

/// Reusable main event loop state. Used by both OTA validation and normal operation
/// so the same loop logic runs in both paths without duplication.
struct MainLoop<'a> {
    app: SpaApp<'static>,
    device_id: &'a str,
    sniff_mode: bool,
    serial_cfg: serial_config::SerialConfigReceiver,
    mqtt_last_tick: u32,
    mqtt_last_tick_time: Instant,
}

/// Static EmbassyClock instance. Safe because EmbassyClock is a zero-sized type
/// with no interior mutability — it simply wraps `embassy_time::Instant::now()`.
static CLOCK: clock::EmbassyClock = clock::EmbassyClock::new();

impl<'a> MainLoop<'a> {
    fn new(device_id: &'a str) -> Self {
        let app = SpaApp::new_from_device_id(&CLOCK, device_id);
        Self {
            app,
            device_id,
            sniff_mode: false,
            serial_cfg: serial_config::SerialConfigReceiver::new(),
            mqtt_last_tick: mqtt_task::MQTT_TASK_TICK.load(Ordering::Relaxed),
            mqtt_last_tick_time: Instant::now(),
        }
    }

    /// Run one iteration of the main event loop.
    /// Returns after processing one event or the 1-second tick timeout.
    async fn tick<TG: esp_hal::timer::timg::TimerGroupInstance>(
        &mut self,
        wdt: &mut esp_hal::timer::timg::Wdt<TG>,
        wifi_stack: &crate::wifi::WifiStack,
        ota: &mut Option<ota::EspOta>,
        ota_buffers: &mut Option<ota::OtaBuffers>,
        ota_rx: &embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, alloc::string::String, 1>,
    ) {
        let frame_rx = FRAME_CHANNEL.receiver();
        let cmd_rx = COMMAND_CHANNEL.receiver();
        let pump_timer_rx = PUMP_TIMER_CHANNEL.receiver();

        // Feed the hardware watchdog each iteration
        wdt.feed();

        // Check MQTT task health: if the tick counter hasn't changed in 120s,
        // the MQTT task is frozen (cooperative executor starvation). Threshold
        // accounts for reconnect backoff (up to 60s per attempt).
        let mqtt_tick = mqtt_task::MQTT_TASK_TICK.load(Ordering::Relaxed);
        if mqtt_tick != self.mqtt_last_tick {
            self.mqtt_last_tick = mqtt_tick;
            self.mqtt_last_tick_time = Instant::now();
        } else if self.mqtt_last_tick_time.elapsed().as_secs() >= 120 {
            warn!(
                "MQTT task appears frozen (tick unchanged for {}s)",
                self.mqtt_last_tick_time.elapsed().as_secs()
            );
            send_alert("error", "mqtt_task_frozen");
            // Reset the timer so we don't spam alerts every tick
            self.mqtt_last_tick_time = Instant::now();
        }

        let tick_interval = Duration::from_secs(1);

        // Multiplex: wait for a UART frame, an MQTT command, or the 1-second
        // tick timer — whichever fires first.
        match select3(
            frame_rx.receive(),
            cmd_rx.receive(),
            Timer::after(tick_interval),
        )
        .await
        {
            // UART frame received
            Either3::First(frame) => {
                process_uart_frames(
                    frame,
                    &mut self.app,
                    self.device_id,
                    self.sniff_mode,
                    &frame_rx,
                )
                .await;
            }
            // MQTT command received
            Either3::Second(cmd) => {
                handle_mqtt_command(
                    cmd,
                    &mut self.app,
                    &mut self.sniff_mode,
                    self.device_id,
                )
                .await;
            }
            // Tick timer expired
            Either3::Third(_) => {}
        }

        // Drain MQTT commands (non-blocking)
        while let Ok(cmd) = cmd_rx.try_receive() {
            handle_mqtt_command(
                cmd,
                &mut self.app,
                &mut self.sniff_mode,
                self.device_id,
            )
            .await;
        }

        // Auto-disable sniff_mode after burst capture completes.
        // The uart_task clears SNIFF_CAPTURE when the capture is done.
        if self.sniff_mode && sniff::SNIFF_CAPTURE.load(Ordering::Relaxed) == 0 {
            self.sniff_mode = false;
            let actions = self.app.force_publish();
            execute_actions(
                &actions,
                self.device_id,
                false,
                read_wifi_rssi(),
            )
            .await;
            info!("Sniff mode auto-disabled after burst capture");
        }

        // Periodic tick: stale detection, registration timeout, diagnostics
        let tick_actions = self.app.tick();
        execute_actions(
            &tick_actions,
            self.device_id,
            self.sniff_mode,
            read_wifi_rssi(),
        )
        .await;

        // Update registration state for UART fast-path (CTS loss may have reset it)
        APP_REGISTERED.store(self.app.is_registered(), Ordering::Relaxed);

        // Heap check (uses actual ESP32 free heap)
        let heap_actions = self.app.check_heap(esp_alloc::HEAP.free());
        execute_actions(
            &heap_actions,
            self.device_id,
            self.sniff_mode,
            read_wifi_rssi(),
        )
        .await;

        // UART health check: alert if no bytes received after 30s of uptime
        // Re-alert every 5 minutes until bytes are seen
        let uptime = uptime_secs();
        if !UART_FIRST_BYTE_SEEN.load(Ordering::Relaxed) && uptime >= 30 {
            let last_alert = UART_LAST_NO_BYTE_ALERT_SECS.load(Ordering::Relaxed) as u64;
            if uptime - last_alert >= 300 || last_alert == 0 {
                UART_LAST_NO_BYTE_ALERT_SECS.store(uptime as u32, Ordering::Relaxed);
                error!("UART: no bytes received after {}s — check RS-485 wiring (RX=GPIO16, TX=GPIO17)", uptime);
                send_alert("error", "no_uart_bytes");
            }
        }

        // Non-blocking serial config check: poll UART0 RX FIFO for config-flash data.
        // If a complete config is received, save to NVS and reboot. This replaces
        // the old blocking 5-second startup wait — config-flash now works at any time.
        if let Some(new_config) = self.serial_cfg.poll() {
            if let Some(ota_handler) = ota.take() {
                // Recover FlashStorage from OTA, create temp NVS, save config, reboot.
                let flash = ota_handler.into_flash();
                match esp_nvs::Nvs::new(0x9000, 0x6000, flash) {
                    Ok(mut nvs) => {
                        let mut aes = esp_hal::aes::Aes::new(unsafe { esp_hal::peripherals::AES::steal() });
                        let mut rng = esp_hal::rng::Rng::new();
                        new_config.save(&mut nvs, &mut aes, &mut rng);
                        info!("Serial config saved to NVS, rebooting...");
                    }
                    Err(e) => {
                        error!("Failed to open NVS for serial config save: {:?}", e);
                    }
                }
                esp_hal::system::software_reset();
            } else {
                warn!("Serial config received but OTA/NVS unavailable, ignoring");
            }
        }

        handle_ota_request(wifi_stack, ota, ota_buffers, ota_rx, wdt).await;

        // Drain pump timer commands
        while let Ok((pump_index, minutes)) = pump_timer_rx.try_receive() {
            let actions = self.app.start_pump_timer(pump_index, minutes);
            execute_actions(
                &actions,
                self.device_id,
                self.sniff_mode,
                read_wifi_rssi(),
            )
            .await;
            info!("Started pump {} timer for {} min", pump_index, minutes);
        }
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Initialize logger (uses raw UART0 registers, bypasses buggy ROM function)
    logger::init();

    // Initialize remote log capture (captures info/warn/error into ring buffer for MQTT)
    #[cfg(feature = "remote-log")]
    remote_log::init_remote_log();

    // Record boot timestamp for diagnostics uptime calculation
    DIAGNOSTICS_START_SECS.store(
        (Instant::now().as_millis() / 1000) as u32,
        Ordering::Relaxed,
    );

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // Configure TIMG1 as independent hardware watchdog (30s timeout)
    let timg1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG1);
    let mut wdt = timg1.wdt;
    wdt.set_timeout(
        esp_hal::timer::timg::MwdtStage::Stage0,
        esp_hal::time::Duration::from_secs(200),
    );
    wdt.enable();
    info!("Hardware watchdog enabled (200s timeout)");

    info!("Launa ESP32 firmware v{} starting...", FIRMWARE_VERSION);

    let app_config;
    let mut ota = None;
    let mut ota_buffers = None;
    let mut nvs_handle: Option<esp_nvs::Nvs<esp_storage::FlashStorage<'static>>> = None;
    let mut pending_crash_alarm: Option<crash_info::CrashInfo> = None;
    match config::AppConfig::open_nvs(peripherals.FLASH) {
        Some(mut nvs) => {
            let mut aes = esp_hal::aes::Aes::new(peripherals.AES);
            let mut rng = esp_hal::rng::Rng::new();
            app_config = config::AppConfig::load(&mut nvs, &mut aes, &mut rng);
            info!("Config loaded: device_id={}", app_config.device_id);

            // Read crash info from previous boot (written by panic handler).
            // Stored in memory for publishing after MQTT connects.
            pending_crash_alarm = crash_info::read_crash_info(&mut nvs);

            // Expose NVS handle to panic handler for crash recording.
            unsafe {
                crash_info::set_nvs_ptr(&mut nvs);
            }

            // Early serial config check: if NVS has placeholder credentials,
            // poll UART0 RX FIFO for config-flash data before proceeding.
            // This allows config-flash to work even when NVS is empty (device
            // can't reach main loop without valid WiFi credentials).
            // Runs for up to 5 seconds, checking every 100ms.
            let is_placeholder = app_config.wifi_ssid == config::AppConfig::PLACEHOLDER_SSID
                || app_config.wifi_password == config::AppConfig::PLACEHOLDER_WIFI_PASS;
            if is_placeholder {
                info!("Placeholder credentials detected, waiting for serial config (5s window)...");
                crash_info::clear_nvs_ptr(); // safe to clear before polling
                let mut serial_cfg = serial_config::SerialConfigReceiver::new();
                let start = embassy_time::Instant::now();
                loop {
                    if let Some(new_config) = serial_cfg.poll() {
                        new_config.save(&mut nvs, &mut aes, &mut rng);
                        info!("Serial config saved to NVS (early), rebooting...");
                        esp_hal::system::software_reset();
                    }
                    if start.elapsed().as_secs() >= 5 {
                        info!("No serial config received, proceeding with placeholder config");
                        break;
                    }
                    embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
                }
            }

            nvs_handle = Some(nvs);
        }
        None => {
            warn!("NVS unavailable — using default config, OTA disabled");
            app_config = config::AppConfig::default();
        }
    }

    // Recover flash from NVS for OTA use
    // Clear the NVS pointer before consuming the handle, and clear crash flag
    // from NVS now that we've read it into pending_crash_alarm.
    if let Some(mut nvs) = nvs_handle.take() {
        crash_info::clear_nvs_ptr();
        if pending_crash_alarm.is_some() {
            crash_info::clear_crash_info(&mut nvs);
        }
        let flash = nvs.into_inner();
        ota = Some(ota::create_ota(flash));
        ota_buffers = Some(ota::OtaBuffers::new());
    }

    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("Failed to create UART")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    // Auto-direction RS-485 transceiver (no DE pin).
    let uart_transport = transport::Rs485Transport::new(uart, None);
    info!("RS-485 UART initialized (auto-direction, no DE pin)");

    let wifi_stack = init_wifi(
        spawner,
        peripherals.WIFI,
        esp_hal::rng::Rng::new(),
        &app_config,
    )
    .await;

    // --- Config-poll window ---
    // Fast-poll UART0 for serial config data before spawning MQTT/UART tasks.
    // The main loop only polls once per ~1s tick, which is too slow to drain
    // the UART0 128-byte RX FIFO before it overflows with config lines sent
    // at 150ms intervals. This 10-second window at 10ms polling cadence ensures
    // config-flash reliably delivers the full config payload.
    {
        let mut cfg_rx = serial_config::SerialConfigReceiver::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        info!("Config-poll window open (10s)...");
        loop {
            if let Some(new_config) = cfg_rx.poll() {
                if let Some(ota_handler) = ota.take() {
                    let flash = ota_handler.into_flash();
                    match esp_nvs::Nvs::new(0x9000, 0x6000, flash) {
                        Ok(mut nvs) => {
                            let mut aes = esp_hal::aes::Aes::new(unsafe { esp_hal::peripherals::AES::steal() });
                            let mut rng = esp_hal::rng::Rng::new();
                            new_config.save(&mut nvs, &mut aes, &mut rng);
                            info!("Serial config saved to NVS, rebooting...");
                        }
                        Err(e) => {
                            error!("Failed to open NVS for serial config save: {:?}", e);
                        }
                    }
                    esp_hal::system::software_reset();
                }
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            Timer::after(Duration::from_millis(10)).await;
        }
        info!("Config-poll window closed");
    }

    // Create MqttClient on the stack. Socket buffers inside are pre-allocated
    // via mk_static! in new() — only the struct itself is stack-local, and it
    // moves into the embassy task on spawn.
    let mqtt = mqtt_client::MqttClient::new(wifi_stack.stack, &app_config, boot_id());

    // Derive client hash from device ID for RS-485 registration.
    // Must be set BEFORE spawning uart_task, which reads REG_CLIENT_HASH at
    // startup to pre-encode the registration response.
    let client_hash = launa_core::derive_client_hash(&app_config.device_id);
    info!("Client hash: {:02X}{:02X} (derived from device_id)", client_hash[0], client_hash[1]);
    REG_CLIENT_HASH.store(
        ((client_hash[0] as u16) << 8) | (client_hash[1] as u16),
        Ordering::Relaxed,
    );

    // Spawn background tasks on the ThreadModeExecutor.
    spawner.spawn(mqtt_task::mqtt_task(mqtt).unwrap());
    spawner.spawn(uart_task(uart_transport).unwrap());

    // Wait for mqtt_task to complete initial connection
    info!("Waiting for MQTT task to connect...");
    MQTT_CONNECTED_SIGNAL.wait().await;

    // Publish crash alarm from previous boot if present.
    if let Some(crash) = pending_crash_alarm.take() {
        let alarm_json = crash_info::crash_alarm_json(&crash, FIRMWARE_VERSION);
        let _ = ALERT_CHANNEL.try_send(alarm_json.into_bytes());
        info!("Crash alarm queued: reason={}", crash.reason.as_str());
    }

    let device_id_str: &str = &app_config.device_id;
    let ota_rx = OTA_CHANNEL.receiver();

    // --- OTA Validation (runs only when partition is freshly flashed) ---
    if ota.as_mut().is_some_and(|o| o.needs_validation()) {
        info!("OTA partition unvalidated, running boot validation...");
        let mut validation_loop = MainLoop::new(device_id_str);
        // Run a few ticks to prove core logic doesn't crash
        for _ in 0..3 {
            validation_loop.tick(&mut wdt, &wifi_stack, &mut ota, &mut ota_buffers, &ota_rx).await;
        }
        if let Some(ref mut o) = ota {
            if let Err(e) = o.mark_valid() {
                warn!("Failed to mark firmware valid: {:?}", e);
            } else {
                info!("Firmware marked valid (boot validation passed)");
            }
        }
    }

    // --- Normal operation ---
    info!("Entering main event loop");

    let mut main_loop = MainLoop::new(device_id_str);
    loop {
        main_loop.tick(&mut wdt, &wifi_stack, &mut ota, &mut ota_buffers, &ota_rx).await;
    }
}
