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
use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::ram;
use launa_hal::Transport as _;
use launa_core::{AppAction, SpaApp};
use launa_protocol::command::Command;
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_protocol::status::StatusUpdate;
use log::{error, info, warn};

use diagnostics::{publish_diagnostics, send_alert};
use types::FaultBuf;

mod clock;
mod config;
mod logger;
mod crypto;
mod diagnostics;
mod macros;
mod mqtt_client;
mod net_util;
mod ota;
mod transport;
mod types;
mod wifi;

#[cfg(feature = "remote-log")]
mod remote_log;
mod self_test;

#[cfg(feature = "sniff")]
mod sniff;

/// Custom panic handler: logs panic location, waits 500ms for log flush,
/// then triggers a software reset. Replaces esp-backtrace's default infinite
/// loop to allow automatic recovery from panics.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Write directly to UART0 registers - don't use the logger since
    // the panic might have occurred while holding the logger lock.
    const UART_FIFO: usize = 0x60000000;
    const UART_STATUS: usize = 0x6000001C;
    const TX_FIFO_CNT_MASK: u32 = 0x7F << 16;
    const FIFO_SIZE: u16 = 128;

    let msg = core::format_args!("PANIC: {}\n", info);
    let mut buf = [0u8; 256];
    let mut writer = SliceWrite::new(&mut buf);
    let _ = core::fmt::Write::write_fmt(&mut writer, msg);
    let written = writer.len();

    unsafe {
        for &b in &buf[..written] {
            // Wait for FIFO space
            while (((UART_STATUS as *const u32).read_volatile() & TX_FIFO_CNT_MASK) >> 16) as u16 >= FIFO_SIZE {
                core::hint::spin_loop();
            }
            (UART_FIFO as *mut u8).write_volatile(b);
        }
        // Wait for FIFO to drain
        while (((UART_STATUS as *const u32).read_volatile() & TX_FIFO_CNT_MASK) >> 16) as u16 > 0 {
            core::hint::spin_loop();
        }
    }

    // Busy-wait ~500ms to allow UART TX to fully transmit.
    let mut counter: u32 = 0;
    let iterations = 5_000_000; // ~500ms at 240 MHz with overhead
    while counter < iterations {
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

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, Frame, 4> = Channel::new();
static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, Command, 4> = Channel::new();
static UART_TX_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();
static STATE_CHANNEL: Channel<CriticalSectionRawMutex, (StatusUpdate, FaultBuf, bool), 4> = Channel::new();
static PUMP_TIMER_CHANNEL: Channel<CriticalSectionRawMutex, (u8, u32), 4> = Channel::new();
static DIAGNOSTICS_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 2> = Channel::new();
static OTA_CHANNEL: Channel<CriticalSectionRawMutex, alloc::string::String, 1> = Channel::new();

/// Signal set when WiFi reconnects after a disconnect. MQTT task checks this
/// to force a clean MQTT reconnect (old TCP socket may be stale).
pub static WIFI_RECONNECT_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Channel for sending alert payloads from the main loop to the MQTT task.
static ALERT_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();

#[embassy_executor::task]
async fn uart_task(mut transport: transport::Rs485Transport) {
    let mut decoder = FrameDecoder::new();
    let frame_sender = FRAME_CHANNEL.sender();
    let uart_rx = UART_TX_CHANNEL.receiver();
    let mut buf = [0u8; 128];

    info!("UART task started");

    loop {
        // Read from UART first (prioritize reads to avoid starving frame processing)
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

        // Check for outgoing data after reads
        if let Ok(data) = uart_rx.try_receive() {
            if let Err(e) = transport.write(&data).await {
                error!("UART write error: {:?}", e);
            }
        }
    }
}

mod mqtt_task;

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
                let fb = fault.as_ref().map_or(FaultBuf::EMPTY, |s| FaultBuf::from_str(s));
                if STATE_CHANNEL.try_send((status.clone(), fb, *recovering_from_stale)).is_err() {
                    warn!("STATE_CHANNEL full, dropping state update (capacity 4)");
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
                command_retries,
                command_drops,
            } => {
                publish_diagnostics(device_id, *uptime_secs, *frames_received, *command_retries, *command_drops);
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

/// Handle an incoming MQTT command, routing through self-test or SpaApp.
async fn handle_mqtt_command(
    cmd: Command,
    app: &mut SpaApp<'_>,
    self_test_state: &mut Option<self_test::SelfTestState>,
    device_id: &str,
) -> bool {
    // Returns true if self-test state changed and should be published immediately
    match cmd {
        Command::SelfTest(enable) => {
            if enable {
                if self_test_state.is_none() {
                    info!("Self-test mode enabled");
                    *self_test_state = Some(self_test::SelfTestState::new());
                    return true;
                }
            } else {
                if self_test_state.is_some() {
                    info!("Self-test mode disabled, resuming normal operation");
                    *self_test_state = None;
                }
            }
            false
        }
        _ => {
            if let Some(ref mut st) = self_test_state {
                st.apply_command(&cmd)
            } else {
                let actions = app.on_mqtt_command(cmd);
                execute_actions(&actions, device_id).await;
                false
            }
        }
    }
}

#[cfg(feature = "hw-test")]
#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

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

    // Wait for CONFIG_START over serial, parse key=value lines,
    // write to NVS on CONFIG_END. 30-second timeout.
    info!("Waiting for serial config (30s timeout)...");

    /// Maximum line length for serial config receiver.
    /// Config lines are short key=value pairs (well under 256 bytes).
    /// This bound prevents OOM on the 32 KiB ESP32 heap if serial input
    /// is continuous without a newline terminator.
    const MAX_LINE_LEN: usize = 256;

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
                        if line_buf.len() < MAX_LINE_LEN {
                            line_buf.push(byte);
                        }
                        // Excess bytes beyond MAX_LINE_LEN are silently dropped.
                        // A warning is not logged per-byte to avoid flooding
                        // the serial output on sustained overflow.
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

    // Mask sensitive fields: SSID and MQTT host are secrets that should
    // not appear in plain text in logs. Show first 2 chars + "***" to aid
    // debugging without exposing full values.
    let masked_ssid = if app_config.wifi_ssid.len() > 2 {
        // SAFETY: index 0..2 is within bounds since len() > 2
        let prefix = &app_config.wifi_ssid[..2];
        alloc::format!("{}***", prefix)
    } else {
        alloc::string::String::from("***")
    };
    let masked_host = if app_config.mqtt_host.len() > 2 {
        // SAFETY: index 0..2 is within bounds since len() > 2
        let prefix = &app_config.mqtt_host[..2];
        alloc::format!("{}***", prefix)
    } else {
        alloc::string::String::from("***")
    };
    info!(
        "Parsed config: ssid={} mqtt={}:{} device={}",
        masked_ssid, masked_host, app_config.mqtt_port, app_config.device_id
    );

    // Write to NVS
    match config::AppConfig::open_nvs(peripherals.FLASH) {
        Some(mut nvs) => {
            let mut aes = esp_hal::aes::Aes::new(peripherals.AES);
            let mut rng = esp_hal::rng::Rng::new();
            app_config.save(&mut nvs, &mut aes, &mut rng);
            let _ = uart.write(b"CONFIG_OK\n");
            let _ = uart.flush();
            info!("Config saved to NVS successfully");
        }
        None => {
            warn!("NVS unavailable — cannot save config");
            let _ = uart.write(b"CONFIG_ERROR:nvs_unavailable\n");
            let _ = uart.flush();
        }
    }
}

#[cfg(not(any(feature = "sniff", feature = "hw-test")))]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    use launa_ota::OtaUpdate;

    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Initialize logger (uses raw UART0 registers, bypasses buggy ROM function)
    logger::init();

    // Record boot timestamp for diagnostics uptime calculation
    DIAGNOSTICS_START_SECS.store((Instant::now().as_millis() / 1000) as u32, Ordering::Relaxed);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // Configure TIMG1 as independent hardware watchdog (30s timeout)
    let timg1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG1);
    let mut wdt = timg1.wdt;
    wdt.set_timeout(
        esp_hal::timer::timg::MwdtStage::Stage0,
        esp_hal::time::Duration::from_secs(120),
    );
    wdt.enable();
    info!("Hardware watchdog enabled (120s timeout)");

    info!("Launa ESP32 firmware starting...");

    let app_config;
    let mut ota;
    let mut ota_buffers;
    match config::AppConfig::open_nvs(peripherals.FLASH) {
        Some(mut nvs) => {
            let mut aes = esp_hal::aes::Aes::new(peripherals.AES);
            let mut rng = esp_hal::rng::Rng::new();
            app_config = config::AppConfig::load(&mut nvs, &mut aes, &mut rng);
            info!("Config loaded: device_id={}", app_config.device_id);
            // Recover flash from NVS for OTA use
            let flash = nvs.into_inner();
            ota = Some(ota::create_ota(flash));
            ota_buffers = Some(ota::OtaBuffers::new());
        }
        None => {
            warn!("NVS unavailable — using default config, OTA disabled");
            app_config = config::AppConfig::default();
            ota = None;
            ota_buffers = None;
        }
    }

    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("Failed to create UART")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    let uart_transport = transport::Rs485Transport::new(uart, Some(peripherals.GPIO4.into()));
    info!("RS-485 UART initialized");

    let wifi_stack = match wifi::WifiStack::connect(
        spawner,
        peripherals.WIFI,
        esp_hal::rng::Rng::new(),
        &app_config.wifi_ssid,
        &app_config.wifi_password,
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
    };

    let mut mqtt = {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            match mqtt_client::MqttClient::connect(wifi_stack.stack, &app_config).await {
                Ok(m) => break m,
                Err(e) => {
                    let backoff_secs = (5u64 << attempt.saturating_sub(1).min(4)).min(60);
                    error!(
                        "MQTT connect attempt {} failed: {:?}, retrying in {}s",
                        attempt, e, backoff_secs
                    );
                    Timer::after(Duration::from_secs(backoff_secs)).await;
                    if attempt >= 10 {
                        error!("MQTT connect failed after 10 attempts, resetting");
                        esp_hal::system::software_reset();
                    }
                }
            }
        }
    };

    let _ = mqtt.publish_availability(true).await;
    let _ = mqtt.publish_discovery(false).await; // Fahrenheit default; mqtt_task will re-publish with correct scale after first status
    let _ = mqtt.subscribe_commands().await;

    // Mark firmware as valid (boot successful: WiFi + MQTT connected).
    if let Some(ref mut o) = ota {
        if let Err(e) = o.mark_valid() {
            warn!("Failed to mark firmware valid: {:?}", e);
        } else {
            info!("Firmware marked valid (boot validation passed)");
        }
    }

    // Spawn background tasks
    spawner
        .spawn(mqtt_task::mqtt_task(mqtt).unwrap());
    spawner
        .spawn(uart_task(uart_transport).unwrap());

    info!("Entering main event loop");

    let frame_rx = FRAME_CHANNEL.receiver();
    let cmd_rx = COMMAND_CHANNEL.receiver();
    let pump_timer_rx = PUMP_TIMER_CHANNEL.receiver();
    let ota_rx = OTA_CHANNEL.receiver();

    let clock = clock::EmbassyClock::new();
    let mut app = SpaApp::new(&clock);
    let device_id_str: &str = &app_config.device_id;
    let mut self_test_state: Option<self_test::SelfTestState> = None;
    let mut self_test_last_publish: Option<Instant> = None;
    const SELF_TEST_PUBLISH_INTERVAL_SECS: u64 = 5;

    let tick_interval = Duration::from_secs(1);

    loop {
        // Feed the hardware watchdog each iteration
        wdt.feed();

        // When self-test is active, skip UART frames entirely
        let skip_uart = self_test_state.is_some();

        // Multiplex: wait for either a UART frame, an MQTT command, or a
        // 1-second tick timer. This replaces the old blocking receive() that
        // hung indefinitely when the spa was off (no OTA, no commands, no ticks).
        match select(frame_rx.receive(), select(cmd_rx.receive(), Timer::after(tick_interval))).await {
            // UART frame received
            Either::First(frame) => {
                if !skip_uart {
                    let actions = app.process_frame(&frame);
                    execute_actions(&actions, device_id_str).await;

                    // Drain all available frames
                    while let Ok(frame) = frame_rx.try_receive() {
                        let actions = app.process_frame(&frame);
                        execute_actions(&actions, device_id_str).await;
                    }
                }
                // When in self-test mode, drain and discard UART frames
                while frame_rx.try_receive().is_ok() {}
            }
            // MQTT command received
            Either::Second(Either::First(cmd)) => {
                let changed = handle_mqtt_command(cmd, &mut app, &mut self_test_state, device_id_str).await;
                if changed {
                    if let Some(ref st) = self_test_state {
                        execute_actions(&[AppAction::PublishState {
                            status: st.status().clone(),
                            fault: None,
                            recovering_from_stale: false,
                        }], device_id_str).await;
                        self_test_last_publish = Some(Instant::now());
                    }
                }
            }
            // Tick timer expired
            Either::Second(Either::Second(_)) => {}
        }

        // Drain MQTT commands (non-blocking)
        while let Ok(cmd) = cmd_rx.try_receive() {
            let changed = handle_mqtt_command(cmd, &mut app, &mut self_test_state, device_id_str).await;
            if changed {
                if let Some(ref st) = self_test_state {
                    execute_actions(&[AppAction::PublishState {
                        status: st.status().clone(),
                        fault: None,
                        recovering_from_stale: false,
                    }], device_id_str).await;
                    self_test_last_publish = Some(Instant::now());
                }
            }
        }

        // In self-test mode, publish simulated status periodically
        if let Some(ref st) = self_test_state {
            let now = Instant::now();
            let should_publish = self_test_last_publish
                .map_or(true, |t| t.elapsed().as_secs() >= SELF_TEST_PUBLISH_INTERVAL_SECS);
            if should_publish {
                execute_actions(&[AppAction::PublishState {
                    status: st.status().clone(),
                    fault: None,
                    recovering_from_stale: false,
                }], device_id_str).await;
                self_test_last_publish = Some(now);
            }
        } else {
            self_test_last_publish = None;
            // Periodic tick: stale detection, registration timeout, diagnostics
            let tick_actions = app.tick();
            execute_actions(&tick_actions, device_id_str).await;
        }

        // Heap check (uses actual ESP32 free heap)
        let heap_actions = app.check_heap(esp_alloc::HEAP.free());
        execute_actions(&heap_actions, device_id_str).await;

        if let Ok(firmware_url) = ota_rx.try_receive() {
            // If URL contains "?test=1", run TCP connectivity test instead of OTA
            if firmware_url.contains("?test=1") {
                info!("OTA: TCP test requested via ?test=1 parameter");
                match ota_buffers.as_mut() {
                    Some(b) => {
                        match ota::tcp_test(wifi_stack.stack, &firmware_url, b).await {
                            Ok(()) => info!("OTA: TCP test PASSED"),
                            Err(()) => {
                                error!("OTA: TCP test FAILED");
                                send_alert("error", "tcp_test_failed");
                            }
                        }
                    }
                    None => {
                        warn!("TCP test requested but buffers unavailable");
                        send_alert("error", "ota_unavailable_no_flash");
                    }
                }
                // Do NOT reset — continue normal operation
            } else {
                match (ota.as_mut(), ota_buffers.as_mut()) {
                    (Some(o), Some(b)) => {
                        info!("OTA: starting firmware download from main loop");
                        if let Err(()) = ota::perform_ota_update(wifi_stack.stack, o, &firmware_url, b, || wdt.feed()).await {
                            error!("OTA update failed");
                            send_alert("error", "ota_update_failed");
                        }
                        // If we get here without resetting, something went very wrong
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

        // Drain pump timer commands
        while let Ok((pump_index, minutes)) = pump_timer_rx.try_receive() {
            let actions = app.start_pump_timer(pump_index, minutes);
            execute_actions(&actions, device_id_str).await;
            info!("Started pump {} timer for {} min", pump_index, minutes);
        }
    }
}
