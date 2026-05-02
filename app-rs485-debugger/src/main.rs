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
use esp_hal::gpio::{AnyPin, Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use esp_hal::uart::Uart;
use esp_hal::Async;
use launa_mqtt::mqtt_codec::{encode_connect, encode_publish, parse_connack, ConnectConfig};
use launa_protocol::frame::FrameDecoder;
use log::{info, warn};

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
            _ => Device { id: "?", index: 0 },
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

/// Create a `&'static mut` reference to a value using `static_cell`.
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
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
            let mut written = 0;
            while written < data.len() {
                let n = self.uart.write(&data[written..]).map_err(|_| ())?;
                written += n;
            }
            self.uart.flush().map_err(|_| ())?;
            de.set_low();
        } else {
            let mut written = 0;
            while written < data.len() {
                let n = self.uart.write(&data[written..]).map_err(|_| ())?;
                written += n;
            }
            self.uart.flush().map_err(|_| ())?;
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
fn sync_slot(sender_index: u8, _dev: &Device) -> (u8, embassy_time::Instant) {
    let new_counter = sender_index.wrapping_add(1);
    let new_deadline = embassy_time::Instant::now() + SLOT_DURATION;
    (new_counter, new_deadline)
}

// ── WiFi ─────────────────────────────────────────────────────────────

/// Embassy task managing WiFi connection lifecycle.
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

/// Embassy task running the embassy-net network stack.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, esp_radio::wifi::Interface<'static>>) {
    runner.run().await;
}

/// Connect to WiFi, wait for DHCP, return network stack.
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

    info!("WiFi started, connecting...");

    let wifi_interface = interfaces.station;

    let mut dhcp_config = DhcpConfig::default();
    let hostname: heapless::String<32> = "launa-rs485dbg".parse().unwrap();
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

// ── Minimal MQTT ─────────────────────────────────────────────────────

const MQTT_SOCKET_BUF_SIZE: usize = 512;
const MQTT_KEEP_ALIVE_SECS: u16 = 60;

/// Pre-allocated socket buffers (reused across reconnects).
struct MqttBuffers {
    rx: &'static UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
    tx: &'static UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
}

/// Minimal MQTT state: just enough to connect and publish.
struct MqttState {
    stack: &'static Stack<'static>,
    buffers: MqttBuffers,
    transport: Option<TcpTransport>,
    topic: String, // e.g. "launa/rs485_debugger/A/status"
    last_outgoing: Instant,
}

/// Wrapper around embassy-net TcpSocket implementing embedded-io-async traits.
struct TcpTransport {
    socket: TcpSocket<'static>,
}

#[derive(Debug)]
struct TransportError;

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TransportError")
    }
}

impl core::error::Error for TransportError {}

impl embedded_io_async::Error for TransportError {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        embedded_io_async::ErrorKind::Other
    }
}

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

impl MqttState {
    fn new(stack: &'static Stack<'static>, device_id: &str) -> Self {
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
            topic: format!("launa/rs485_debugger/{}/status", device_id),
            last_outgoing: Instant::now(),
        }
    }

    fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    /// Connect TCP + MQTT CONNECT handshake. Returns true on success.
    async fn connect(&mut self) -> bool {
        self.transport.take();

        // SAFETY: old TcpSocket was dropped above. We are the only task
        // accessing these buffers (single executor, cooperative scheduling).
        let rx: &'static mut [u8] = unsafe { &mut *self.buffers.rx.get() };
        let tx: &'static mut [u8] = unsafe { &mut *self.buffers.tx.get() };
        let mut socket = TcpSocket::new(*self.stack, rx, tx);

        // Resolve MQTT host
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
            warn!("MQTT: TCP connect to {}:{} failed: {:?}", MQTT_HOST, endpoint.port, e);
            return false;
        }

        self.transport = Some(TcpTransport { socket });
        self.last_outgoing = Instant::now();

        // MQTT CONNECT with LWT on the status topic
        let client_id = format!("launa_rs485dbg_{}", self.topic.split('/').nth(2).unwrap_or("?"));
        let config = ConnectConfig {
            client_id: &client_id,
            lwt_topic: &self.topic,
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

        // Read CONNACK (at least 4 bytes)
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

        self.last_outgoing = Instant::now();
        info!("MQTT connected to {}:{}", MQTT_HOST, endpoint.port);
        true
    }

    /// Publish a QoS 0 payload to the status topic. Non-blocking on failure.
    async fn publish_status(&mut self, payload: &[u8]) -> bool {
        if let Ok(packet) = encode_publish(&self.topic, payload, 0, false, None) {
            self.send_bytes(&packet).await.is_ok()
        } else {
            false
        }
    }

    /// Send keepalive PINGREQ if half the keepalive has elapsed.
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

