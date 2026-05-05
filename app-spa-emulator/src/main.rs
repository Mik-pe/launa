//! Launa Spa Emulator — simulates a Balboa BP6013G1 spa controller on ESP32.
//!
//! Uses `launa_sim::SpaSim` to generate realistic RS-485 frames over UART1,
//! identical to what a real spa mainboard would send. Processes incoming
//! commands from a Launa client and responds accordingly.
//!
//! ## Realistic Timing
//!
//! Matches the real BP6013G1 traffic pattern captured with the RS-485 sniffer:
//! - Ready (CTS) frames every ~20ms: `10 BF 06`
//! - Status frames every ~280ms (14 Ready intervals): `FF AF 13 ...`
//! - Registration queries when unregistered: `FE BF 00` (~1s interval, in its own CTS slot)
//!
//! ## Half-Duplex Bus Protocol
//!
//! With MAX13487E auto-direction RS-485 transceivers, the bus is half-duplex:
//! both devices cannot TX simultaneously. The real spa avoids contention by:
//! 1. Sending CTS/Status/Query frames on a fixed schedule
//! 2. Only reading during a brief response window (~1ms) after each TX
//! 3. The client (display panel) ONLY responds during those windows
//!
//! This emulator follows the same pattern: TX first, then open a brief
//! response window, then idle until the next frame interval.
//!
//! Publishes diagnostics over MQTT every ~14 seconds (each status frame cycle).
//!
//! Subscribes to `launa/spa_emulator/config` for live parameter tuning:
//! - `{"post_tx_delay_ms":2}` — post-TX turnaround delay (ms)
//! - `{"suppress_tx":true}` — suppress all TX output (silent mode)
//! - `{"suppress_registration":true}` — suppress NewClientQuery frames
//!
//! GPIO17 = TX, GPIO16 = RX (UART1, 115200 baud, RS-485 half-duplex)

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_net::{
    dns::DnsQueryType, Config as NetConfig, DhcpConfig, IpAddress, IpEndpoint, Ipv4Address,
    Runner, Stack, StackResources,
};
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::{Read, Write};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::uart::Uart;
use launa_mqtt::mqtt_codec::{
    encode_connect, encode_publish, encode_puback, encode_subscribe, parse_connack,
    parse_incoming_publish, parse_suback, ConnectConfig,
};
use launa_mqtt::packet::try_extract_packet;
use launa_protocol::frame::FrameDecoder;
use launa_sim::{SpaSim, SpaState};
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

// Build-time WiFi/MQTT config (set by xtask via env vars)
const WIFI_SSID: &str = env!("LAUNA_WIFI_SSID");
const WIFI_PASSWORD: &str = env!("LAUNA_WIFI_PASSWORD");
const MQTT_HOST: &str = env!("LAUNA_MQTT_HOST");
const MQTT_PORT: &str = env!("LAUNA_MQTT_PORT");

/// Interval between individual RS-485 frames on the bus.
/// Real Balboa BP6013G1 sends Ready frames every ~20ms.
const FRAME_INTERVAL: Duration = Duration::from_millis(20);

/// Number of Ready frames between consecutive Status frames.
/// Real spa: ~14 Readys per Status (~280ms status interval / ~20ms ready interval).
const READYS_PER_STATUS: u32 = 14;

/// How often to publish MQTT diagnostics (in status frames, ~14 seconds).
const MQTT_PUBLISH_INTERVAL: u32 = 14;

/// Number of status frame cycles between registration queries when unregistered.
/// The real BP6013G1 sends FEBF 00 approximately once per second.
/// Each status cycle is ~280ms, so 3 cycles ≈ 840ms, 4 cycles ≈ 1120ms.
/// Using 4 gives ~1.1s interval, closest to the observed ~1s pattern.
const REG_QUERY_STATUS_INTERVALS: u32 = 4;

