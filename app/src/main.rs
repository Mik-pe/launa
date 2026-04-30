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
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::ram;
use launa_core::{AppAction, SpaApp};
use launa_hal::Transport as _;
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
mod transport;
mod types;
mod uart_raw;
mod wifi;

mod rate_log;
mod self_test;

/// Custom panic handler: logs panic location, stores crash info to NVS,
/// waits for UART flush, then triggers a software reset.
/// Replaces esp-backtrace's default infinite loop to allow automatic recovery
/// from panics. Crash info is published via MQTT on next boot.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Write directly to UART0 registers — don't use the logger since
    // the panic might have occurred while holding the logger lock.

    // Print heap free first — uses only stack, no allocation.
    let heap_free = esp_alloc::HEAP.free();
    {
        let heap_msg = core::format_args!("\nHEAP free: {} bytes\n", heap_free);
        let mut heap_buf = [0u8; 48];
        let mut w = SliceWrite::new(&mut heap_buf);
        let _ = core::fmt::Write::write_fmt(&mut w, heap_msg);
        let heap_len = w.len();
        uart_raw::write_bytes(&heap_buf[..heap_len]);
        uart_raw::flush();
    }

    // Print location — short format (filename only) to avoid truncation.
    if let Some(loc) = info.location() {
        let file = loc.file();
        let filename = file.rsplit('/').next().unwrap_or(file);
        let loc_msg = core::format_args!(
            "PANIC {}:{}\n",
            filename,
            loc.line(),
        );
        let mut loc_buf = [0u8; 80];
        let mut w = SliceWrite::new(&mut loc_buf);
        let _ = core::fmt::Write::write_fmt(&mut w, loc_msg);
        let loc_len = w.len();
        uart_raw::write_bytes(&loc_buf[..loc_len]);
        uart_raw::flush();
    }

    // Print full panic message (may be long for OOM).
    // Use heap check: if heap is zero/critically low, skip the full
    // message since format! would re-trigger OOM → infinite recursion.
    if heap_free > 256 {
        let msg = core::format_args!("MSG: {}\n", info);
        let mut buf = [0u8; 1024];
        let mut writer = SliceWrite::new(&mut buf);
        let _ = core::fmt::Write::write_fmt(&mut writer, msg);
        let written = writer.len();
        uart_raw::write_bytes(&buf[..written]);
        // Flush twice to ensure all bytes are sent before the delay
        uart_raw::flush();
        uart_raw::flush();

        // Write crash info to NVS (pre-check prevents repeated writes in crash loops)
        let panic_msg = core::str::from_utf8(&buf[..written]).unwrap_or("PANIC");
        let reason = crash_info::CrashReason::classify(panic_msg);
        crash_info::write_crash_info(reason, panic_msg);
    }

    // Busy-wait ~1s to allow UART TX to fully transmit.
    const PANIC_DELAY_ITERATIONS: u32 = 10_000_000;
    let mut counter: u32 = 0;
    while counter < PANIC_DELAY_ITERATIONS {
        counter += 1;
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }

    esp_hal::system::software_reset()
}

/// Minimal writer that writes to a byte slice and tracks position.
struct SliceWrite<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> SliceWrite<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        SliceWrite { buf, pos: 0 }
    }

    fn len(&self) -> usize {
        self.pos
    }
}

impl<'a> core::fmt::Write for SliceWrite<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = &mut self.buf[self.pos..];
        let len = bytes.len().min(remaining.len());
        remaining[..len].copy_from_slice(&bytes[..len]);
        self.pos += len;
        Ok(())
    }
}

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
static UART_FIRST_BYTE_SEEN: AtomicU32 = AtomicU32::new(0); // 0=no, 1=yes
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

/// Channel for sending alert payloads from the main loop to the MQTT task.
static ALERT_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();

/// Channel for sending raw sniff frame JSON from main loop to MQTT task.
static SNIFF_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();

/// Client hash for RS-485 registration (2 bytes, set once at boot).
/// Shared between main loop and uart_task for fast-path registration responses.
static CLIENT_HASH_H: AtomicU32 = AtomicU32::new(0);
static CLIENT_HASH_L: AtomicU32 = AtomicU32::new(0);

/// Pending client ID assigned by the spa, stored by uart_task fast-path
/// when the FE BF 02 <ID> frame is decoded. Used to send the ACK on the
/// next Ready frame.
static PENDING_CLIENT_ID: AtomicU32 = AtomicU32::new(0);

