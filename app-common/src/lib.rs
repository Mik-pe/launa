//! Shared code for ESP32 app crates (app-rs485-debugger, app-sniffer, app-spa-emulator).
//!
//! Provides common WiFi initialization, MQTT transport, and utility code
//! used by all ESP32 app binaries.

#![no_std]

extern crate alloc;

pub mod wifi;

use alloc::vec::Vec;
use core::cell::UnsafeCell;

use embassy_net::tcp::TcpSocket;
use embassy_net::{dns::DnsQueryType, IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::{Read, Write};
use log::warn;

// Re-export key types for convenience
pub use embassy_net;
pub use esp_radio;
pub use launa_mqtt;

/// Create a `&'static mut` reference to a value using `static_cell`.
#[macro_export]
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

// ── MQTT Socket Buffer Size ──────────────────────────────────────────

/// Default MQTT socket buffer size (shared across all app crates).
pub const MQTT_SOCKET_BUF_SIZE: usize = 512;

/// Default MQTT keep-alive interval in seconds.
pub const MQTT_KEEP_ALIVE_SECS: u16 = 60;

// ── TCP Transport ────────────────────────────────────────────────────

/// Wrapper around embassy-net TcpSocket implementing embedded-io-async traits.
pub struct TcpTransport {
    pub socket: TcpSocket<'static>,
}

#[derive(Debug)]
pub struct TransportError;

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

// ── MQTT Buffers ─────────────────────────────────────────────────────

/// Pre-allocated socket buffers (reused across reconnects).
pub struct MqttBuffers {
    pub rx: &'static UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
    pub tx: &'static UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
}

impl MqttBuffers {
    /// Allocate static socket buffers using mk_static!.
    pub fn new() -> Self {
        let rx = crate::mk_static!(
            UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
            UnsafeCell::new([0u8; MQTT_SOCKET_BUF_SIZE])
        );
        let tx = crate::mk_static!(
            UnsafeCell<[u8; MQTT_SOCKET_BUF_SIZE]>,
            UnsafeCell::new([0u8; MQTT_SOCKET_BUF_SIZE])
        );
        MqttBuffers { rx, tx }
    }
}

// ── MQTT State Core ──────────────────────────────────────────────────

/// Core MQTT connection state shared across all app crates.
///
/// Provides the common connect/send/read/ping logic. Each app crate can
/// wrap this with its own specific fields and methods.
pub struct MqttStateCore {
    pub stack: &'static Stack<'static>,
    pub buffers: MqttBuffers,
    pub transport: Option<TcpTransport>,
    pub last_outgoing: Instant,
}

impl MqttStateCore {
    /// Create a new core MQTT state with pre-allocated buffers.
    pub fn new(stack: &'static Stack<'static>) -> Self {
        MqttStateCore {
            stack,
            buffers: MqttBuffers::new(),
            transport: None,
            last_outgoing: Instant::now(),
        }
    }

    /// Returns true if the TCP transport is connected.
    pub fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    /// Disconnect the TCP transport.
    pub fn disconnect(&mut self) {
        self.transport.take();
    }

    /// Create a new TcpSocket from the pre-allocated buffers.
    ///
    /// # Safety
    /// The caller must ensure the old socket (if any) has been dropped
    /// before calling this. This is safe in single-executor cooperative
    /// scheduling contexts.
    pub fn create_socket(&mut self) -> TcpSocket<'static> {
        let rx: &'static mut [u8] = unsafe { &mut *self.buffers.rx.get() };
        let tx: &'static mut [u8] = unsafe { &mut *self.buffers.tx.get() };
        TcpSocket::new(*self.stack, rx, tx)
    }

    /// Connect TCP to the given MQTT host:port. Returns the endpoint on success.
    ///
    /// Resolves the hostname, connects the TCP socket, and stores the transport.
    /// Does NOT send the MQTT CONNECT packet (callers handle that themselves).
    pub async fn connect_tcp(
        &mut self,
        mqtt_host: &str,
        mqtt_port: u16,
    ) -> Result<IpEndpoint, ()> {
        self.transport.take();

        let mut socket = self.create_socket();

        let addr = match resolve_host(self.stack, mqtt_host).await {
            Some(a) => a,
            None => {
                warn!("MQTT: DNS failed for '{}'", mqtt_host);
                return Err(());
            }
        };
        let endpoint = IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::from_octets(addr)),
            port: mqtt_port,
        };

        if let Err(e) = socket.connect(endpoint).await {
            warn!("MQTT: TCP connect to {}:{} failed: {:?}", mqtt_host, endpoint.port, e);
            return Err(());
        }

        self.transport = Some(TcpTransport { socket });
        self.last_outgoing = Instant::now();
        Ok(endpoint)
    }

    /// Perform the MQTT CONNECT handshake.
    ///
    /// Sends CONNECT, reads CONNACK. Takes the transport on failure.
    pub async fn mqtt_connect_handshake(
        &mut self,
        config: &launa_mqtt::mqtt_codec::ConnectConfig<'_>,
        mqtt_host: &str,
        mqtt_port: u16,
    ) -> bool {
        let connect_packet = launa_mqtt::mqtt_codec::encode_connect(config);

        if self.send_bytes(&connect_packet).await.is_err() {
            warn!("MQTT: CONNECT send failed");
            self.transport.take();
            return false;
        }

        let mut buf = [0u8; 64];
        match self.read_exact(&mut buf, 4).await {
            Ok(n) if n >= 4 => {
                if launa_mqtt::mqtt_codec::parse_connack(&buf[..n]).is_err() {
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
        log::info!("MQTT connected to {}:{}", mqtt_host, mqtt_port);
        true
    }

    /// Send a QoS 0 publish to the given topic. Returns true on success.
    pub async fn publish(&mut self, topic: &str, payload: &[u8]) -> bool {
        if let Ok(packet) = launa_mqtt::mqtt_codec::encode_publish(topic, payload, 0, false, None) {
            self.send_bytes(&packet).await.is_ok()
        } else {
            false
        }
    }

    /// Send keepalive PINGREQ if half the keepalive has elapsed.
    pub async fn maybe_ping(&mut self) -> bool {
        let half = Duration::from_secs(MQTT_KEEP_ALIVE_SECS as u64 / 2);
        if self.last_outgoing.elapsed() >= half {
            let ping = launa_mqtt::mqtt_codec::encode_pingreq();
            if self.send_bytes(&ping).await.is_err() {
                return false;
            }
        }
        true
    }

    /// Send raw bytes over the transport.
    pub async fn send_bytes(&mut self, data: &[u8]) -> Result<(), ()> {
        let transport = self.transport.as_mut().ok_or(())?;
        transport.write_all(data).await.map_err(|_| ())?;
        transport.flush().await.map_err(|_| ())?;
        self.last_outgoing = Instant::now();
        Ok(())
    }

    /// Read exactly `min_bytes` from the transport with a 5-second deadline.
    pub async fn read_exact(&mut self, buf: &mut [u8], min_bytes: usize) -> Result<usize, ()> {
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

// ── DNS Resolution ───────────────────────────────────────────────────

/// Resolve hostname to IPv4 address.
///
/// Fast path: tries parsing as dotted quad. Falls back to DNS resolution.
pub async fn resolve_host(stack: &Stack<'static>, host: &str) -> Option<[u8; 4]> {
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