/// How long to wait after TX for the client's response.
/// The real BP6013G1 display panel responds in ~0.5-0.7ms after a CTS.
/// With MAX13487E auto-direction RS-485 transceivers, add ~0.5ms turnaround.
/// After a FEBF 00 (NewClientQuery), the app firmware responds immediately
/// in the fast-path (no idle-gap queue delay), typically within ~1-2ms.
///
/// 5ms is generous — matches the real spa's ~1-2ms listen window while
/// leaving plenty of time in the 20ms interval for MQTT processing.
const RX_RESPONSE_WINDOW: Duration = Duration::from_millis(5);

const MQTT_SOCKET_BUF_SIZE: usize = 512;
const MQTT_KEEP_ALIVE_SECS: u16 = 60;

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

// ── Runtime config (tunable via MQTT) ────────────────────────────────

struct RuntimeConfig {
    /// Post-TX turnaround delay in ms (after each UART write).
    post_tx_delay_ms: u64,
    /// If true, suppress all UART TX output (silent mode).
    suppress_tx: bool,
    /// If true, suppress NewClientQuery frames (don't solicit registration).
    suppress_registration: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            post_tx_delay_ms: 2,
            suppress_tx: false,
            suppress_registration: false,
        }
    }
}

// ── WiFi ─────────────────────────────────────────────────────────────

#[embassy_executor::task]
async fn connection_task(mut controller: esp_radio::wifi::WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(_info) => {
                info!("WiFi connected");
                loop {
                    if !controller.is_connected() {
                        break;
                    }
                    Timer::after(Duration::from_secs(1)).await;
                }
                warn!("WiFi disconnected");
            }
            Err(e) => {
                warn!("WiFi connect failed: {:?}", e);
            }
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, esp_radio::wifi::Interface<'static>>) {
    runner.run().await;
}

fn wifi_init(
    spawner: Spawner,
    wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
    rng: Rng,
) -> &'static Stack<'static> {
    let station_config = esp_radio::wifi::Config::Station(
        esp_radio::wifi::sta::StationConfig::default()
            .with_ssid(WIFI_SSID)
            .with_password(String::from(WIFI_PASSWORD)),
    );

    info!("Starting WiFi...");
    let (controller, interfaces) = esp_radio::wifi::new(
        wifi_peripheral,
        esp_radio::wifi::ControllerConfig::default().with_initial_config(station_config),
    )
    .expect("WiFi init failed");

    let wifi_interface = interfaces.station;

    let mut dhcp_config = DhcpConfig::default();
    let hostname: heapless::String<32> = "launa-spa-emu".parse().unwrap();
    dhcp_config.hostname = Some(hostname);
    let net_config = NetConfig::dhcpv4(dhcp_config);
    let seed = ((rng.random() as u64) << 32) | (rng.random() as u64);

    spawner.spawn(connection_task(controller).unwrap());
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        net_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );
    let stack_ref = mk_static!(Stack<'static>, stack);
    spawner.spawn(net_task(runner).unwrap());

    stack_ref
}

// ── MQTT Client ──────────────────────────────────────────────────────

struct MqttBuffers {
    rx: &'static UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
    tx: &'static UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
}

struct TcpTransport {
    socket: TcpSocket<'static>,
}

#[derive(Debug)]
struct TransportError;

impl embedded_io_async::Error for TransportError {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        embedded_io_async::ErrorKind::Other
    }
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TransportError")
    }
}

impl core::error::Error for TransportError {}

impl embedded_io_async::ErrorType for TcpTransport {
    type Error = TransportError;
}

impl Read for TcpTransport {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.socket.read(buf).await.map_err(|_| TransportError)
    }
}

impl Write for TcpTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.socket.write(buf).await.map_err(|_| TransportError)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.socket.flush().await.map_err(|_| TransportError)
    }
}

struct MqttState {
    stack: &'static Stack<'static>,
    buffers: MqttBuffers,
    transport: Option<TcpTransport>,
    status_topic: String,
    config_topic: String,
    last_outgoing: Instant,
    /// Incoming MQTT packet reassembly buffer.
    rx_buffer: Vec<u8>,
    /// Monotonic packet ID counter for SUBSCRIBE/PUBACK.
    next_packet_id: u16,
}