/// Registration fast-path state machine, shared between uart_task and main loop.
/// uart_task reads this to decide whether to handle registration inline.
/// Main loop updates it when the SpaApp state machine changes.
///
/// Values:
///   0 = unregistered, waiting for query (uart_task should handle FE BF 00)
///   1 = query received, waiting for Ready to send ID request
///   2 = ID request sent, waiting for assignment (uart_task should handle FE BF 02)
///   3 = assigned, waiting for Ready to send ACK
///   4 = registered (uart_task should NOT handle registration frames)
static REG_FAST_STATE: AtomicU32 = AtomicU32::new(0);

#[embassy_executor::task]
async fn uart_task(mut transport: transport::Rs485Transport) {
    static UART_READ_ERR: launa_core::RateLog = launa_core::RateLog::new();
    static UART_WRITE_ERR: launa_core::RateLog = launa_core::RateLog::new();

    let mut decoder = FrameDecoder::new();
    let frame_sender = FRAME_CHANNEL.sender();
    let uart_rx = UART_TX_CHANNEL.receiver();
    let mut buf = [0u8; 128];
    let mut first_bytes_logged = false;
    let mut first_frame_logged = false;

    info!("UART task started");

    loop {
        // Use select so TX can proceed even when no RX data is available.
        match select(transport.read(&mut buf), uart_rx.receive()).await {
            Either::First(result) => {
                match result {
                    Ok(n) if n > 0 => {
                        UART_BYTES_RECEIVED.fetch_add(n as u32, Ordering::Relaxed);
                        if !first_bytes_logged {
                            first_bytes_logged = true;
                            UART_FIRST_BYTE_SEEN.store(1, Ordering::Relaxed);
                            // Log first few raw bytes for diagnostics
                            let hex_dump: Vec<u8> = buf[..n.min(16)].to_vec();
                            let hex_str = launa_protocol::hex::to_hex(&hex_dump);
                            info!("UART: first {} bytes from spa bus: {}", n, hex_str);
                        }
                        // Periodic raw byte logging (every ~1000 bytes)
                        let total = UART_BYTES_RECEIVED.load(Ordering::Relaxed);
                        if total % 1000 < n as u32 {
                            let hex_dump: Vec<u8> = buf[..n.min(8)].to_vec();
                            let hex_str = launa_protocol::hex::to_hex(&hex_dump);
                            debug!("UART: {} total bytes, last {} bytes: {}", total, n, hex_str);
                        }
                        let prev_errors = decoder.frame_error_count();
                        for &byte in &buf[..n] {
                            if let Some(frame) = decoder.feed(byte) {
                                if !first_frame_logged {
                                    first_frame_logged = true;
                                    info!(
                                        "UART: first frame decoded, type={:02X}{:02X}, len={}",
                                        frame.message_type[0],
                                        frame.message_type[1],
                                        frame.payload.len()
                                    );
                                }

                                // Fast-path registration: the Balboa spa sends the
                                // FE BF 00 registration query, then a Ready (10 BF 06)
                                // frame immediately after. The client must respond on
                                // the Ready frame (when the bus is free), NOT during
                                // the query frame (which would cause a bus collision).
                                let reg_state = REG_FAST_STATE.load(Ordering::Relaxed);
                                if reg_state != 4 {
                                    // Handle the registration query: queue the response
                                    if frame.message_type == [0xFE, 0xBF]
                                        && frame.payload.first() == Some(&0x00)
                                        && reg_state == 0
                                    {
                                        // Spa sent "any new clients?" — prepare ID request
                                        // but don't send yet; wait for the next Ready frame.
                                        REG_FAST_STATE.store(1, Ordering::Relaxed);
                                        warn!("REG fast-path: query received, waiting for Ready");
                                    }
                                    // Handle the ID assignment: queue the ACK
                                    else if frame.message_type == [0xFE, 0xBF]
                                        && frame.payload.len() >= 2
                                        && frame.payload[0] == 0x02
                                        && reg_state == 2
                                    {
                                        // Spa assigned client ID — prepare ACK
                                        // but don't send yet; wait for the next Ready frame.
                                        REG_FAST_STATE.store(3, Ordering::Relaxed);
                                        let id = frame.payload[1];
                                        PENDING_CLIENT_ID.store(id as u32, Ordering::Relaxed);
                                        warn!("REG fast-path: assigned ID 0x{:02X}, waiting for Ready", id);
                                    }
                                    // Handle Ready frame: send any queued registration response
                                    // Note: Ready can be 10 BF 06 (unregistered) or <ID> BF 06 (registered).
                                    // We match any XX BF with payload 06 when a reg response is pending.
                                    else if reg_state == 1 || reg_state == 3 {
                                        // Diagnostic: log every frame while waiting for Ready
                                        warn!(
                                            "REG fast-path: checking frame {:02X}{:02X} payload {:02X?} (state={})",
                                            frame.message_type[0], frame.message_type[1],
                                            frame.payload.first(),
                                            reg_state
                                        );
                                        if frame.message_type[1] == 0xBF
                                            && frame.payload.first() == Some(&0x06)
                                        {
                                            if reg_state == 1 {
                                                // Send ID request now (bus is free)
                                                let hash_h = CLIENT_HASH_H.load(Ordering::Relaxed) as u8;
                                                let hash_l = CLIENT_HASH_L.load(Ordering::Relaxed) as u8;
                                                match launa_protocol::frame::FrameEncoder::encode(
                                                    [0xFE, 0xBF],
                                                    &[0x01, 0x02, hash_h, hash_l],
                                                ) {
                                                    Ok(encoded) => {
                                                        if let Err(_) =
                                                            transport.write(&encoded).await
                                                        {
                                                            rate_error!(
                                                                UART_WRITE_ERR,
                                                                "UART write error: reg request"
                                                            );
                                                        } else {
                                                            REG_FAST_STATE.store(2, Ordering::Relaxed);
                                                            warn!(
                                                                "REG fast-path: sent ID request {:02X}{:02X}",
                                                                hash_h, hash_l
                                                            );
                                                        }
                                                    }
                                                    Err(_) => {
                                                        warn!("REG fast-path: encode failed");
                                                    }
                                                }
                                            } else if reg_state == 3 {
                                                // Send ACK now (bus is free)
                                                let id = PENDING_CLIENT_ID.load(Ordering::Relaxed) as u8;
                                                match launa_protocol::frame::FrameEncoder::encode(
                                                    [id, 0xBF],
                                                    &[0x03],
                                                ) {
                                                    Ok(encoded) => {
                                                        if let Err(_) =
                                                            transport.write(&encoded).await
                                                        {
                                                            rate_error!(
                                                                UART_WRITE_ERR,
                                                                "UART write error: reg ack"
                                                            );
                                                        } else {
                                                            REG_FAST_STATE.store(4, Ordering::Relaxed);
                                                            info!(
                                                                "REG fast-path: registered with ID 0x{:02X}",
                                                                id
                                                            );
                                                        }
                                                    }
                                                    Err(_) => {
                                                        warn!("REG fast-path: ack encode failed");
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                frame_sender.send(frame).await;
                            }
                        }
                        let new_errors = decoder.frame_error_count();
                        if new_errors > prev_errors {
                            FRAME_ERROR_COUNT
                                .fetch_add(new_errors - prev_errors, Ordering::Relaxed);
                        }
                    }
                    Ok(_) => {
                        Timer::after(Duration::from_millis(1)).await;
                    }
                    Err(_) => {
                        rate_error!(UART_READ_ERR, "UART read error: Io");
                        Timer::after(Duration::from_millis(10)).await;
                    }
                }
                // Drain any pending TX after processing RX
                while let Ok(data) = uart_rx.try_receive() {
                    if let Err(_) = transport.write(&data).await {
                        rate_error!(UART_WRITE_ERR, "UART write error: Io (drain)");
                    }
                }
            }
            Either::Second(data) => {
                if let Err(_) = transport.write(&data).await {
                    rate_error!(UART_WRITE_ERR, "UART write error: Io");
                }
                // Drain any additional pending TX
                while let Ok(data) = uart_rx.try_receive() {
                    if let Err(_) = transport.write(&data).await {
                        rate_error!(UART_WRITE_ERR, "UART write error: Io (drain)");
                    }
                }
            }
        }
    }
}

mod mqtt_task;

/// Self-test status publish interval in seconds.
const SELF_TEST_PUBLISH_INTERVAL_SECS: u64 = 1;

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

/// Sync SpaApp registration state to the uart_task fast-path atomics.
///
/// The uart_task uses `REG_FAST_STATE` and `CLIENT_HASH_*` to respond to
/// registration frames immediately without going through the main loop.
/// This function must be called after any SpaApp operation that may change
/// registration state (process_frame, tick, hash rotation, stale detection).
fn sync_reg_fast_path(app: &SpaApp<'_>) {
    let new_state = match app.registration_state_str() {
        "waiting_for_query" => 0u32,
        "waiting_for_assignment" => 2u32,
        "registered" => 4u32,
        _ => 0u32,
    };
    let prev = REG_FAST_STATE.load(Ordering::Relaxed);
    if prev != new_state {
        REG_FAST_STATE.store(new_state, Ordering::Relaxed);
    }
    // Always sync the current hash (may have rotated due to repeated failures)
    let hash = app.client_hash();
    CLIENT_HASH_H.store(hash[0] as u32, Ordering::Relaxed);
    CLIENT_HASH_L.store(hash[1] as u32, Ordering::Relaxed);
}

/// Execute a batch of `AppAction` side effects from `SpaApp`.
///
/// Maps each action to the corresponding IO operation (UART send, MQTT publish, etc.).
async fn execute_actions(
    actions: &[AppAction],
    device_id: &str,
    self_test: bool,
    sniff_mode: bool,
    wifi_rssi: Option<i32>,
) {
    for action in actions {
        match action {
            AppAction::SendFrame(bytes) => {
                // Suppress duplicate registration frames when the uart_task
                // fast-path has already sent them. Registration frames have
                // message type FE BF (ID request) or XX BF with payload 03
                // (ID ack). The fast-path handles these inline with zero
                // latency; the slow path via SpaApp would send duplicates.
                let reg_fast = REG_FAST_STATE.load(Ordering::Relaxed);
                let is_dup_reg = if bytes.len() >= 6 {
                    // Check for FE BF registration request or ID BF 03 ack
                    let inner_start = if bytes[0] == 0x7E { 1 } else { 0 };
                    if bytes.len() > inner_start + 3 {
                        let mt_hi = bytes[inner_start + 1];
                        let mt_lo = bytes[inner_start + 2];
                        // FE BF XX = registration request (fast-path state >= 2 means already sent)
                        // or XX BF 03 = ID ack (fast-path state == 4 means already sent)
                        (mt_hi == 0xFE && mt_lo == 0xBF && reg_fast >= 2)
                            || (mt_lo == 0xBF
                                && bytes.len() > inner_start + 4
                                && bytes[inner_start + 3] == 0x03
                                && reg_fast == 4)
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !is_dup_reg {
                    UART_TX_CHANNEL.send(bytes.clone()).await;
                }
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
                        self_test,
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
                let uart_active = UART_FIRST_BYTE_SEEN.load(Ordering::Relaxed);
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
                if let Err(_) = OTA_CHANNEL.try_send(url.clone()) {
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

/// Publish a raw frame to the sniff channel for MQTT delivery.
///
/// Formats the frame as JSON matching the sniffer protocol:
/// `{"raw":"<hex>","type":"<MT>","len":<N>,"crc_ok":<bool>}`
fn publish_sniff_frame(frame: &Frame) {
    let hex_str = launa_protocol::hex::to_hex(&frame.payload);

    let mt = alloc::format!("{:02X}{:02X}", frame.message_type[0], frame.message_type[1]);
    let crc_ok = Frame::parse(&frame.payload).is_ok();

    let json = alloc::format!(
        r#"{{"raw":"{}","type":"{}","len":{},"crc_ok":{}}}"#,
        hex_str,
        mt,
        frame.payload.len(),
        crc_ok
    );

    if SNIFF_CHANNEL.try_send(Vec::from(json.as_bytes())).is_err() {
        warn!("SNIFF_CHANNEL full, dropping frame");
    }
}

/// Handle an incoming MQTT command, routing through self-test or SpaApp.
async fn handle_mqtt_command(
    cmd: Command,
    app: &mut SpaApp<'_>,
    self_test_state: &mut Option<self_test::SelfTestState>,
    sniff_mode: &mut bool,
    device_id: &str,
    self_test_last_publish: &mut Option<Instant>,
) {
    match cmd {
        Command::SelfTest(enable) => {
            if enable {
                if self_test_state.is_none() {
                    info!("Self-test mode enabled");
                    *self_test_state = Some(self_test::SelfTestState::new());
                    // Reset publish timer so first status is published immediately
                    *self_test_last_publish = None;
                }
            } else {
                if self_test_state.is_some() {
                    info!("Self-test mode disabled, resuming normal operation");
                    *self_test_state = None;
                    // Immediately publish current spa state so the UI
                    // receives self_test: false without waiting for the
                    // next status change from the spa.
                    let actions = app.force_publish();
                    execute_actions(&actions, device_id, false, *sniff_mode, read_wifi_rssi())
                        .await;
                }
            }
        }
        Command::Sniff(enable) => {
            if enable && !*sniff_mode {
                info!("Sniff mode enabled — publishing raw RS-485 frames");
                *sniff_mode = true;
                // Immediately publish state so UI receives sniff_mode: true
                let actions = app.force_publish();
                execute_actions(
                    &actions,
                    device_id,
                    self_test_state.is_some(),
                    true,
                    read_wifi_rssi(),
                )
                .await;
            } else if !enable && *sniff_mode {
                info!("Sniff mode disabled — resuming normal operation");
                *sniff_mode = false;
                // Immediately publish state so UI receives sniff_mode: false
                let actions = app.force_publish();
                execute_actions(
                    &actions,
                    device_id,
                    self_test_state.is_some(),
                    false,
                    read_wifi_rssi(),
                )
                .await;
            }
        }
        Command::Reboot => {
            info!("Remote reboot requested via MQTT, resetting in 1s...");
            // Brief delay to allow the log message and any pending MQTT acks to flush
            Timer::after(Duration::from_secs(1)).await;
            esp_hal::system::software_reset();
        }
        _ => {
            if let Some(ref mut st) = self_test_state {
                st.apply_command(&cmd);
            } else {
                let actions = app.on_mqtt_command(cmd);
                execute_actions(
                    &actions,
                    device_id,
                    self_test_state.is_some(),
                    *sniff_mode,
                    read_wifi_rssi(),
                )
                .await;
            }
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
    use launa_ota::OtaUpdate;

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

/// Receive configuration over USB serial (UART0) using raw register access.
///
/// Waits for `CONFIG_START` followed by key=value lines and `CONFIG_END`.
/// Parses the config and returns it. Does NOT save to NVS — the caller
/// is responsible for persisting.
/// Returns `None` if no config was received within the timeout.
///
/// This uses raw UART0 register reads (see `uart_raw`) so it works alongside
/// the logger which writes to UART0 TX. The host sends config data over
/// USB serial which is physically UART0.
fn receive_serial_config(timeout_secs: u64) -> Option<config::AppConfig> {
    info!("Waiting for serial config ({}s timeout)...", timeout_secs);

    const MAX_LINE_LEN: usize = 256;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut line_buf: Vec<u8> = Vec::new();
    let mut config_started = false;
    let mut kv_pairs: Vec<(alloc::string::String, alloc::string::String)> = Vec::new();
    let mut config_done = false;

    while Instant::now() < deadline && !config_done {
        // Drain UART0 RX FIFO (USB serial)
        let mut rx_byte = uart_raw::read_byte();
        while let Some(byte) = rx_byte {
            if byte == b'\n' {
                let line = {
                    let raw = core::str::from_utf8(&line_buf).unwrap_or("");
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
                if line_buf.len() < MAX_LINE_LEN {
                    line_buf.push(byte);
                }
            }

            rx_byte = uart_raw::read_byte();
        }

        // Brief delay to avoid busy-looping when FIFO is empty
        esp_hal::rom::ets_delay_us(1000);
    }

    if !config_done {
        info!("No serial config received, continuing with NVS config");
        return None;
    }

    // Map dotted keys to AppConfig fields
    let mut app_config = config::AppConfig::default();

    for (key, value) in &kv_pairs {
        match key.as_str() {
            "wifi.ssid" => app_config.wifi_ssid = value.clone(),
            "wifi.password" => app_config.wifi_password = value.clone(),
            "mqtt.host" => app_config.mqtt_host = value.clone(),
            "mqtt.port" => match value.parse::<u16>() {
                Ok(p) => {
                    app_config.mqtt_port = p;
                }
                Err(_) => {
                    warn!("Invalid port: {}", value);
                    uart_raw::write_bytes(
                        alloc::format!("CONFIG_ERROR:invalid_port={}\n", value).as_bytes(),
                    );
                    uart_raw::flush();
                    return None;
                }
            },
            "mqtt.user" => app_config.mqtt_user = value.clone(),
            "mqtt.password" => app_config.mqtt_password = value.clone(),
            "device.id" => app_config.device_id = value.clone(),
            other => {
                warn!("Unknown config key: {}", other);
            }
        }
    }

    info!(
        "Parsed config: ssid=<{} chars> mqtt={}:{} device={}",
        app_config.wifi_ssid.len(),
        app_config.mqtt_host,
        app_config.mqtt_port,
        app_config.device_id
    );

    uart_raw::write_bytes(b"CONFIG_OK\n");
    uart_raw::flush();
    Some(app_config)
}

/// Process UART frames received during the event loop.
///
/// Process incoming UART frames through SpaApp for state updates and commands.
/// In sniff mode, also publishes raw frames to the sniff channel as a side effect.
/// In self-test mode, discards all UART frames with a one-time warning.
async fn process_uart_frames(
    frame: Frame,
    app: &mut SpaApp<'_>,
    device_id: &str,
    self_test_active: bool,
    sniff_mode: bool,
    frame_rx: &embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, Frame, 4>,
    self_test_discard_warned: &mut bool,
) {
    if self_test_active {
        // When in self-test mode, drain and discard UART frames with a one-time warning
        if !*self_test_discard_warned {
            warn!("Self-test active: discarding spa frames (self-test generates its own state)");
            *self_test_discard_warned = true;
        }
        while frame_rx.try_receive().is_ok() {}
        return;
    }

    // In sniff mode, publish raw frames as a side effect alongside normal processing.
    if sniff_mode {
        publish_sniff_frame(&frame);
    }

    let actions = app.process_frame(&frame);
    execute_actions(
        &actions,
        device_id,
        self_test_active,
        sniff_mode,
        read_wifi_rssi(),
    )
    .await;

    while let Ok(frame) = frame_rx.try_receive() {
        if sniff_mode {
            publish_sniff_frame(&frame);
        }
        let actions = app.process_frame(&frame);
        execute_actions(
            &actions,
            device_id,
            self_test_active,
            sniff_mode,
            read_wifi_rssi(),
        )
        .await;
    }
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

/// Mark firmware as valid (boot validation passed: WiFi + MQTT connected).
fn validate_firmware(ota: &mut Option<ota::EspOta>) {
    use launa_ota::OtaUpdate;
    if let Some(ref mut o) = ota {
        if let Err(e) = o.mark_valid() {
            warn!("Failed to mark firmware valid: {:?}", e);
        } else {
            info!("Firmware marked valid (boot validation passed)");
        }
    }
}

/// Run self-test tick logic: advance simulator state and publish status periodically.
async fn tick_self_test(
    self_test_state: &mut self_test::SelfTestState,
    device_id: &str,
    sniff_mode: bool,
    self_test_last_publish: &mut Option<Instant>,
) {
    self_test_state.tick();
    let now = Instant::now();
    let should_publish = self_test_last_publish.map_or(true, |t| {
        t.elapsed().as_secs() >= SELF_TEST_PUBLISH_INTERVAL_SECS
    });
    if should_publish {
        let status = self_test_state.status();
        execute_actions(
            &[AppAction::PublishState {
                status: status.clone(),
                fault: None,
                recovering_from_stale: false,
                registration_state: "registered",
            }],
            device_id,
            true,
            sniff_mode,
            read_wifi_rssi(),
        )
        .await;
        *self_test_last_publish = Some(now);
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

    let mut app_config;
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

            nvs_handle = Some(nvs);
        }
        None => {
            warn!("NVS unavailable — using default config, OTA disabled");
            app_config = config::AppConfig::default();
        }
    }

    // Brief serial config window: listen on UART0 (USB serial) for config-flash.
    // If CONFIG_START is received within 5 seconds, accept new config and save to NVS.
    // Otherwise proceed normally with the NVS/default config.
    if let Some(new_config) = receive_serial_config(5) {
        app_config = new_config;
        if let Some(ref mut nvs) = nvs_handle {
            let mut aes = esp_hal::aes::Aes::new(unsafe { esp_hal::peripherals::AES::steal() });
            let mut rng = esp_hal::rng::Rng::new();
            app_config.save(nvs, &mut aes, &mut rng);
            info!("Serial config saved to NVS");
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

    info!("RS-485 UART initialized (no DE pin, auto-direction transceiver)");

    let uart_transport = transport::Rs485Transport::new(uart, None);

    let wifi_stack = init_wifi(
        spawner,
        peripherals.WIFI,
        esp_hal::rng::Rng::new(),
        &app_config,
    )
    .await;

    // Create MqttClient on the stack. Socket buffers inside are pre-allocated
    // via mk_static! in new() — only the struct itself is stack-local, and it
    // moves into the embassy task on spawn.
    let mut mqtt = mqtt_client::MqttClient::new(wifi_stack.stack, &app_config, boot_id());

    // Connect to MQTT broker on the ThreadModeExecutor. The heavy formatting
    // in connect/discovery code runs here to avoid overflowing the shared
    // interrupt/task stack on ESP32/Xtensa.
    info!("Connecting to MQTT broker...");
    {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match mqtt.connect().await {
                Ok(()) => break,
                Err(e) => {
                    let backoff = launa_core::network::backoff_secs(attempt);
                    error!(
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
    if let Err(e) = mqtt.post_connect_publish(false).await {
        warn!("Post-connect publish failed: {:?}", e);
    }

    // Publish crash alarm from previous boot if present.
    // MQTT is now connected, so we send via ALERT_CHANNEL for the mqtt_task
    // to publish.
    if let Some(ref crash) = pending_crash_alarm {
        let alarm_json = crash_info::crash_alarm_json(crash, FIRMWARE_VERSION);
        let _ = ALERT_CHANNEL.try_send(Vec::from(alarm_json.as_bytes()));
        info!("Crash alarm queued: reason={}", crash.reason.as_str());
        drop(pending_crash_alarm.take());
    }

    validate_firmware(&mut ota);

    // Spawn background tasks on the ThreadModeExecutor.
    spawner.spawn(mqtt_task::mqtt_task(mqtt).unwrap());
    spawner.spawn(uart_task(uart_transport).unwrap());

    info!("Entering main event loop");

    let frame_rx = FRAME_CHANNEL.receiver();
    let cmd_rx = COMMAND_CHANNEL.receiver();
    let pump_timer_rx = PUMP_TIMER_CHANNEL.receiver();
    let ota_rx = OTA_CHANNEL.receiver();

    let clock = clock::EmbassyClock::new();
    // Derive a unique 2-byte client hash from the device ID for RS-485 registration.
    // Uses a simple FNV-1a-like hash so each device gets a distinct channel assignment.
    let client_hash = {
        let mut h: u16 = 0x811C;
        for &b in app_config.device_id.as_bytes() {
            h ^= b as u16;
            h = h.wrapping_mul(0x0101);
        }
        [(h >> 8) as u8, h as u8]
    };
    // Store client hash for uart_task fast-path registration
    CLIENT_HASH_H.store(client_hash[0] as u32, Ordering::Relaxed);
    CLIENT_HASH_L.store(client_hash[1] as u32, Ordering::Relaxed);
    let mut app = SpaApp::with_client_hash(&clock, client_hash);
    let device_id_str: &str = &app_config.device_id;
    let mut self_test_state: Option<self_test::SelfTestState> = None;
    let mut sniff_mode: bool = false;
    let mut self_test_last_publish: Option<Instant> = None;
    let mut self_test_discard_warned: bool = false;

    let tick_interval = Duration::from_secs(1);

    // Periodic UART TX test: send a registration probe every 15s when no bytes
    // have been received, to verify the UART TX path works. This also acts as
    // a keep-alive ping on the RS-485 bus.
    let mut uart_tx_test_interval: Option<Instant> = None;

    // MQTT task health: track the last tick value and when we last saw it change.
    let mut mqtt_last_tick: u32 = mqtt_task::MQTT_TASK_TICK.load(Ordering::Relaxed);
    let mut mqtt_last_tick_time: Instant = Instant::now();

    loop {
        // Feed the hardware watchdog each iteration
        wdt.feed();

        // Check MQTT task health: if the tick counter hasn't changed in 30s,
        // the MQTT task is frozen (cooperative executor starvation).
        let mqtt_tick = mqtt_task::MQTT_TASK_TICK.load(Ordering::Relaxed);
        if mqtt_tick != mqtt_last_tick {
            mqtt_last_tick = mqtt_tick;
            mqtt_last_tick_time = Instant::now();
        } else if mqtt_last_tick_time.elapsed().as_secs() >= 30 {
            warn!(
                "MQTT task appears frozen (tick unchanged for {}s)",
                mqtt_last_tick_time.elapsed().as_secs()
            );
            send_alert("error", "mqtt_task_frozen");
            // Reset the timer so we don't spam alerts every tick
            mqtt_last_tick_time = Instant::now();
        }

        // Multiplex: wait for either a UART frame, an MQTT command, or a
        // 1-second tick timer. This replaces the old blocking receive() that
        // hung indefinitely when the spa was off (no OTA, no commands, no ticks).
        match select(
            frame_rx.receive(),
            select(cmd_rx.receive(), Timer::after(tick_interval)),
        )
        .await
        {
            // UART frame received
            Either::First(frame) => {
                process_uart_frames(
                    frame,
                    &mut app,
                    device_id_str,
                    self_test_state.is_some(),
                    sniff_mode,
                    &frame_rx,
                    &mut self_test_discard_warned,
                )
                .await;
                sync_reg_fast_path(&app);
            }
            // MQTT command received
            Either::Second(Either::First(cmd)) => {
                handle_mqtt_command(
                    cmd,
                    &mut app,
                    &mut self_test_state,
                    &mut sniff_mode,
                    device_id_str,
                    &mut self_test_last_publish,
                )
                .await;
            }
            // Tick timer expired
            Either::Second(Either::Second(_)) => {}
        }

        // Drain MQTT commands (non-blocking)
        while let Ok(cmd) = cmd_rx.try_receive() {
            handle_mqtt_command(
                cmd,
                &mut app,
                &mut self_test_state,
                &mut sniff_mode,
                device_id_str,
                &mut self_test_last_publish,
            )
            .await;
        }

        // In self-test mode, tick the simulator and publish status periodically
        if let Some(ref mut st) = self_test_state {
            tick_self_test(st, device_id_str, sniff_mode, &mut self_test_last_publish).await;
        } else {
            self_test_last_publish = None;
            // Periodic tick: stale detection, registration timeout, diagnostics
            let tick_actions = app.tick();
            execute_actions(
                &tick_actions,
                device_id_str,
                false,
                sniff_mode,
                read_wifi_rssi(),
            )
            .await;
            // Sync after tick (registration timeout may reset state, hash may rotate)
            sync_reg_fast_path(&app);
        }

        // Heap check (uses actual ESP32 free heap)
        let heap_actions = app.check_heap(esp_alloc::HEAP.free());
        execute_actions(
            &heap_actions,
            device_id_str,
            self_test_state.is_some(),
            sniff_mode,
            read_wifi_rssi(),
        )
        .await;

        // UART health check: alert if no bytes received after 30s of uptime
        // Re-alert every 5 minutes until bytes are seen
        let uptime = uptime_secs();
        if UART_FIRST_BYTE_SEEN.load(Ordering::Relaxed) == 0 && uptime >= 30 {
            let last_alert = UART_LAST_NO_BYTE_ALERT_SECS.load(Ordering::Relaxed) as u64;
            if uptime - last_alert >= 300 || last_alert == 0 {
                UART_LAST_NO_BYTE_ALERT_SECS.store(uptime as u32, Ordering::Relaxed);
                error!("UART: no bytes received after {}s — check RS-485 wiring (RX=GPIO16, TX=GPIO17, DE=GPIO4)", uptime);
                send_alert("error", "no_uart_bytes");
            }
        }

        // UART TX test: every 15s when no bytes received, send a test frame
        // to verify the UART TX path and check if the spa responds.
        if UART_FIRST_BYTE_SEEN.load(Ordering::Relaxed) == 0 && !self_test_state.is_some() {
            let should_test = uart_tx_test_interval.is_none_or(|t| t.elapsed().as_secs() >= 15);
            if should_test {
                uart_tx_test_interval = Some(Instant::now());
                // Send a registration request: FE BF 01 02 F1 73
                match launa_protocol::frame::FrameEncoder::encode(
                    [0xFE, 0xBF],
                    &[0x01, 0x02, client_hash[0], client_hash[1]],
                ) {
                    Ok(encoded) => {
                        info!(
                            "UART TX test: sending registration probe ({} bytes)",
                            encoded.len()
                        );
                        UART_TX_CHANNEL.send(encoded).await;
                    }
                    Err(e) => {
                        warn!("UART TX test: encode failed: {:?}", e);
                    }
                }
            }
        }

        handle_ota_request(&wifi_stack, &mut ota, &mut ota_buffers, &ota_rx, &mut wdt).await;

        // Drain pump timer commands
        while let Ok((pump_index, minutes)) = pump_timer_rx.try_receive() {
            let actions = app.start_pump_timer(pump_index, minutes);
            execute_actions(
                &actions,
                device_id_str,
                self_test_state.is_some(),
                sniff_mode,
                read_wifi_rssi(),
            )
            .await;
            info!("Started pump {} timer for {} min", pump_index, minutes);
        }
    }
}
