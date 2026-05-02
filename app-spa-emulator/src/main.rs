//! Launa Spa Emulator — simulates a Balboa BP6013G1 spa controller on ESP32.
//!
//! Uses `launa_sim::SpaSim` to generate realistic RS-485 frames over UART1,
//! identical to what a real spa mainboard would send. Processes incoming
//! commands from a Launa client and responds accordingly.
//!
//! Publishes diagnostics over MQTT every tick so we can observe the RS-485
//! communication from the spa side.
//!
//! GPIO17 = TX, GPIO16 = RX (UART1, 115200 baud, RS-485 half-duplex)

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
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
use launa_mqtt::mqtt_codec::{encode_connect, encode_publish, parse_connack, ConnectConfig};
use launa_sim::{SpaSim, SpaState};
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

// Build-time WiFi/MQTT config (set by xtask via env vars)
const WIFI_SSID: &str = env!("LAUNA_WIFI_SSID");
const WIFI_PASSWORD: &str = env!("LAUNA_WIFI_PASSWORD");
const MQTT_HOST: &str = env!("LAUNA_MQTT_HOST");
const MQTT_PORT: &str = env!("LAUNA_MQTT_PORT");

const TICK_INTERVAL: Duration = Duration::from_secs(1);

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

// ── Minimal MQTT ─────────────────────────────────────────────────────

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
    topic: String,
    last_outgoing: Instant,
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
            topic: "launa/spa_emulator/status".to_string(),
            last_outgoing: Instant::now(),
        }
    }

    fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    async fn connect(&mut self) -> bool {
        self.transport.take();

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
        info!("MQTT connected");
        true
    }

    async fn publish(&mut self, payload: &[u8]) -> bool {
        if let Ok(packet) = encode_publish(&self.topic, payload, 0, false, None) {
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
    let parts: alloc::vec::Vec<&str> = host.split('.').collect();
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
async fn write_frames(uart: &mut Uart<'static, esp_hal::Async>, data: &[u8]) {
    let mut written = 0;
    while written < data.len() {
        match uart.write(&data[written..]) {
            Ok(n) => written += n,
            Err(_) => return,
        }
    }
    let _ = uart.flush();
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

    let mut mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);

    // Spa simulator
    let mut sim = SpaSim::new();
    let mut read_buf = [0u8; 128];
    let mut rx_count: u64 = 0;
    let mut rx_bytes: u64 = 0;
    let mut tx_count: u64 = 0;
    let start_time = Instant::now();

    info!("Spa simulator started, sending frames at 1Hz...");

    loop {
        // Generate and send spa frames for this tick
        let output = sim.tick();
        if !output.is_empty() {
            write_frames(&mut uart, &output).await;
            tx_count += 1;
        }

        print_state(sim.tick_count(), &sim.state);

        // Process any incoming commands during the remainder of the tick
        let deadline = embassy_time::Instant::now() + TICK_INTERVAL - Duration::from_millis(50);
        while embassy_time::Instant::now() < deadline {
            let remaining = deadline - embassy_time::Instant::now();
            match select(uart.read_async(&mut read_buf), Timer::after(remaining)).await {
                Either::First(Ok(n)) if n > 0 => {
                    rx_count += 1;
                    rx_bytes += n as u64;
                    let hex = launa_protocol::hex::to_hex(&read_buf[..n.min(32)]);
                    info!("RX ({} bytes): {}", n, hex);

                    let responses = sim.process_incoming_bytes(&read_buf[..n]);
                    if !responses.is_empty() {
                        let resp_hex = launa_protocol::hex::to_hex(&responses[..responses.len().min(32)]);
                        info!("TX response ({} bytes): {}", responses.len(), resp_hex);
                        write_frames(&mut uart, &responses).await;
                    }
                }
                Either::First(Err(_)) => {
                    let mut drain = [0u8; 32];
                    let _ = uart.read_buffered(&mut drain);
                }
                _ => {}
            }
        }

        // Publish MQTT diagnostics every tick
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

        if mqtt.is_connected() {
            let json = format!(
                r#"{{"tick":{},"tx_count":{},"rx_count":{},"rx_bytes":{},"temp":{:.1},"set_temp":{:.1},"p1":"{}","p2":"{}","circ":{},"heat":{},"uptime_secs":{}}}"#,
                sim.tick_count(),
                tx_count,
                rx_count,
                rx_bytes,
                sim.state.current_temp.to_fahrenheit(),
                sim.state.set_temp.to_fahrenheit(),
                pump_str(sim.state.pumps[0]),
                pump_str(sim.state.pumps[1]),
                sim.state.circ_pump,
                sim.state.is_heating,
                uptime_secs,
            );
            if !mqtt.publish(json.as_bytes()).await {
                warn!("MQTT publish failed");
                mqtt.transport.take();
                mqtt_reconnect_at = Instant::now() + Duration::from_secs(30);
            }
        }

        Timer::at(deadline + Duration::from_millis(50)).await;
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
