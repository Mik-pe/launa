//! Launa RS-485 Sniffer — passive bus monitor that publishes raw byte captures to MQTT.
//!
//! Third device on the RS-485 bus that only listens. Never transmits.
//! Captures raw UART byte chunks with timestamps and publishes them as JSON.
//! Frame decoding and garbage detection are done by the host-side decoder
//! (`cargo xtask sniff-decode` or the web frontend), not in firmware.
//!
//! **Early capture**: UART listening starts immediately at boot, before WiFi
//! connects. Raw bytes are buffered in RAM and flushed to MQTT once the
//! connection is established. This captures the topside panel's registration
//! handshake during spa power-up.
//!
//! GPIO17 = TX (unused, but configured), GPIO16 = RX (UART1, 115200 baud)
//!
//! MQTT topics:
//!   launa/sniffer/sniff   — raw byte burst capture JSON (compatible with sniff-decode)
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
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::uart::Uart;
use embedded_io_async::Read;
use launa_app_common::wifi::wifi_init;
use launa_app_common::MqttStateCore;
use launa_mqtt::mqtt_codec::ConnectConfig;
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

/// Accumulate raw chunks for this long before publishing a burst.
const BURST_INTERVAL: Duration = Duration::from_secs(1);
/// Maximum total hex bytes per burst before forcing a publish.
const MAX_BURST_HEX_LEN: usize = 8000;
/// Maximum total hex bytes to buffer before WiFi/MQTT is ready.
/// At 2 chars per byte, 20000 hex chars ≈ 10KB on a 72 KiB heap.
const BOOT_BUFFER_MAX_HEX: usize = 20_000;

// ── Raw chunk capture ────────────────────────────────────────────────

/// A raw byte chunk captured from the RS-485 bus with timestamp.
struct RawChunk {
    ts_us: u64,
    hex: String,
}

/// Build the JSON burst capture payload from collected chunks.
///
/// Format: `{"capture_us":N,"chunks":[["R",ts,"HEX"],...]}`
/// The sniffer is RX-only, so all chunks have direction "R".
fn build_burst_json(chunks: &[RawChunk], capture_us: u64) -> Vec<u8> {
    let mut json = String::with_capacity(80 + chunks.len() * 60);
    json.push_str("{\"capture_us\":");
    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", capture_us));
    json.push_str(",\"chunks\":[");

    for (i, c) in chunks.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("[\"R\",");
        let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", c.ts_us));
        json.push_str(",\"");
        json.push_str(&c.hex);
        json.push_str("\"]");
    }

    json.push_str("]}");
    json.into_bytes()
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

/// Read UART and append raw byte chunks to the burst buffer.
/// Returns the number of bytes captured.
fn capture_uart_raw(
    uart: &mut Uart<'static, esp_hal::Async>,
    buf: &mut [u8],
    chunks: &mut Vec<RawChunk>,
    total_hex_len: &mut usize,
    burst_start: Instant,
    total_bytes: &mut u64,
) -> usize {
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
    let hex = launa_protocol::hex::to_hex(&buf[..n]);
    *total_hex_len += hex.len();

    info!("RX {} bytes at +{}us", n, ts_us);

    chunks.push(RawChunk { ts_us, hex });

    n
}