/// Resolve hostname to IPv4 address.
async fn resolve_host(stack: &Stack<'static>, host: &str) -> Option<[u8; 4]> {
    // Fast path: try parsing as dotted quad
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

    // DNS resolution
    match stack.dns_query(host, DnsQueryType::A).await {
        Ok(addrs) => {
            if let Some(addr) = addrs.first() {
                let IpAddress::Ipv4(v4) = *addr;
                Some(v4.octets())
            } else {
                warn!("DNS: no A record for '{}'", host);
                None
            }
        }
        Err(e) => {
            warn!("DNS: failed to resolve '{}': {:?}", host, e);
            None
        }
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
    let net_stack = wifi_init(spawner, peripherals.WIFI, rng);
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
    let mut rx_count: u64 = 0;
    let mut tx_count: u64 = 0;
    let mut total_rx_bytes: u64 = 0;
    let mut last_rx_sender: &'static str = "-";
    let mut last_rx_hex: String = String::new();
    let start_time = Instant::now();

    // Start on our own slot so we transmit immediately if no one else is heard
    let mut slot_counter: u8 = dev.index;
    let mut slot_deadline = embassy_time::Instant::now() + SLOT_DURATION;
    let mut synced = false;
    let mut cycles_since_last_rx: u32 = 0;
    let mut mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);

    loop {
        let slot_start = slot_deadline - SLOT_DURATION;
        let is_my_slot = slot_counter % NUM_DEVICES == dev.index;

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
            let listen_deadline = slot_deadline - Duration::from_millis(5);
            while embassy_time::Instant::now() < listen_deadline {
                let remaining = listen_deadline - embassy_time::Instant::now();
                match select(transport.read(&mut read_buf), Timer::after(remaining)).await {
                    Either::First(Ok(n)) if n > 0 => {
                        if !read_buf[..n].iter().all(|&b| b == 0x00 || b == 0xFF) {
                            total_rx_bytes += n as u64;
                            rx_count += 1;
                            let hex = launa_protocol::hex::to_hex(&read_buf[..n.min(32)]);
                            let (sender, sender_idx) = decode_sender(&mut decoder, &read_buf[..n]);
                            last_rx_sender = sender;
                            last_rx_hex = hex.clone();
                            dlog!(dev, "RX (post-TX) from [{}]: {} bytes: {}", sender, n, hex);
                            if let Some(idx) = sender_idx {
                                if idx != dev.index {
                                    let (new_ctr, new_dl) = sync_slot(idx, &dev);
                                    slot_counter = new_ctr;
                                    slot_deadline = new_dl;
                                    if !synced {
                                        synced = true;
                                        dlog!(dev, "SYNC: locked to peer [{}]", sender);
                                    }
                                    cycles_since_last_rx = 0;
                                    dlog!(dev, "SYNC: sender=[{}] idx={}, new_slot={}", sender, idx, slot_counter);
                                }
                            }
                        }
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

            let listen_deadline = slot_deadline - Duration::from_millis(5);
            while embassy_time::Instant::now() < listen_deadline {
                let remaining = listen_deadline - embassy_time::Instant::now();
                match select(transport.read(&mut read_buf), Timer::after(remaining)).await {
                    Either::First(Ok(n)) if n > 0 => {
                        if !read_buf[..n].iter().all(|&b| b == 0x00 || b == 0xFF) {
                            total_rx_bytes += n as u64;
                            rx_count += 1;
                            let hex = launa_protocol::hex::to_hex(&read_buf[..n.min(32)]);
                            let prev_errors = decoder.frame_error_count();
                            let (sender, sender_idx) = decode_sender(&mut decoder, &read_buf[..n]);
                            let new_errors = decoder.frame_error_count();
                            last_rx_sender = sender;
                            last_rx_hex = hex.clone();

                            if new_errors > prev_errors {
                                dlog!(dev, "RX from [{}]: {} bytes: {} (crc err)", sender, n, hex);
                            } else {
                                dlog!(dev, "RX from [{}]: {} bytes: {}", sender, n, hex);
                            }

                            if let Some(idx) = sender_idx {
                                if idx != dev.index {
                                    let (new_ctr, new_dl) = sync_slot(idx, &dev);
                                    slot_counter = new_ctr;
                                    slot_deadline = new_dl;
                                    if !synced {
                                        synced = true;
                                        dlog!(dev, "SYNC: locked to peer [{}]", sender);
                                    }
                                    cycles_since_last_rx = 0;
                                    dlog!(dev, "SYNC: sender=[{}] idx={}, new_slot={}", sender, idx, slot_counter);
                                }
                            }
                        }
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

        // ── MQTT publish every 3 slots (1 full cycle) ────────────────
        if slot_counter > 0 && slot_counter % 3 == 0 {
            let uptime_secs = start_time.elapsed().as_secs();

            // MQTT keepalive ping
            if mqtt.is_connected() {
                if !mqtt.maybe_ping().await {
                    dwarn!(dev, "MQTT ping failed, disconnecting");
                    mqtt.transport.take();
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
                let hex_display = if last_rx_hex.len() > 64 {
                    &last_rx_hex[..64]
                } else {
                    &last_rx_hex
                };
                let json = format!(
                    r#"{{"device_id":"{}","tx_count":{},"rx_count":{},"rx_bytes":{},"last_rx_sender":"{}","last_rx_hex":"{}","seq":{},"slot_counter":{},"synced":{},"uptime_secs":{}}}"#,
                    dev.id, tx_count, rx_count, total_rx_bytes,
                    last_rx_sender, hex_display,
                    seq, slot_counter, synced, uptime_secs
                );
                if !mqtt.publish_status(json.as_bytes()).await {
                    dwarn!(dev, "MQTT publish failed");
                    mqtt.transport.take();
                    mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
                }
            }

            // Serial stats (keep existing log pattern)
            dlog!(dev, "stats: slot={} tx={} rx={} rx_bytes={} synced={} uptime={}s",
                  slot_counter, tx_count, rx_count, total_rx_bytes, synced, uptime_secs);
            if total_rx_bytes == 0 {
                dwarn!(dev, "NO RX BYTES — transceiver RX may be broken");
            }

            // Track sync timeout
            cycles_since_last_rx += 1;
            if synced && cycles_since_last_rx >= SYNC_TIMEOUT_CYCLES {
                dwarn!(dev, "SYNC LOST: no RX sync for {} cycles, free-running", cycles_since_last_rx);
                synced = false;
            }
            if !synced && cycles_since_last_rx >= SYNC_TIMEOUT_CYCLES {
                dlog!(dev, "FREE-RUN: unsynced for {} cycles, starting own slot", cycles_since_last_rx);
                slot_counter = dev.index;
                slot_deadline = embassy_time::Instant::now() + SLOT_DURATION;
                cycles_since_last_rx = 0;
            }
        }

        // ── Advance to next slot ─────────────────────────────────────
        Timer::at(slot_deadline).await;
        slot_deadline = slot_deadline + SLOT_DURATION;
        slot_counter = slot_counter.wrapping_add(1);
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