impl MqttState {
    fn new(stack: &'static Stack<'static>) -> Self {
        let rx = mk_static!(
            UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
            UnsafeCell::new([0u8; MQTT_SOCKET_BUF_SIZE])
        );
        let tx = mk_static!(
            UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
            UnsafeCell::new([0u8; MQTT_SOCKET_BUF_SIZE])
        );
        MqttState {
            stack,
            buffers: MqttBuffers { rx, tx },
            transport: None,
            status_topic: "launa/spa_emulator/status".to_string(),
            config_topic: "launa/spa_emulator/config".to_string(),
            last_outgoing: Instant::now(),
            rx_buffer: Vec::new(),
            next_packet_id: 1,
        }
    }

    fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    fn alloc_packet_id(&mut self) -> u16 {
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        if self.next_packet_id == 0 {
            self.next_packet_id = 1;
        }
        id
    }

    async fn connect(&mut self) -> bool {
        self.transport.take();
        self.rx_buffer.clear();

        let rx: &'static mut [u8] = unsafe { &mut *self.buffers.rx.get() };
        let tx: &'static mut [u8] = unsafe { &mut *self.buffers.tx.get() };
        let mut socket = TcpSocket::new(*self.stack, rx, tx);

        let addr = match resolve_host(self.stack, MQTT_HOST).await {
            Some(a) => a,
            None => {
                warn!("MQTT: DNS failed for '{}'", MQTT_HOST);
                return false;
            }
        };
        let endpoint = IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::from_octets(addr)),
            port: MQTT_PORT.parse().unwrap_or(1883),
        };

        if let Err(e) = socket.connect(endpoint).await {
            warn!("MQTT: TCP connect failed: {:?}", e);
            return false;
        }

        self.transport = Some(TcpTransport { socket });
        self.last_outgoing = Instant::now();

        let client_id = "launa_spa_emulator";
        let config = ConnectConfig {
            client_id,
            lwt_topic: &self.status_topic,
            username: None,
            password: None,
            keep_alive: MQTT_KEEP_ALIVE_SECS,
        };
        let connect_packet = encode_connect(&config);

        if self.send_bytes(&connect_packet).await.is_err() {
            warn!("MQTT: CONNECT send failed");
            self.transport.take();
            return false;
        }

        let mut buf = [0u8; 64];
        match self.read_exact(&mut buf, 4).await {
            Ok(n) if n >= 4 => {
                if parse_connack(&buf[..n]).is_err() {
                    warn!("MQTT: CONNACK rejected");
                    self.transport.take();
                    return false;
                }
            }
            _ => {
                warn!("MQTT: CONNACK read failed");
                self.transport.take();
                return false;
            }
        }

        // Subscribe to config topic
        let pkt_id = self.alloc_packet_id();
        let sub_packet = encode_subscribe(&self.config_topic, pkt_id);
        if self.send_bytes(&sub_packet).await.is_err() {
            warn!("MQTT: SUBSCRIBE send failed");
            self.transport.take();
            return false;
        }

        // Wait for SUBACK
        let mut suback_buf = [0u8; 32];
        match self.read_exact(&mut suback_buf, 5).await {
            Ok(n) if n >= 3 => {
                if parse_suback(&suback_buf[..n], pkt_id).is_err() {
                    warn!("MQTT: SUBACK rejected");
                    self.transport.take();
                    return false;
                }
            }
            _ => {
                warn!("MQTT: SUBACK read failed");
                self.transport.take();
                return false;
            }
        }

        self.last_outgoing = Instant::now();
        info!("MQTT connected + subscribed to {}", self.config_topic);
        true
    }

    async fn publish(&mut self, payload: &[u8]) -> bool {
        if let Ok(packet) = encode_publish(&self.status_topic, payload, 0, false, None) {
            self.send_bytes(&packet).await.is_ok()
        } else {
            false
        }
    }

    async fn maybe_ping(&mut self) -> bool {
        let half = Duration::from_secs(MQTT_KEEP_ALIVE_SECS as u64 / 2);
        if self.last_outgoing.elapsed() >= half {
            let ping = launa_mqtt::mqtt_codec::encode_pingreq();
            if self.send_bytes(&ping).await.is_err() {
                return false;
            }
        }
        true
    }

    /// Non-blocking read from MQTT socket with timeout. Returns parsed
    /// (topic, payload) for any incoming PUBLISH packets.
    async fn try_recv(&mut self) -> Option<(String, Vec<u8>)> {
        let transport = self.transport.as_mut()?;
        let mut tmp = [0u8; 256];
        // Short timeout so we don't block the main loop
        match select(transport.read(&mut tmp), Timer::after(Duration::from_millis(10))).await {
            Either::First(Ok(0)) => None,
            Either::First(Ok(n)) => {
                self.rx_buffer.extend_from_slice(&tmp[..n]);
                self.process_rx_buffer().await
            }
            Either::First(Err(_)) => None,
            Either::Second(_) => None, // timeout, no data
        }
    }

    /// Process buffered RX data, extracting complete MQTT packets.
    /// Returns the first PUBLISH packet's (topic, payload), discarding
    /// other packet types (PUBACK, SUBACK, PINGREQ, etc.).
    async fn process_rx_buffer(&mut self) -> Option<(String, Vec<u8>)> {
        while let Some(packet) = try_extract_packet(&mut self.rx_buffer) {
            if packet.is_empty() {
                continue;
            }
            let pkt_type = packet[0] >> 4;
            match pkt_type {
                3 => {
                    // PUBLISH
                    match parse_incoming_publish(&packet) {
                        Ok(pub_msg) => {
                            // Send PUBACK for QoS 1
                            if let Some(pid) = pub_msg.packet_id {
                                let puback = encode_puback(pid);
                                let _ = self.send_bytes(&puback).await;
                            }
                            let topic = String::from(pub_msg.topic);
                            let payload = Vec::from(pub_msg.payload);
                            return Some((topic, payload));
                        }
                        Err(e) => {
                            warn!("MQTT: failed to parse PUBLISH: {:?}", e);
                        }
                    }
                }
                4 => { /* PUBACK — ignore */ }
                9 => { /* SUBACK — ignore (already handled in connect) */ }
                13 => { /* PINGREQ — send PINGRESP */
                    let resp = launa_mqtt::mqtt_codec::encode_pingresp();
                    let _ = self.send_bytes(&resp).await;
                }
                _ => {
                    info!("MQTT: unexpected packet type {}", pkt_type);
                }
            }
        }
        None
    }

    async fn send_bytes(&mut self, data: &[u8]) -> Result<(), ()> {
        let transport = self.transport.as_mut().ok_or(())?;
        transport.write_all(data).await.map_err(|_| ())?;
        transport.flush().await.map_err(|_| ())?;
        self.last_outgoing = Instant::now();
        Ok(())
    }

    async fn read_exact(&mut self, buf: &mut [u8], min_bytes: usize) -> Result<usize, ()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut pos = 0;
        while pos < min_bytes {
            if Instant::now() >= deadline {
                return Err(());
            }
            let transport = self.transport.as_mut().ok_or(())?;
            match transport.read(&mut buf[pos..]).await {
                Ok(0) => Timer::after(Duration::from_millis(10)).await,
                Ok(n) => pos += n,
                Err(_) => return Err(()),
            }
        }
        Ok(pos)
    }
}

