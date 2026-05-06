//! Launa RS-485 Debugger — slot-driven round-robin TX/RX for 2 devices.
//!
//! Each device identifies itself as A or B from its MAC address.
//! Devices cycle through 1-second slots using a local counter:
//!   slot_counter % 2 == 0 → A transmits
//!   slot_counter % 2 == 1 → B transmits
//!
//! Devices self-synchronize by resetting their slot counter when they
//! receive a frame from the other device. If no sync is received within
//! 3 cycles, the device free-runs on its own slot.
//!
//! WiFi + MQTT is used to publish test status so we can observe results
//! without relying on garbled serial output from auto-direction transceivers.

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
use esp_hal::gpio::{AnyPin, Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use esp_hal::uart::Uart;
use esp_hal::Async;
use launa_app_common::wifi::wifi_init;
use launa_app_common::MqttStateCore;
use launa_mqtt::mqtt_codec::ConnectConfig;
use launa_protocol::frame::FrameDecoder;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

// Build-time WiFi/MQTT config (set by xtask via env vars)
const WIFI_SSID: &str = env!("LAUNA_WIFI_SSID");
const WIFI_PASSWORD: &str = env!("LAUNA_WIFI_PASSWORD");
const MQTT_HOST: &str = env!("LAUNA_MQTT_HOST");
const MQTT_PORT: &str = env!("LAUNA_MQTT_PORT");

const FIRMWARE_VERSION: &str =
    concat!("rs485-debugger ", env!("CARGO_PKG_VERSION"), " (", env!("GIT_SHORT_SHA"), ")");

const SLOT_DURATION: Duration = Duration::from_secs(1);
const NUM_DEVICES: u8 = 2;
const TX_OFFSET: Duration = Duration::from_millis(100);
const POST_TX_GUARD: Duration = Duration::from_millis(10);
const SYNC_TIMEOUT_CYCLES: u32 = 3; // Free-run after 3 cycles (9s) with no RX sync;

/// Device index derived from MAC: A=0, B=1
struct Device {
    id: &'static str,
    index: u8,
}

impl Device {
    fn from_mac() -> Self {
        let mac = esp_hal::efuse::base_mac_address();
        let bytes = mac.as_bytes();
        match (bytes[4], bytes[5]) {
            (0x83, 0xC8) => Device { id: "A", index: 0 },
            (0x12, 0xBC) => Device { id: "B", index: 1 },
            _ => {
                let b4 = bytes[4];
                let b5 = bytes[5];
                panic!(
                    "Unknown MAC {:02X}:{:02X} — device identity cannot be determined",
                    b4, b5
                );
            }
        }
    }
}

macro_rules! dlog {
    ($dev:expr, $($arg:tt)*) => {
        log::info!("[{}] {}", $dev.id, alloc::format!($($arg)*))
    };
}

macro_rules! dwarn {
    ($dev:expr, $($arg:tt)*) => {
        log::warn!("[{}] {}", $dev.id, alloc::format!($($arg)*))
    };
}

// ── RS-485 Transport ─────────────────────────────────────────────────

/// RS-485 half-duplex UART transport.
struct Rs485Transport {
    uart: Uart<'static, Async>,
    de_pin: Option<Output<'static>>,
}

impl Rs485Transport {
    fn new(uart: Uart<'static, Async>, de_pin: Option<AnyPin<'static>>) -> Self {
        let de = de_pin.map(|pin| Output::new(pin, Level::Low, OutputConfig::default()));
        Rs485Transport { uart, de_pin: de }
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        self.uart.read_async(buf).await.map_err(|_| ())
    }

    async fn write_frame(&mut self, data: &[u8]) -> Result<(), ()> {
        if let Some(de) = self.de_pin.as_mut() {
            de.set_high();
            Timer::after(Duration::from_micros(50)).await;
        }
        let mut written = 0;
        while written < data.len() {
            let n = self.uart.write(&data[written..]).map_err(|_| ())?;
            written += n;
        }
        self.uart.flush().map_err(|_| ())?;
        if let Some(de) = self.de_pin.as_mut() {
            de.set_low();
        }
        Timer::after(Duration::from_micros(5000)).await;
        Ok(())
    }
}

/// Build a simple test frame: `7E <len> FE <device_index> <payload...> <crc> 7E`
fn build_test_frame(device: &Device, seq: u32) -> Vec<u8> {
    let payload = [
        0xFE,                       // type_hi (registration-related, identifiable)
        device.index,               // who sent it
        (seq >> 8) as u8,           // sequence high byte
        (seq & 0xFF) as u8,         // sequence low byte
    ];
    let len = (payload.len() + 2) as u8;

    let mut frame = Vec::new();
    frame.push(0x7E);
    frame.push(len);
    frame.extend_from_slice(&payload);
    let crc = launa_protocol::crc8::compute(&frame[1..]);
    frame.push(crc);
    frame.push(0x7E);
    frame
}

/// Feed bytes through the frame decoder and return (sender_id, sender_index).
fn decode_sender(decoder: &mut FrameDecoder, bytes: &[u8]) -> (&'static str, Option<u8>) {
    let mut sender = "?";
    let mut sender_index = None;
    for &byte in bytes {
        if let Some(frame) = decoder.feed(byte) {
            if frame.message_type[0] == 0xFE {
                let src_index = frame.message_type[1];
                sender_index = Some(src_index);
                sender = match src_index {
                    0 => "A",
                    1 => "B",
                    _ => "?",
                };
            }
        }
    }
    (sender, sender_index)
}

/// Re-sync slot counter based on a received frame's sender index.
/// Sender just finished its slot, so our next slot starts at sender_index + 1.
fn sync_slot(sender_index: u8) -> (u8, embassy_time::Instant) {
    let new_counter = sender_index.wrapping_add(1);
    let new_deadline = embassy_time::Instant::now() + SLOT_DURATION;
    (new_counter, new_deadline)
}

/// RX statistics tracked across the main loop.
struct RxStats {
    rx_count: u64,
    total_rx_bytes: u64,
    last_rx_sender: &'static str,
    last_rx_hex: String,
}

/// Context for the RS-485 main loop's mutable state, extracted to avoid
/// passing 10+ &mut references to `handle_rx_bytes`.
struct SlotState {
    slot_counter: u8,
    slot_deadline: embassy_time::Instant,
    synced: bool,
    cycles_since_last_rx: u32,
}

/// Process received bytes: decode frames, log, update stats, and re-sync slots.
/// `check_crc` enables CRC error logging (used in RX-slot, not TX-slot).
fn handle_rx_bytes(
    dev: &Device,
    decoder: &mut FrameDecoder,
    data: &[u8],
    stats: &mut RxStats,
    slot: &mut SlotState,
    label: &str,
    check_crc: bool,
) {
    if data.iter().all(|&b| b == 0x00 || b == 0xFF) {
        return;
    }
    stats.total_rx_bytes += data.len() as u64;
    stats.rx_count += 1;
    let hex = launa_protocol::hex::to_hex(&data[..data.len().min(32)]);
    let prev_errors = if check_crc { decoder.frame_error_count() } else { 0 };
    let (sender, sender_idx) = decode_sender(decoder, data);
    let new_errors = if check_crc { decoder.frame_error_count() } else { 0 };
    stats.last_rx_sender = sender;
    stats.last_rx_hex = hex.clone();

    if check_crc && new_errors > prev_errors {
        dlog!(dev, "RX ({}) from [{}]: {} bytes: {} (crc err)", label, sender, data.len(), hex);
    } else {
        dlog!(dev, "RX ({}) from [{}]: {} bytes: {}", label, sender, data.len(), hex);
    }

    if let Some(idx) = sender_idx {
        if idx != dev.index {
            let (new_ctr, new_dl) = sync_slot(idx);
            slot.slot_counter = new_ctr;
            slot.slot_deadline = new_dl;
            if !slot.synced {
                slot.synced = true;
                dlog!(dev, "SYNC: locked to peer [{}]", sender);
            }
            slot.cycles_since_last_rx = 0;
            dlog!(dev, "SYNC: sender=[{}] idx={}, new_slot={}", sender, idx, slot.slot_counter);
        }
    }
}

// ── MQTT (crate-specific wrapper) ────────────────────────────────────

/// Minimal MQTT state: wraps MqttStateCore with a status topic.
struct MqttState {
    core: MqttStateCore,
    topic: String, // e.g. "launa/rs485_debugger/A/status"
}

impl MqttState {
    fn new(stack: &'static embassy_net::Stack<'static>, device_id: &str) -> Self {
        MqttState {
            core: MqttStateCore::new(stack),
            topic: format!("launa/rs485_debugger/{}/status", device_id),
        }
    }

    fn is_connected(&self) -> bool {
        self.core.is_connected()
    }

    /// Connect TCP + MQTT CONNECT handshake. Returns true on success.
    async fn connect(&mut self) -> bool {
        let port: u16 = MQTT_PORT.parse().unwrap_or(1883);
        if self.core.connect_tcp(MQTT_HOST, port).await.is_err() {
            return false;
        }

        // MQTT CONNECT with LWT on the status topic
        let client_id = format!("launa_rs485dbg_{}", self.topic.split('/').nth(2).unwrap_or("?"));
        let config = ConnectConfig {
            client_id: &client_id,
            lwt_topic: &self.topic,
            username: None,
            password: None,
            keep_alive: launa_app_common::MQTT_KEEP_ALIVE_SECS,
        };
        self.core.mqtt_connect_handshake(&config, MQTT_HOST, port).await
    }

    /// Publish a QoS 0 payload to the status topic. Non-blocking on failure.
    async fn publish_status(&mut self, payload: &[u8]) -> bool {
        self.core.publish(&self.topic, payload).await
    }

    /// Send keepalive PINGREQ if half the keepalive has elapsed.
    async fn maybe_ping(&mut self) -> bool {
        self.core.maybe_ping().await
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

    let dev = Device::from_mac();
    info!("=== [{}] RS-485 Debugger v{} ===", dev.id, FIRMWARE_VERSION);
    info!("[{}] UART1 (TX=GPIO17, RX=GPIO16), 115200 baud", dev.id);
    info!("[{}] Round-robin TX: A=slot 0, B=slot 1 (2s cycle)", dev.id);

    // ── UART1 for RS-485 ──
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("UART1 init failed")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    let mut transport = Rs485Transport::new(uart, None);
    dlog!(dev, "UART1 ready, auto-direction mode");

    // ── WiFi + MQTT ──
    info!("[{}] Connecting to SSID: {:?}", dev.id, WIFI_SSID);
    let rng = Rng::new();
    let net_stack = wifi_init(spawner, peripherals.WIFI, rng, WIFI_SSID, WIFI_PASSWORD, "launa-rs485dbg");
    info!("Waiting for DHCP...");
    net_stack.wait_config_up().await;
    if let Some(cfg) = net_stack.config_v4() {
        info!("Got IP: {}", cfg.address);
    }
    let mut mqtt = MqttState::new(net_stack, dev.id);

    // Initial MQTT connect (non-fatal if it fails)
    if mqtt.connect().await {
        dlog!(dev, "MQTT ready, topic: {}", mqtt.topic);
    } else {
        dwarn!(dev, "MQTT connect failed, will retry");
    }

    // ── RS-485 loop state ──
    let mut decoder = FrameDecoder::new();
    let mut read_buf = [0u8; 128];
    let mut seq: u32 = 0;
    let mut tx_count: u64 = 0;
    let start_time = Instant::now();

    let mut rx_stats = RxStats {
        rx_count: 0,
        total_rx_bytes: 0,
        last_rx_sender: "-",
        last_rx_hex: String::new(),
    };

    // Start on our own slot so we transmit immediately if no one else is heard
    let mut slot = SlotState {
        slot_counter: dev.index,
        slot_deadline: embassy_time::Instant::now() + SLOT_DURATION,
        synced: false,
        cycles_since_last_rx: 0,
    };
    let mut mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);

    loop {
        let slot_start = slot.slot_deadline - SLOT_DURATION;
        let is_my_slot = slot.slot_counter % NUM_DEVICES == dev.index;

        if is_my_slot {
            // ── TX slot ──────────────────────────────────────────────
            Timer::at(slot_start + TX_OFFSET).await;

            let frame = build_test_frame(&dev, seq);
            match transport.write_frame(&frame).await {
                Ok(()) => {
                    tx_count += 1;
                    let hex = launa_protocol::hex::to_hex(&frame);
                    dlog!(dev, "TX #{} ({} bytes): {}", seq, frame.len(), hex);
                    seq += 1;
                }
                Err(_) => {
                    dwarn!(dev, "TX FAILED #{}", seq);
                }
            }

            Timer::after(POST_TX_GUARD).await;

            // Drain echoed bytes
            let mut drain = [0u8; 64];
            loop {
                match transport.uart.read_buffered(&mut drain) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }

            // Listen for remainder of slot
            let listen_deadline = slot.slot_deadline - Duration::from_millis(5);
            while embassy_time::Instant::now() < listen_deadline {
                let remaining = listen_deadline - embassy_time::Instant::now();
                match select(transport.read(&mut read_buf), Timer::after(remaining)).await {
                    Either::First(Ok(n)) if n > 0 => {
                        handle_rx_bytes(&dev, &mut decoder, &read_buf[..n], &mut rx_stats, &mut slot, "post-TX", false);
                    }
                    Either::First(Err(_)) => {
                        let mut drain = [0u8; 32];
                        let _ = transport.uart.read_buffered(&mut drain);
                    }
                    _ => {}
                }
            }
        } else {
            // ── RX slot ──────────────────────────────────────────────
            let mut drain = [0u8; 64];
            let _ = transport.uart.read_buffered(&mut drain);

            let listen_deadline = slot.slot_deadline - Duration::from_millis(5);
            while embassy_time::Instant::now() < listen_deadline {
                let remaining = listen_deadline - embassy_time::Instant::now();
                match select(transport.read(&mut read_buf), Timer::after(remaining)).await {
                    Either::First(Ok(n)) if n > 0 => {
                        handle_rx_bytes(&dev, &mut decoder, &read_buf[..n], &mut rx_stats, &mut slot, "listen", true);
                    }
                    Either::First(Err(_)) => {
                        let mut drain = [0u8; 32];
                        let _ = transport.uart.read_buffered(&mut drain);
                        Timer::after(Duration::from_millis(1)).await;
                    }
                    _ => {}
                }
            }
        }

        // ── MQTT publish every 3 slots ────────────────────────────────
        if slot.slot_counter > 0 && slot.slot_counter % 3 == 0 {
            let uptime_secs = start_time.elapsed().as_secs();

            // MQTT keepalive ping
            if mqtt.is_connected() {
                if !mqtt.maybe_ping().await {
                    dwarn!(dev, "MQTT ping failed, disconnecting");
                    mqtt.core.disconnect();
                }
            }

            // Try to reconnect if disconnected (with backoff)
            if !mqtt.is_connected() && Instant::now() >= mqtt_reconnect_at {
                dlog!(dev, "MQTT reconnecting...");
                if mqtt.connect().await {
                    dlog!(dev, "MQTT reconnected");
                } else {
                    dwarn!(dev, "MQTT reconnect failed, retry in 30s");
                    mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
                }
            }

            // Publish status JSON
            if mqtt.is_connected() {
                // Truncate last_rx_hex to 64 chars to keep payload small
                let hex_display = if rx_stats.last_rx_hex.len() > 64 {
                    &rx_stats.last_rx_hex[..64]
                } else {
                    &rx_stats.last_rx_hex
                };
                let json = format!(
                    r#"{{"device_id":"{}","tx_count":{},"rx_count":{},"rx_bytes":{},"last_rx_sender":"{}","last_rx_hex":"{}","seq":{},"slot_counter":{},"synced":{},"uptime_secs":{}}}"#,
                    dev.id, tx_count, rx_stats.rx_count, rx_stats.total_rx_bytes,
                    rx_stats.last_rx_sender, hex_display,
                    seq, slot.slot_counter, slot.synced, uptime_secs
                );
                if !mqtt.publish_status(json.as_bytes()).await {
                    dwarn!(dev, "MQTT publish failed");
                    mqtt.core.disconnect();
                    mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
                }
            }

            // Serial stats (keep existing log pattern)
            dlog!(dev, "stats: slot={} tx={} rx={} rx_bytes={} synced={} uptime={}s",
                  slot.slot_counter, tx_count, rx_stats.rx_count, rx_stats.total_rx_bytes, slot.synced, uptime_secs);
            if rx_stats.total_rx_bytes == 0 {
                dwarn!(dev, "NO RX BYTES — transceiver RX may be broken");
            }

            // Track sync timeout
            slot.cycles_since_last_rx += 1;
            if slot.synced && slot.cycles_since_last_rx >= SYNC_TIMEOUT_CYCLES {
                dwarn!(dev, "SYNC LOST: no RX sync for {} cycles, free-running", slot.cycles_since_last_rx);
                slot.synced = false;
            }
            if !slot.synced && slot.cycles_since_last_rx >= SYNC_TIMEOUT_CYCLES {
                dlog!(dev, "FREE-RUN: unsynced for {} cycles, starting own slot", slot.cycles_since_last_rx);
                slot.slot_counter = dev.index;
                slot.slot_deadline = embassy_time::Instant::now() + SLOT_DURATION;
                slot.cycles_since_last_rx = 0;
            }
        }

        // ── Advance to next slot ─────────────────────────────────────
        Timer::at(slot.slot_deadline).await;
        slot.slot_deadline = slot.slot_deadline + SLOT_DURATION;
        slot.slot_counter = slot.slot_counter.wrapping_add(1);
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