/// Trim chunks to keep total hex length within budget, dropping oldest first.
fn trim_chunks(chunks: &mut Vec<RawChunk>, max_hex: usize, total_hex_len: &mut usize) {
    while *total_hex_len > max_hex && !chunks.is_empty() {
        let removed = chunks.remove(0);
        *total_hex_len -= removed.hex.len();
    }
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

    let mut read_buf = [0u8; 128];
    let boot_start = Instant::now();

    // Boot buffer: accumulate raw chunks while WiFi/MQTT connects
    let mut boot_chunks: Vec<RawChunk> = Vec::new();
    let mut boot_hex_len: usize = 0;
    let mut total_bytes: u64 = 0;

    // We still need a decoder for local log output and frame/error counting
    let mut decoder = FrameDecoder::new();

    // ── Phase 2: Start WiFi, continue capturing during connect ───────
    let rng = Rng::new();
    let net_stack = wifi_init(spawner, peripherals.WIFI, rng, WIFI_SSID, WIFI_PASSWORD, "launa-sniffer");
    info!("WiFi connecting, still capturing...");

    // Capture raw bytes while waiting for DHCP
    loop {
        if net_stack.is_config_up() {
            break;
        }

        capture_uart_raw(
            &mut uart,
            &mut read_buf,
            &mut boot_chunks,
            &mut boot_hex_len,
            boot_start,
            &mut total_bytes,
        );

        // Also feed decoder for local logging
        let _ = decoder.feed_slice(&read_buf);

        trim_chunks(&mut boot_chunks, BOOT_BUFFER_MAX_HEX, &mut boot_hex_len);

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
        capture_uart_raw(
            &mut uart,
            &mut read_buf,
            &mut boot_chunks,
            &mut boot_hex_len,
            boot_start,
            &mut total_bytes,
        );

        trim_chunks(&mut boot_chunks, BOOT_BUFFER_MAX_HEX, &mut boot_hex_len);

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
    let boot_chunk_count = boot_chunks.len();
    if !boot_chunks.is_empty() {
        let capture_us = boot_start.elapsed().as_micros();
        let json = build_burst_json(&boot_chunks, capture_us);
        info!(
            "Flushing boot buffer: {} chunks, {}ms",
            boot_chunk_count,
            capture_us / 1000
        );

        if mqtt.publish(SNIFF_TOPIC, &json).await {
            info!("Boot buffer published: {} bytes", json.len());
        } else {
            warn!("Boot buffer publish failed, chunks lost");
            mqtt.core.disconnect();
        }
        boot_chunks.clear();
        boot_hex_len = 0;
    } else {
        info!("No data captured during boot");
    }

    // Publish initial status
    {
        let uptime_secs = boot_start.elapsed().as_secs();
        let status_json = format!(
            r#"{{"bytes":{},"uptime_secs":{},"version":"{}","boot_chunks":{}}}"#,
            total_bytes, uptime_secs, FIRMWARE_VERSION, boot_chunk_count,
        );
        let _ = mqtt.publish(STATUS_TOPIC, status_json.as_bytes()).await;
    }

    // ── Phase 5: Normal burst-capture loop ───────────────────────────
    let mut burst_chunks: Vec<RawChunk> = Vec::new();
    let mut burst_hex_len: usize = 0;
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

                if burst_chunks.is_empty() {
                    burst_start = Instant::now();
                }

                let ts_us = burst_start.elapsed().as_micros();
                let hex = launa_protocol::hex::to_hex(&read_buf[..n]);
                burst_hex_len += hex.len();

                info!("RX {} bytes at +{}us", n, ts_us);

                burst_chunks.push(RawChunk { ts_us, hex });

                // Also feed decoder for local log output
                let frames = decoder.feed_slice(&read_buf[..n]);
                for frame in &frames {
                    let mt = format!(
                        "{:02X}{:02X}",
                        frame.message_type[0], frame.message_type[1]
                    );
                    info!("Decoded: {} ({} bytes)", mt, frame.payload.len());
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
        let burst_full = burst_hex_len >= MAX_BURST_HEX_LEN;

        if !burst_chunks.is_empty() && (burst_elapsed || burst_full) {
            let capture_us = burst_start.elapsed().as_micros();
            let json = build_burst_json(&burst_chunks, capture_us);
            let chunk_count = burst_chunks.len();
            burst_chunks.clear();
            burst_hex_len = 0;

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
                    info!("Published burst: {} chunks ({} bytes)", chunk_count, json.len());
                } else {
                    warn!("MQTT sniff publish failed");
                    mqtt.core.disconnect();
                    mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
                }
            }

            // Publish status diagnostics periodically
            if mqtt.is_connected() {
                let uptime_secs = start_time.elapsed().as_secs();
                let status_json = format!(
                    r#"{{"bytes":{},"uptime_secs":{},"version":"{}"}}"#,
                    total_bytes, uptime_secs, FIRMWARE_VERSION
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
