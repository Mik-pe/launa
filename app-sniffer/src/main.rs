//! Launa RS-485 Sniffer — passive bus monitor that publishes decoded frames to MQTT.
//!
//! Third device on the RS-485 bus that only listens. Never transmits.
//! Decodes all Balboa protocol frames and publishes them to MQTT in the
//! same burst-capture JSON format as the main app's sniffer, so
//! `cargo xtask sniff-decode` works with it out of the box.
//!
//! **Early capture**: UART listening starts immediately at boot, before WiFi
//! connects. Frames are buffered in RAM and flushed to MQTT once the
//! connection is established. This captures the topside panel's registration
//! handshake during spa power-up.
//!
//! GPIO17 = TX (unused, but configured), GPIO16 = RX (UART1, 115200 baud)
//!
//! MQTT topics:
//!   launa/sniffer/sniff   — burst capture JSON (compatible with sniff-decode)
//!   launa/sniffer/status  — sniffer health/diagnostics

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::Read;
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::uart::Uart;
use launa_app_common::wifi::wifi_init;
use launa_app_common::MqttStateCore;
use launa_mqtt::mqtt_codec::ConnectConfig;
use launa_protocol::dispatcher::dispatch_frame;
use launa_protocol::frame::FrameDecoder;
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

const FIRMWARE_VERSION: &str =
    concat!("launa-sniffer ", env!("CARGO_PKG_VERSION"), " (", env!("GIT_SHORT_SHA"), ")");

// Build-time WiFi/MQTT config (set by xtask via env vars)
const WIFI_SSID: &str = env!("LAUNA_WIFI_SSID");
const WIFI_PASSWORD: &str = env!("LAUNA_WIFI_PASSWORD");
const MQTT_HOST: &str = env!("LAUNA_MQTT_HOST");
const MQTT_PORT: &str = env!("LAUNA_MQTT_PORT");

/// Accumulate frames for this long before publishing a burst.
const BURST_INTERVAL: Duration = Duration::from_secs(1);
/// Maximum frames per burst before forcing a publish.
const MAX_BURST_FRAMES: usize = 100;
/// Maximum frames to buffer before WiFi/MQTT is ready.
/// At ~80 bytes per frame struct, 500 frames ≈ 40 KiB on a 72 KiB heap.
const BOOT_BUFFER_MAX_FRAMES: usize = 500;

// ── Frame capture ────────────────────────────────────────────────────

/// A decoded protocol frame captured from the bus.
struct CapturedFrame {
    ts_us: u64,
    message_type: [u8; 2],
    payload: Vec<u8>,
}

/// Raw bytes captured between valid frame boundaries (garbage/collisions).
struct GarbageBytes {
    ts_us: u64,
    bytes: Vec<u8>,
}

/// Either a decoded frame or raw garbage bytes from the bus.
enum CaptureEntry {
    Frame(CapturedFrame),
    Garbage(GarbageBytes),
}

/// Tracks raw bytes on the RS-485 bus to detect garbage between valid frames.
///
/// Works alongside `FrameDecoder`. Watches for `0x7E` frame markers and
/// accumulates raw bytes inside frame boundaries. When the decoder reports
/// an error (bad CRC, bad length), or bytes appear outside any frame, they
/// are emitted as `GarbageBytes` entries.
struct RawBusTracker {
    /// Bytes accumulated inside the current 0x7E...0x7E boundary.
    pending: Vec<u8>,
    /// Whether we're inside a frame (saw start 0x7E, waiting for end 0x7E).
    in_frame: bool,
    /// Timestamp (us) when the current garbage sequence started.
    garbage_start_us: Option<u64>,
    /// Maximum garbage bytes to buffer before forcing an emit.
    max_garbage: usize,
}

impl RawBusTracker {
    fn new() -> Self {
        RawBusTracker {
            pending: Vec::new(),
            in_frame: false,
            garbage_start_us: None,
            max_garbage: 64,
        }
    }