async fn resolve_host(stack: &Stack<'static>, host: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 {
        let mut octets = [0u8; 4];
        let mut valid = true;
        for (i, p) in parts.iter().enumerate() {
            match p.parse::<u8>() {
                Ok(v) => octets[i] = v,
                Err(_) => valid = false,
            }
        }
        if valid {
            return Some(octets);
        }
    }

    match stack.dns_query(host, DnsQueryType::A).await {
        Ok(addrs) => {
            if let Some(addr) = addrs.first() {
                let IpAddress::Ipv4(v4) = *addr;
                Some(v4.octets())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

// ── Config parsing ───────────────────────────────────────────────────

/// Apply a JSON config payload to the runtime config.
/// Format: `{"key":value,...}` — unknown keys are silently ignored.
fn apply_config(config: &mut RuntimeConfig, json: &[u8]) {
    // Minimal JSON parser for flat key-value pairs.
    // Handles: bool, u64, string values.
    let s = match core::str::from_utf8(json) {
        Ok(s) => s.trim(),
        Err(_) => return,
    };
    // Strip outer braces
    let inner = s.trim_start_matches('{').trim_end_matches('}');

    // Split on commas (naive but sufficient for flat JSON)
    for pair in inner.split(',') {
        let pair = pair.trim();
        if let Some(colon_pos) = pair.find(':') {
            let key = pair[..colon_pos].trim().trim_matches('"');
            let value = pair[colon_pos + 1..].trim();

            match key {
                "post_tx_delay_ms" => {
                    if let Ok(v) = value.parse::<u64>() {
                        config.post_tx_delay_ms = v;
                        info!("Config: post_tx_delay_ms = {}", v);
                    }
                }
                "suppress_tx" => {
                    if let Ok(v) = value.parse::<bool>() {
                        config.suppress_tx = v;
                        info!("Config: suppress_tx = {}", v);
                    }
                }
                "suppress_registration" => {
                    if let Ok(v) = value.parse::<bool>() {
                        config.suppress_registration = v;
                        info!("Config: suppress_registration = {}", v);
                    }
                }
                _ => {
                    info!("Config: unknown key '{}'", key);
                }
            }
        }
    }
}

// ── Spa helpers ──────────────────────────────────────────────────────

fn pump_str(p: launa_protocol::status::PumpState) -> &'static str {
    match p {
        launa_protocol::status::PumpState::Off => "off",
        launa_protocol::status::PumpState::Low => "LOW",
        launa_protocol::status::PumpState::High => "HIGH",
        _ => "?",
    }
}

fn print_state(tick: u64, state: &SpaState) {
    info!(
        "[{:>4}] temp={:.1} set={:.1} heat={} p1={} p2={} p3={} circ={} blow={} l1={} l2={} hold={}",
        tick,
        state.current_temp.to_fahrenheit(),
        state.set_temp.to_fahrenheit(),
        state.is_heating,
        pump_str(state.pumps[0]),
        pump_str(state.pumps[1]),
        pump_str(state.pumps[2]),
        if state.circ_pump { "ON" } else { "off" },
        if state.blower { "ON" } else { "off" },
        if state.lights[0] { "ON" } else { "off" },
        if state.lights[1] { "ON" } else { "off" },
        if state.hold { "YES" } else { "no" },
    );
}

/// Write data to UART, flushing before and after.
/// Includes a configurable post-TX turnaround delay for the auto-direction
/// RS-485 transceiver to switch from TX to RX mode.
async fn write_frames(uart: &mut Uart<'static, esp_hal::Async>, data: &[u8], post_tx_delay_ms: u64) {
    let mut written = 0;
    while written < data.len() {
        match uart.write(&data[written..]) {
            Ok(n) => written += n,
            Err(_) => return,
        }
    }
    let _ = uart.flush();
    Timer::after(Duration::from_millis(post_tx_delay_ms)).await;
}

/// Read any available bytes from the UART RX buffer during a response window.
///
/// This is called only during the brief response window after the emulator
/// has transmitted a CTS or Query frame. The real BP6013G1 only expects
/// client responses in this window — the client should never TX outside it.
///
/// Returns decoded frames and raw byte count for diagnostics.
async fn read_response_window(
    uart: &mut Uart<'static, esp_hal::Async>,
    frame_decoder: &mut FrameDecoder,
    read_buf: &mut [u8],
    window: Duration,
) -> (u64, Vec<launa_protocol::frame::Frame>) {
    let deadline = Instant::now() + window;
    let mut rx_bytes: u64 = 0;
    let mut frames = Vec::new();

    while Instant::now() < deadline {
        let remaining = deadline - Instant::now();
        let timeout = remaining.min(Duration::from_millis(2));

        match select(uart.read_async(read_buf), Timer::after(timeout)).await {
            Either::First(Ok(n)) if n > 0 => {
                rx_bytes += n as u64;
                let new_frames = frame_decoder.feed_slice(&read_buf[..n]);
                frames.extend(new_frames);
            }
            Either::First(Err(_)) => {
                // UART error, drain and continue
                let mut drain = [0u8; 32];
                let _ = uart.read_buffered(&mut drain);
            }
            _ => {} // Timeout or 0 bytes
        }
    }

    (rx_bytes, frames)
}

// ── Frame extraction helpers ─────────────────────────────────────────

/// Extract just the status frame (FFAF) from a tick() output burst.
///
/// The SpaSim::tick() output contains: [reg_query?] [status_frame] [ready_frame]
/// all concatenated. We need to find and return just the status frame.
/// Frame format: 7E <len> <type_hi> <type_lo> <payload...> <crc> 7E
fn extract_status_frame(data: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x7E && i + 2 < data.len() {
            let len = data[i + 1] as usize;
            // Frame is: 7E LEN ... CRC 7E, total = 2 + len + 1 (CRC) = len + 3
            let frame_end = i + len + 3;
            if frame_end <= data.len() && data[frame_end - 1] == 0x7E {
                // Check message type: bytes at offset 2,3 are the type
                if i + 4 < frame_end {
                    let mt_hi = data[i + 2];
                    let mt_lo = data[i + 3];
                    if mt_hi == 0xFF && mt_lo == 0xAF {
                        return Some(data[i..frame_end].to_vec());
                    }
                }
                i = frame_end;
                continue;
            }
        }
        i += 1;
    }
    None
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

    info!("=== Launa Spa Emulator ===");
    info!("UART1 (TX=GPIO17, RX=GPIO16), 115200 baud");

    // UART1 for RS-485
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let mut uart = Uart::new(peripherals.UART1, uart_config)
        .expect("UART1 init failed")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    info!("UART1 ready");

    // Drain stale bytes
    let mut drain = [0u8; 64];
    let _ = uart.read_buffered(&mut drain);

    // WiFi + MQTT
    let rng = Rng::new();
    let net_stack = wifi_init(spawner, peripherals.WIFI, rng);
    info!("Waiting for DHCP...");
    net_stack.wait_config_up().await;
    if let Some(cfg) = net_stack.config_v4() {
        info!("Got IP: {}", cfg.address);
    }

    let mut mqtt = MqttState::new(net_stack);
    if mqtt.connect().await {
        info!("MQTT ready");
    } else {
        warn!("MQTT connect failed, will retry");
    }

    // Runtime config (tunable via MQTT)
    let mut rt_config = RuntimeConfig::default();

    // Spa simulator
    let mut sim = SpaSim::new();
    sim.set_require_registration(true);
    // Don't set registration_window_ticks — in the emulator's real-time model,
    // tick_count only advances on status frame boundaries (~280ms), not per
    // second as in integration tests. The timing window would incorrectly reject
    // valid responses that arrive between tick boundaries.
    let mut frame_decoder = FrameDecoder::new();
    let mut read_buf = [0u8; 128];
    let mut rx_count: u64 = 0;
    let mut rx_bytes: u64 = 0;
    let mut tx_count: u64 = 0;
    let mut decoded_frames: u64 = 0;
    let mut frame_errors: u64 = 0;
    let start_time = Instant::now();

    // Sub-frame counter: 0 = send Status, 1..READYS_PER_STATUS = send Ready (CTS)
    let mut sub_frame: u32 = 0;
    // MQTT publish counter (counts status frames)
    let mut status_frame_count: u32 = 0;
    // Registration query counter: counts status frames, sends FEBF 00 every
    // REG_QUERY_STATUS_INTERVALS when unregistered. The real spa sends FEBF 00
    // approximately once per second (~3-4 status cycles of ~280ms each).
    let mut reg_query_counter: u32 = 0;
    let mut mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);

    info!("Spa simulator started, realistic ~20ms frame timing...");

    loop {
        // ── Phase 1: TX — Send CTS/Status/Query frame ──
        //
        // The real BP6013G1 sends frames on a fixed schedule:
        // - CTS (10BF 06) every ~20ms
        // - Status (FFAF) every ~280ms (14 CTS intervals)
        // - NewClientQuery (FEBF 00) every ~1s when unregistered
        //
        // We TX first, then open a brief response window for the client.
        // This matches the real spa's behavior: it sends, then listens for
        // the display panel's response in the next ~1-2ms.
        let mut tx_data = Vec::new();

        // Determine if this CTS slot should carry a registration query instead.
        // The real BP6013G1 sends FEBF 00 in its own ~20ms slot (never combined
        // with the status frame). It sends the query approximately once per second
        // (~3-4 status cycles × 280ms ≈ 840-1120ms). The query replaces a CTS,
        // not the status frame.
        let should_send_reg_query = !rt_config.suppress_registration
            && !sim.is_registered()
            && sub_frame != 0
            && reg_query_counter >= REG_QUERY_STATUS_INTERVALS;

        if !rt_config.suppress_tx {
            if sub_frame == 0 {
                // Status frame slot — only send FFAF status, never FEBF here.
                // The real spa sends FFAF alone in its own 20ms slot.

                // Advance simulation physics and generate status frame.
                // We call tick() which returns the full burst (reg query + status + ready),
                // but we only use the status frame — we send Ready/Query frames ourselves
                // at the correct ~20ms interval in the CTS slots.
                let full_output = sim.tick();
                // Parse out just the status frame (FFAF type) from the tick output.
                let status_frame = extract_status_frame(&full_output);
                if let Some(status) = status_frame {
                    tx_data.extend_from_slice(&status);
                }

                status_frame_count += 1;
                reg_query_counter += 1;
            } else if should_send_reg_query {
                // Registration query slot — send FEBF 00 instead of CTS.
                // The real spa sends FEBF 00 alone in its own 20ms slot,
                // never combined with any other frame.
                tx_data.extend_from_slice(&sim.generate_registration_query());
                reg_query_counter = 0;
            } else {
                // Ready (CTS) frame slot — just send 10BF 06
                tx_data.extend_from_slice(&sim.generate_ready_frame());
            }
        } else if sub_frame == 0 {
            // Even with TX suppressed, still advance the simulation
            let _ = sim.tick();
            status_frame_count += 1;
            reg_query_counter += 1;
        }

        // Send the frame(s)
        if !tx_data.is_empty() {
            write_frames(&mut uart, &tx_data, rt_config.post_tx_delay_ms).await;
            tx_count += 1;
        }

        // ── Phase 2: RX response window ──
        //
        // After sending a CTS or Query frame, the real BP6013G1 expects
        // the display panel's response within ~0.7ms. We open a brief
        // response window (RX_RESPONSE_WINDOW = 5ms, generous) to read
        // the client's reply. The real spa only listens for ~1-2ms.
        //
        // This is the ONLY time we read UART. With MAX13487E auto-direction
        // RS-485 transceivers, the bus is half-duplex: we must not try to
        // read while the client might be TX-ing, and we must not TX while
        // the client might still be responding.
        //
        // With MAX13487E transceivers, there is NO self-echo — the
        // transceiver's auto-direction prevents TX data from appearing on RO.
        // Capture frame error count before RX window for delta tracking
        let prev_err = frame_decoder.frame_error_count();

        let (window_bytes, frames) = read_response_window(
            &mut uart,
            &mut frame_decoder,
            &mut read_buf,
            RX_RESPONSE_WINDOW,
        )
        .await;

        if window_bytes > 0 {
            rx_count += 1;
            rx_bytes += window_bytes;
        }

        // Track frame errors
        let total_err = frame_decoder.frame_error_count();
        if total_err > prev_err {
            frame_errors += (total_err - prev_err) as u64;
        }

        // Process decoded frames
        for frame in &frames {
            decoded_frames += 1;
            let mt = alloc::format!(
                "{:02X}{:02X}",
                frame.message_type[0], frame.message_type[1]
            );
            let payload_hex =
                launa_protocol::hex::to_hex(&frame.payload[..frame.payload.len().min(16)]);
            info!(
                "DECODED frame type={} payload_len={} payload={}",
                mt,
                frame.payload.len(),
                payload_hex
            );

            if let Some(response) = sim.process_frame(frame) {
                let resp_hex =
                    launa_protocol::hex::to_hex(&response[..response.len().min(32)]);
                info!("TX response ({} bytes): {}", response.len(), resp_hex);
                // Send the spa's reaction frame (e.g., ClientIdAssignment after
                // a NewClientResponse). This is part of the same response window —
                // the real spa sends these reaction frames immediately.
                write_frames(&mut uart, &response, rt_config.post_tx_delay_ms).await;
            }
        }

        // If registration just completed, reset the query counter
        if sim.is_registered() {
            reg_query_counter = 0;
        }

        // ── Phase 3: MQTT + idle until next frame ──
        //
        // Use remaining time in the 20ms interval for MQTT processing.
        // The bus is silent during this phase — neither the spa nor the
        // client should be TX-ing.
        let frame_deadline = Instant::now() + FRAME_INTERVAL;

        // Process MQTT config messages (non-blocking)
        if mqtt.is_connected() {
            match mqtt.try_recv().await {
                Some((topic, payload)) => {
                    if topic == mqtt.config_topic {
                        apply_config(&mut rt_config, &payload);
                    } else {
                        info!("MQTT: unexpected topic: {}", topic);
                    }
                }
                None => {}
            }
        }

        // Log state every status frame
        if sub_frame == 0 {
            print_state(sim.tick_count(), &sim.state);
        }

        // Publish MQTT diagnostics periodically
        let uptime_secs = start_time.elapsed().as_secs();
        if mqtt.is_connected() {
            if !mqtt.maybe_ping().await {
                warn!("MQTT ping failed");
                mqtt.transport.take();
            }
        }

        if !mqtt.is_connected() && Instant::now() >= mqtt_reconnect_at {
            if mqtt.connect().await {
                info!("MQTT reconnected");
            } else {
                warn!("MQTT reconnect failed, retry in 30s");
                mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
            }
        }

        if mqtt.is_connected() && status_frame_count >= MQTT_PUBLISH_INTERVAL {
            status_frame_count = 0;
            let json = format!(
                r#"{{"tick":{},"tx_count":{},"rx_count":{},"rx_bytes":{},"decoded_frames":{},"frame_errors":{},"registered":{},"rejected_unregistered":{},"rejected_reg_timing":{},"temp":{:.1},"set_temp":{:.1},"p1":"{}","p2":"{}","circ":{},"heat":{},"uptime_secs":{},"post_tx_delay_ms":{},"suppress_tx":{},"suppress_reg":{}}}"#,
                sim.tick_count(),
                tx_count,
                rx_count,
                rx_bytes,
                decoded_frames,
                frame_errors,
                sim.is_registered(),
                sim.rejected_unregistered_frames(),
                sim.rejected_registration_responses(),
                sim.state.current_temp.to_fahrenheit(),
                sim.state.set_temp.to_fahrenheit(),
                pump_str(sim.state.pumps[0]),
                pump_str(sim.state.pumps[1]),
                sim.state.circ_pump,
                sim.state.is_heating,
                uptime_secs,
                rt_config.post_tx_delay_ms,
                rt_config.suppress_tx,
                rt_config.suppress_registration,
            );
            if !mqtt.publish(json.as_bytes()).await {
                warn!("MQTT publish failed");
                mqtt.transport.take();
                mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
            }
        }

        // Wait until next frame interval
        let now = Instant::now();
        if now < frame_deadline {
            Timer::after(frame_deadline - now).await;
        }

        // Advance sub-frame counter
        sub_frame += 1;
        if sub_frame > READYS_PER_STATUS {
            sub_frame = 0;
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