    /// Feed a slice of raw UART bytes. Returns garbage entries for any
    /// bytes that fall outside valid frame boundaries.
    ///
    /// The caller should also feed the same bytes to `FrameDecoder` to get
    /// the decoded frames. This tracker only captures interstitial garbage.
    fn feed(&mut self, data: &[u8], ts_us: u64) -> Vec<GarbageBytes> {
        let mut garbage = Vec::new();

        for &byte in data {
            if byte == 0x7E {
                if self.in_frame {
                    // End of frame boundary. The pending bytes are the frame body
                    // (which FrameDecoder will parse). Clear pending — not garbage.
                    self.pending.clear();
                    self.in_frame = false;
                    self.garbage_start_us = None;
                } else {
                    // Start of a new frame boundary.
                    // Any pending bytes before this are inter-frame garbage.
                    if !self.pending.is_empty() {
                        garbage.push(GarbageBytes {
                            ts_us: self.garbage_start_us.unwrap_or(ts_us),
                            bytes: core::mem::take(&mut self.pending),
                        });
                        self.garbage_start_us = None;
                    }
                    self.in_frame = true;
                }
            } else if self.in_frame {
                // Inside a frame boundary — accumulate (will be cleared on end 0x7E).
                self.pending.push(byte);
            } else {
                // Outside any frame boundary — this is inter-frame garbage.
                if self.garbage_start_us.is_none() {
                    self.garbage_start_us = Some(ts_us);
                }
                self.pending.push(byte);

                // Force emit if buffer grows too large
                if self.pending.len() >= self.max_garbage {
                    garbage.push(GarbageBytes {
                        ts_us: self.garbage_start_us.unwrap_or(ts_us),
                        bytes: core::mem::take(&mut self.pending),
                    });
                    self.garbage_start_us = None;
                }
            }
        }

        garbage
    }

    /// Flush any remaining pending garbage at the end of a burst.
    fn flush(&mut self, ts_us: u64) -> Option<GarbageBytes> {
        if self.pending.is_empty() {
            None
        } else {
            let garbage = GarbageBytes {
                ts_us: self.garbage_start_us.unwrap_or(ts_us),
                bytes: core::mem::take(&mut self.pending),
            };
            self.garbage_start_us = None;
            self.in_frame = false;
            Some(garbage)
        }
    }
}

fn build_burst_json(entries: &[CaptureEntry], capture_us: u64) -> Vec<u8> {
    let frame_count = entries.iter().filter(|e| matches!(e, CaptureEntry::Frame(_))).count();
    let mut json = String::with_capacity(80 + entries.len() * 40);
    json.push_str("{\"capture_us\":");
    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", capture_us));
    json.push_str(",\"frame_count\":");
    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", frame_count));
    json.push_str(",\"entries\":[");

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        match entry {
            CaptureEntry::Frame(frame) => {
                // [ts_us, "TYPE", "PAYLOAD_HEX"]
                json.push('[');
                let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", frame.ts_us));
                json.push(',');
                let _ = core::fmt::Write::write_fmt(
                    &mut json,
                    core::format_args!("\"{:02X}{:02X}\"", frame.message_type[0], frame.message_type[1]),
                );
                json.push_str(",\"");
                for b in &frame.payload {
                    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{:02X}", b));
                }
                json.push_str("\"]");
            }
            CaptureEntry::Garbage(garbage) => {
                // [ts_us, "garbage", "RAW_HEX"]
                json.push('[');
                let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", garbage.ts_us));
                json.push_str(",\"garbage\",\"");
                for b in &garbage.bytes {
                    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{:02X}", b));
                }
                json.push_str("\"]");
            }
        }
    }

    json.push_str("]}");
    json.into_bytes()
}

/// Produce a short human-readable description of a decoded message.
fn describe_message(msg: &launa_protocol::dispatcher::IncomingMessage) -> &'static str {
    use launa_protocol::dispatcher::IncomingMessage;
    match msg {
        IncomingMessage::StatusUpdate(_) => "Status",
        IncomingMessage::Ready => "Ready",
        IncomingMessage::ConfigurationResponse(_) => "Config",
        IncomingMessage::ControlConfiguration(_) => "CtrlConfig",
        IncomingMessage::InformationResponse(_) => "Info",
        IncomingMessage::FaultLogResponse(_) => "Fault",
        IncomingMessage::FilterCyclesResponse(_) => "Filter",
        IncomingMessage::Registration(_) => "Registration",
        IncomingMessage::PreferencesResponse { .. } => "Preferences",
        IncomingMessage::SetupParametersResponse { .. } => "SetupParams",
        IncomingMessage::Unknown { .. } => "Unknown",
        _ => "?",
    }
}

// ── MQTT (crate-specific wrapper) ────────────────────────────────────

const SNIFF_TOPIC: &str = "launa/sniffer/sniff";
const STATUS_TOPIC: &str = "launa/sniffer/status";

struct MqttState {
    core: MqttStateCore,
}

impl MqttState {
    fn new(stack: &'static embassy_net::Stack<'static>) -> Self {
        MqttState {
            core: MqttStateCore::new(stack),
        }
    }

    fn is_connected(&self) -> bool {
        self.core.is_connected()
    }

    async fn connect(&mut self) -> bool {
        let port: u16 = MQTT_PORT.parse().unwrap_or(1883);
        if self.core.connect_tcp(MQTT_HOST, port).await.is_err() {
            return false;
        }

        let config = ConnectConfig {
            client_id: "launa_sniffer",
            lwt_topic: STATUS_TOPIC,
            username: None,
            password: None,
            keep_alive: launa_app_common::MQTT_KEEP_ALIVE_SECS,
        };
        self.core.mqtt_connect_handshake(&config, MQTT_HOST, port).await
    }

    async fn publish(&mut self, topic: &str, payload: &[u8]) -> bool {
        self.core.publish(topic, payload).await
    }

    async fn maybe_ping(&mut self) -> bool {
        self.core.maybe_ping().await
    }

    /// Non-blocking read to drain any incoming MQTT packets (PINGREQ from broker, etc).
    async fn drain_incoming(&mut self) {
        let Some(transport) = self.core.transport.as_mut() else { return };
        let mut tmp = [0u8; 128];
        match select(transport.read(&mut tmp), Timer::after(Duration::from_millis(5))).await {
            Either::First(Ok(n)) if n > 0 => {
                // Minimal handling: respond to PINGREQ
                if n >= 2 && tmp[0] >> 4 == 12 {
                    let resp = launa_mqtt::mqtt_codec::encode_pingresp();
                    let _ = self.core.send_bytes(&resp).await;
                }
            }
            _ => {}
        }
    }
}

// ── UART capture helper ──────────────────────────────────────────────

/// Read UART, decode frames, and append to the burst buffer.
/// Also tracks interstitial garbage bytes via the RawBusTracker.
/// Returns the number of new entries (frames + garbage) decoded.
fn capture_uart(
    uart: &mut Uart<'static, esp_hal::Async>,
    decoder: &mut FrameDecoder,
    raw_tracker: &mut RawBusTracker,
    buf: &mut [u8],
    entries: &mut Vec<CaptureEntry>,
    burst_start: Instant,
    total_frames: &mut u64,
    total_bytes: &mut u64,
    total_errors: &mut u64,
    total_garbage: &mut u64,
    log_frames: bool,
) -> usize {
    let mut new_entries = 0;

    // Non-blocking read from UART
    let n = match uart.read(buf) {
        Ok(n) => n,
        Err(_) => {
            let mut drain = [0u8; 32];
            let _ = uart.read_buffered(&mut drain);
            return 0;
        }
    };

    if n == 0 {
        return 0;
    }

    *total_bytes += n as u64;
    let ts_us = burst_start.elapsed().as_micros();

    // Track raw bytes for garbage detection
    for g in raw_tracker.feed(&buf[..n], ts_us) {
        *total_garbage += 1;
        new_entries += 1;
        if log_frames {
            info!(
                "GARBAGE ({} bytes): {}",
                g.bytes.len(),
                launa_protocol::hex::to_hex(&g.bytes[..g.bytes.len().min(32)])
            );
        }
        entries.push(CaptureEntry::Garbage(g));
    }

    // Decode frames
    let prev_errors = decoder.frame_error_count() as u64;
    let frames = decoder.feed_slice(&buf[..n]);
    let new_error_count = decoder.frame_error_count() as u64;
    if new_error_count > prev_errors {
        *total_errors += new_error_count - prev_errors;
    }

    for frame in &frames {
        *total_frames += 1;
        new_entries += 1;

        let ts_us = burst_start.elapsed().as_micros();

        if log_frames {
            let msg = dispatch_frame(frame);
            let desc = describe_message(&msg);
            let mt = format!(
                "{:02X}{:02X}",
                frame.message_type[0], frame.message_type[1]
            );
            let payload_hex =
                launa_protocol::hex::to_hex(&frame.payload[..frame.payload.len().min(32)]);
            info!(
                "FRAME {} ({}, {} bytes): {}",
                mt,
                desc,
                frame.payload.len(),
                payload_hex
            );
        }

        entries.push(CaptureEntry::Frame(CapturedFrame {
            ts_us,
            message_type: frame.message_type,
            payload: frame.payload.clone(),
        }));
    }

    new_entries
}

/// Count frames in a list of CaptureEntry.
fn count_frames(entries: &[CaptureEntry]) -> usize {
    entries.iter().filter(|e| matches!(e, CaptureEntry::Frame(_))).count()
}

/// Trim entries to keep within budget, preferring to keep garbage (rare) over frames (common).
fn trim_entries(entries: &mut Vec<CaptureEntry>, max: usize) {
    if entries.len() <= max {
        return;
    }
    // Remove oldest frame entries first, keep garbage
    let excess = entries.len() - max;
    let mut removed = 0;
    entries.retain(|e| {
        if removed >= excess {
            return true;
        }
        if matches!(e, CaptureEntry::Frame(_)) {
            removed += 1;
            false
        } else {
            true
        }
    });
}

// ── Main ─────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_println::logger::init_logger(log::LevelFilter::Info);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    info!("=== Launa RS-485 Sniffer v{} ===", FIRMWARE_VERSION);
    info!("UART1 (TX=GPIO17, RX=GPIO16), 115200 baud — LISTEN ONLY");

    // ── Phase 1: Start UART immediately (before WiFi) ────────────────
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let mut uart = Uart::new(peripherals.UART1, uart_config)
        .expect("UART1 init failed")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    info!("UART1 ready — capturing from power-on");

    // Drain stale bytes
    let mut drain = [0u8; 64];
    let _ = uart.read_buffered(&mut drain);

    let mut decoder = FrameDecoder::new();
    let mut raw_tracker = RawBusTracker::new();
    let mut read_buf = [0u8; 128];
    let boot_start = Instant::now();

    // Boot buffer: accumulate entries while WiFi/MQTT connects
    let mut boot_entries: Vec<CaptureEntry> = Vec::new();
    let mut total_frames: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_errors: u64 = 0;
    let mut total_garbage: u64 = 0;

    // ── Phase 2: Start WiFi, continue capturing during connect ───────
    let rng = Rng::new();
    let net_stack = wifi_init(spawner, peripherals.WIFI, rng, WIFI_SSID, WIFI_PASSWORD, "launa-sniffer");
    info!("WiFi connecting, still capturing...");

    // Capture frames while waiting for DHCP
    loop {
        if net_stack.is_config_up() {
            break;
        }

        capture_uart(
            &mut uart,
            &mut decoder,
            &mut raw_tracker,
            &mut read_buf,
            &mut boot_entries,
            boot_start,
            &mut total_frames,
            &mut total_bytes,
            &mut total_errors,
            &mut total_garbage,
            true,
        );

        trim_entries(&mut boot_entries, BOOT_BUFFER_MAX_FRAMES);

        Timer::after(Duration::from_millis(10)).await;
    }

    if let Some(cfg) = net_stack.config_v4() {
        info!("Got IP: {}", cfg.address);
    }

    // ── Phase 3: Connect MQTT, continue capturing during handshake ───
    let mut mqtt = MqttState::new(net_stack);
    let mut mqtt_connected = false;

    let mut mqtt_reconnect_at = Instant::now();
    let mut mqtt_attempts: u32 = 0;

    while !mqtt_connected {
        capture_uart(
            &mut uart,
            &mut decoder,
            &mut raw_tracker,
            &mut read_buf,
            &mut boot_entries,
            boot_start,
            &mut total_frames,
            &mut total_bytes,
            &mut total_errors,
            &mut total_garbage,
            true,
        );

        trim_entries(&mut boot_entries, BOOT_BUFFER_MAX_FRAMES);

        if Instant::now() >= mqtt_reconnect_at {
            mqtt_attempts += 1;
            info!("MQTT connect attempt {}...", mqtt_attempts);
            if mqtt.connect().await {
                mqtt_connected = true;
                info!("MQTT ready");
            } else {
                warn!("MQTT connect failed, retry in 5s");
                mqtt_reconnect_at = Instant::now() + Duration::from_secs(5);
            }
        }

        Timer::after(Duration::from_millis(50)).await;
    }

    // ── Phase 4: Flush boot buffer to MQTT ───────────────────────────
    let boot_frame_count;
    if !boot_entries.is_empty() {
        // Flush any pending garbage from the tracker
        let ts_us = boot_start.elapsed().as_micros();
        if let Some(g) = raw_tracker.flush(ts_us) {
            boot_entries.push(CaptureEntry::Garbage(g));
        }

        let capture_us = boot_start.elapsed().as_micros();
        let json = build_burst_json(&boot_entries, capture_us);
        boot_frame_count = count_frames(&boot_entries);
        let garbage_count = boot_entries.len() - boot_frame_count;
        info!(
            "Flushing boot buffer: {} frames + {} garbage entries ({}ms)",
            boot_frame_count,
            garbage_count,
            capture_us / 1000
        );

        if mqtt.publish(SNIFF_TOPIC, &json).await {
            info!("Boot buffer published: {} entries, {} bytes", boot_entries.len(), json.len());
        } else {
            warn!("Boot buffer publish failed, entries lost");
            mqtt.core.disconnect();
        }
        boot_entries.clear();
    } else {
        boot_frame_count = 0;
        info!("No frames captured during boot");
    }

    // Publish initial status
    {
        let uptime_secs = boot_start.elapsed().as_secs();
        let status_json = format!(
            r#"{{"frames":{},"bytes":{},"errors":{},"garbage":{},"uptime_secs":{},"version":"{}","boot_capture_frames":{}}}"#,
            total_frames, total_bytes, total_errors, total_garbage, uptime_secs, FIRMWARE_VERSION, boot_frame_count,
        );
        let _ = mqtt.publish(STATUS_TOPIC, status_json.as_bytes()).await;
    }

    // ── Phase 5: Normal burst-capture loop ───────────────────────────
    let mut burst_entries: Vec<CaptureEntry> = Vec::new();
    let mut burst_start = Instant::now();
    let start_time = Instant::now();

    mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);

    info!("Continuous capture started, publishing to {}", SNIFF_TOPIC);

    loop {
        // ── Read UART (continuous, non-blocking with short timeout) ──
        match select(uart.read_async(&mut read_buf), Timer::after(Duration::from_millis(50))).await
        {
            Either::First(Ok(n)) if n > 0 => {
                total_bytes += n as u64;
                let ts_us = burst_start.elapsed().as_micros();

                // Track raw bytes for garbage detection
                for g in raw_tracker.feed(&read_buf[..n], ts_us) {
                    total_garbage += 1;
                    info!(
                        "GARBAGE ({} bytes): {}",
                        g.bytes.len(),
                        launa_protocol::hex::to_hex(&g.bytes[..g.bytes.len().min(32)])
                    );
                    burst_entries.push(CaptureEntry::Garbage(g));
                }

                // Decode frames
                let prev_errors = decoder.frame_error_count() as u64;
                let frames = decoder.feed_slice(&read_buf[..n]);
                let new_errors = decoder.frame_error_count() as u64;
                if new_errors > prev_errors {
                    total_errors += new_errors - prev_errors;
                }

                if burst_entries.is_empty() {
                    burst_start = Instant::now();
                }

                for frame in &frames {
                    total_frames += 1;

                    let ts_us = burst_start.elapsed().as_micros();
                    let msg = dispatch_frame(frame);
                    let desc = describe_message(&msg);

                    let mt = format!(
                        "{:02X}{:02X}",
                        frame.message_type[0], frame.message_type[1]
                    );
                    let payload_hex =
                        launa_protocol::hex::to_hex(&frame.payload[..frame.payload.len().min(32)]);
                    info!(
                        "FRAME {} ({}, {} bytes): {}",
                        mt,
                        desc,
                        frame.payload.len(),
                        payload_hex
                    );

                    burst_entries.push(CaptureEntry::Frame(CapturedFrame {
                        ts_us,
                        message_type: frame.message_type,
                        payload: frame.payload.clone(),
                    }));
                }
            }
            Either::First(Err(_)) => {
                let mut drain = [0u8; 32];
                let _ = uart.read_buffered(&mut drain);
            }
            Either::Second(_) => {}
            _ => {}
        }

        // ── Publish burst when interval elapsed or buffer full ──
        let burst_elapsed = burst_start.elapsed() >= BURST_INTERVAL;
        let entry_count = burst_entries.len();
        let burst_full = entry_count >= MAX_BURST_FRAMES;

        if !burst_entries.is_empty() && (burst_elapsed || burst_full) {
            // Flush any pending garbage
            let ts_us = burst_start.elapsed().as_micros();
            if let Some(g) = raw_tracker.flush(ts_us) {
                burst_entries.push(CaptureEntry::Garbage(g));
            }

            let capture_us = burst_start.elapsed().as_micros();
            let json = build_burst_json(&burst_entries, capture_us);
            let frame_count = count_frames(&burst_entries);
            let garbage_count = burst_entries.len() - frame_count;
            burst_entries.clear();

            // MQTT keepalive
            if mqtt.is_connected() {
                if !mqtt.maybe_ping().await {
                    warn!("MQTT ping failed");
                    mqtt.core.disconnect();
                } else {
                    mqtt.drain_incoming().await;
                }
            }

            // Reconnect if needed
            if !mqtt.is_connected() && Instant::now() >= mqtt_reconnect_at {
                if mqtt.connect().await {
                    info!("MQTT reconnected");
                } else {
                    warn!("MQTT reconnect failed, retry in 30s");
                    mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
                }
            }

            // Publish burst capture
            if mqtt.is_connected() {
                if mqtt.publish(SNIFF_TOPIC, &json).await {
                    let garb_str = if garbage_count > 0 {
                        alloc::format!(" + {} garbage", garbage_count)
                    } else {
                        String::new()
                    };
                    info!("Published burst: {} frames{} ({} bytes)", frame_count, garb_str, json.len());
                } else {
                    warn!("MQTT sniff publish failed");
                    mqtt.core.disconnect();
                    mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
                }
            }

            // Publish status diagnostics every ~100 frames
            if total_frames % 100 < frame_count as u64 && mqtt.is_connected() {
                let uptime_secs = start_time.elapsed().as_secs();
                let status_json = format!(
                    r#"{{"frames":{},"bytes":{},"errors":{},"garbage":{},"uptime_secs":{},"version":"{}"}}"#,
                    total_frames, total_bytes, total_errors, total_garbage, uptime_secs, FIRMWARE_VERSION
                );
                let _ = mqtt.publish(STATUS_TOPIC, status_json.as_bytes()).await;
            }
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    let mut counter: u32 = 0;
    while counter < 10_000_000 {
        counter += 1;
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
    esp_hal::system::software_reset()
}
