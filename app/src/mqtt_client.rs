//! MQTT v5 client over embassy-net TCP.
//!
//! Hand-rolled MQTT v5 protocol implementation. Handles: connect with
//! username/password, publish (QoS 0/1), subscribe, keepalive PINGREQ,
//! incoming PUBACK, packet reassembly, and reconnect.
//!
//! Protocol encoding/decoding is delegated to `launa_mqtt::v5_codec`.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::cell::UnsafeCell;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::{self, Read, Write, ErrorType};
use launa_mqtt::topics::TopicBuilder;
use launa_mqtt::command_parser::{self, ParseResult};
use launa_mqtt::discovery::DiscoveryBuilder;
use launa_mqtt::state::status_to_json;
use launa_mqtt::packet::{decode_remaining_length, try_extract_packet};
use embassy_futures::select::{select, Either};
use launa_mqtt::v5_codec::{
    encode_connect, encode_disconnect, encode_pingreq, encode_pingresp, encode_puback,
    encode_publish, encode_subscribe, parse_connack, parse_suback, ConnectConfig,
};
use launa_core::{RateLimiter, RATE_LIMIT_MAX_COMMANDS, RATE_LIMIT_WINDOW_MS};

use launa_protocol::command::{Command, validate_set_temperature};
use launa_protocol::status::{TemperatureScale, TempRange, StatusUpdate};
use log::{info, warn, debug, error};

use crate::config::AppConfig;
use crate::mk_static;
use crate::net_util;

#[derive(Debug)]
pub enum MqttAction {
    Command(Command),
    StartPumpTimer { pump: u8, minutes: u32 },
    SelfTest(bool),
}

pub struct TcpTransport {
    socket: TcpSocket<'static>,
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

impl TcpTransport {
    pub fn new(socket: TcpSocket<'static>) -> Self {
        TcpTransport { socket }
    }
}

impl ErrorType for TcpTransport {
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

const DEFAULT_KEEP_ALIVE_SECS: u16 = 30;
const RX_BUFFER_MAX_SIZE: usize = 2048; // 2 KiB cap

pub struct MqttClient {
    transport: Option<TcpTransport>,
    stack: &'static Stack<'static>,
    socket_rx_buf: &'static UnsafeCell<[u8; 1024]>,
    socket_tx_buf: &'static UnsafeCell<[u8; 1024]>,
    pub device_id: String,
    keep_alive: u16,
    config_host: String,
    config_port: u16,
    config_user: String,
    config_password: String,
    next_packet_id: u16,
    last_outgoing: Instant,
    rx_buffer: Vec<u8>,
    rate_limiter: RateLimiter,
    /// Last disconnect reason, set by recv() before returning None.
    pub last_disconnect: Option<String>,
}

#[derive(Debug)]
pub enum MqttError {
    ConnectionFailed,
    PublishFailed,
    SubscribeFailed,
    ReadFailed,
}

/// Parse incoming MQTT command using launa-mqtt's command parser.
pub fn parse_command(command_topic_base: &str, topic: &str, payload: &[u8], scale: Option<TemperatureScale>, range: Option<TempRange>) -> Option<MqttAction> {
    match command_parser::parse_command(command_topic_base, topic, payload) {
        ParseResult::Valid(Command::SetTemperature(temp)) => {
            if let (Some(s), Some(r)) = (scale, range) {
                // Validate the display value first
                match validate_set_temperature(temp, s, r) {
                    Ok(_) => {
                        // Convert display value to wire value.
                        // In Celsius mode, re-parse the raw float to preserve
                        // 0.5°C precision (e.g. 38.5°C → wire 77).
                        // Fahrenheit display values ARE wire values.
                        let wire_value = match s {
                            TemperatureScale::Celsius => {
                                // Re-parse raw float to preserve 0.5°C precision.
                                // Display value * 2 = wire value (e.g. 38.5 → 77).
                                let raw_float: f32 = core::str::from_utf8(payload)
                                    .ok()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(temp as f32);
                                (raw_float * 2.0 + 0.5) as u8
                            }
                            _ => temp,
                        };
                        Some(MqttAction::Command(Command::SetTemperature(wire_value)))
                    }
                    Err(e) => {
                        warn!("MQTT temperature {} rejected for {:?}/{:?}: {:?}", temp, s, r, e);
                        None
                    }
                }
            } else {
                warn!("MQTT temperature {} rejected: scale/range not yet known (no status received)", temp);
                None
            }
        }
        ParseResult::Valid(cmd) => Some(MqttAction::Command(cmd)),
        ParseResult::TimerPump { minutes, pump_index } => {
            info!("MQTT pump timer: pump {} for {} min", pump_index, minutes);
            Some(MqttAction::StartPumpTimer { pump: pump_index, minutes })
        }
        ParseResult::TemperatureOutOfRange { raw_value, .. } => {
            warn!("MQTT command rejected: temperature {} out of range", raw_value);
            None
        }
        ParseResult::UnknownSubtopic(sub) => {
            warn!("MQTT command rejected: unknown subtopic '{}'", sub);
            None
        }
        ParseResult::InvalidPayload(msg) => {
            warn!("MQTT command rejected: invalid payload: {}", msg);
            None
        }
    }
}

impl MqttClient {
    pub async fn connect(
        stack: &'static Stack<'static>,
        config: &AppConfig,
    ) -> Result<Self, MqttError> {
        // Allocate socket buffers once — wrapped in UnsafeCell so we can safely
        // reborrow across reconnects without raw-pointer aliasing UB.
        let socket_rx_buf = mk_static!(UnsafeCell<[u8; 1024]>, UnsafeCell::new([0u8; 1024]));
        let socket_tx_buf = mk_static!(UnsafeCell<[u8; 1024]>, UnsafeCell::new([0u8; 1024]));

        // SAFETY: This is the first and only borrow of these newly-allocated
        // buffers. No other task or code path has access to them yet. The
        // TcpSocket takes exclusive ownership of the &mut slices. When the
        // socket is later dropped in reconnect(), the single-task ownership
        // invariant allows us to reborrow from the UnsafeCell fields.
        // This is sound because MqttClient is owned by a single embassy task
        // (mqtt_task) — no concurrent access is possible.
        let rx: &'static mut [u8] = unsafe { &mut *socket_rx_buf.get() };
        let tx: &'static mut [u8] = unsafe { &mut *socket_tx_buf.get() };
        let mut socket = TcpSocket::new(*stack, rx, tx);
        // Socket timeout for detecting truly dead connections (broker unreachable,
        // network partition). Keep-alive pings are handled by racing transport.read()
        // against a timer in recv(), so this timeout only fires in catastrophic cases.
        socket.set_timeout(Some(Duration::from_secs(60)));

        let addr = match net_util::resolve_host(stack, &config.mqtt_host).await {
            Some(a) => a,
            None => {
                error!("MQTT: failed to resolve host '{}'", config.mqtt_host);
                return Err(MqttError::ConnectionFailed);
            }
        };
        let endpoint = IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::from_octets(addr)),
            port: config.mqtt_port,
        };

        socket.connect(endpoint).await.map_err(|e| {
            error!("MQTT TCP connect failed: {:?}", e);
            MqttError::ConnectionFailed
        })?;

        let transport = TcpTransport::new(socket);

        let mut client = MqttClient {
            transport: Some(transport),
            stack,
            socket_rx_buf,
            socket_tx_buf,
            device_id: config.device_id.clone(),
            keep_alive: DEFAULT_KEEP_ALIVE_SECS,
            config_host: config.mqtt_host.clone(),
            config_port: config.mqtt_port,
            config_user: config.mqtt_user.clone(),
            config_password: config.mqtt_password.clone(),
            next_packet_id: 1,
            last_outgoing: Instant::now(),
            rx_buffer: Vec::with_capacity(RX_BUFFER_MAX_SIZE),
            rate_limiter: RateLimiter::new(),
            last_disconnect: None,
        };

        let client_id = format!("launa_{}", config.device_id);
        let topics = TopicBuilder::new(&config.device_id);
        let avail_topic = topics.availability_topic();
        let config_user = client.config_user.clone();
        let config_password = client.config_password.clone();
        let username = if config_user.is_empty() { None } else { Some(config_user.as_str()) };
        let password = if config_password.is_empty() { None } else { Some(config_password.as_str()) };

        client.send_connect(&client_id, &avail_topic, username, password).await?;

        info!("MQTT connected to {}:{}", config.mqtt_host, config.mqtt_port);
        Ok(client)
    }

    fn allocate_packet_id(&mut self) -> u16 {
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        if self.next_packet_id == 0 {
            self.next_packet_id = 1;
        }
        id
    }

    async fn send_bytes(&mut self, data: &[u8]) -> Result<(), MqttError> {
        let transport = self.transport.as_mut().ok_or(MqttError::PublishFailed)?;
        transport.write_all(data).await.map_err(|_| MqttError::PublishFailed)?;
        self.last_outgoing = Instant::now();
        Ok(())
    }

    /// Read from the transport until at least `min_bytes` bytes are in `buf[..pos]`,
    /// or until a 5-second timeout expires between individual read attempts.
    /// Returns the total number of bytes read into the buffer.
    async fn read_exact(&mut self, buf: &mut [u8], min_bytes: usize) -> Result<usize, MqttError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut pos = 0;
        while pos < min_bytes {
            if Instant::now() >= deadline {
                warn!("MQTT read_exact timed out: got {} bytes, need {}", pos, min_bytes);
                return Err(MqttError::ReadFailed);
            }
            let transport = self.transport.as_mut().ok_or(MqttError::ReadFailed)?;
            match transport.read(&mut buf[pos..]).await {
                Ok(0) => {
                    // Zero-byte read: remote closed or no data; yield and retry
                    Timer::after(Duration::from_millis(10)).await;
                }
                Ok(n) => {
                    pos += n;
                }
                Err(_) => {
                    return Err(MqttError::ReadFailed);
                }
            }
        }
        Ok(pos)
    }

    /// Send a PINGREQ if half the keepalive interval has elapsed since the
    /// last outgoing packet. Returns `true` if a ping was sent, `false` if
    /// not needed. A successful ping proves the connection is alive.
    pub async fn maybe_ping(&mut self) -> Result<bool, MqttError> {
        let half_keepalive = Duration::from_secs(self.keep_alive as u64 / 2);
        if self.last_outgoing.elapsed() >= half_keepalive {
            debug!("MQTT sending PINGREQ (keepalive)");
            self.send_bytes(&encode_pingreq()).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn reconnect(&mut self) -> Result<(), MqttError> {
        info!("MQTT reconnecting to {}:{}...", self.config_host, self.config_port);

        // Drop old transport first — this drops the old TcpSocket and releases
        // its borrow on the shared socket buffers.
        self.transport.take();

        // SAFETY: The old TcpSocket was dropped above via self.transport.take(),
        // releasing its borrow on the shared socket buffers. We are the only task
        // accessing these buffers: MqttClient is owned by a single embassy task
        // (mqtt_task), so no concurrent access is possible. The UnsafeCell allows
        // us to obtain a fresh mutable reference without raw-pointer aliasing UB.
        let rx: &'static mut [u8] = unsafe { &mut *self.socket_rx_buf.get() };
        let tx: &'static mut [u8] = unsafe { &mut *self.socket_tx_buf.get() };
        let mut socket = TcpSocket::new(*self.stack, rx, tx);
        socket.set_timeout(Some(Duration::from_secs(60)));

        let addr = match net_util::resolve_host(self.stack, &self.config_host).await {
            Some(a) => a,
            None => {
                error!("MQTT: failed to resolve host '{}' during reconnect", self.config_host);
                return Err(MqttError::ConnectionFailed);
            }
        };
        let endpoint = IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::from_octets(addr)),
            port: self.config_port,
        };

        socket.connect(endpoint).await.map_err(|e| {
            error!("MQTT reconnect TCP failed: {:?}", e);
            MqttError::ConnectionFailed
        })?;

        self.transport = Some(TcpTransport::new(socket));
        self.rx_buffer.clear();
        self.next_packet_id = 1;
        self.last_outgoing = Instant::now();

        let client_id = format!("launa_{}", self.device_id);
        let topics = TopicBuilder::new(&self.device_id);
        let avail_topic = topics.availability_topic();
        let config_user = self.config_user.clone();
        let config_password = self.config_password.clone();
        let username = if config_user.is_empty() { None } else { Some(config_user.as_str()) };
        let password = if config_password.is_empty() { None } else { Some(config_password.as_str()) };

        self.send_connect(&client_id, &avail_topic, username, password).await?;

        info!("MQTT reconnected to {}:{}", self.config_host, self.config_port);
        Ok(())
    }

    async fn send_connect(
        &mut self,
        client_id: &str,
        lwt_topic: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<(), MqttError> {
        let config = ConnectConfig {
            client_id,
            lwt_topic,
            username,
            password,
            keep_alive: self.keep_alive,
        };
        let packet = encode_connect(&config);

        self.send_bytes(&packet).await.map_err(|_| MqttError::ConnectionFailed)?;

        let mut buf = [0u8; 64];
        let n = self.read_exact(&mut buf, 4).await.map_err(|_| MqttError::ConnectionFailed)?;
        if parse_connack(&buf[..n]).is_err() {
            error!("MQTT CONNACK unexpected: {:?}", &buf[..n]);
            return Err(MqttError::ConnectionFailed);
        }
        self.last_outgoing = Instant::now();
        Ok(())
    }

    pub async fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        qos: u8,
        retain: bool,
    ) -> Result<(), MqttError> {
        let packet_id = if qos > 0 {
            Some(self.allocate_packet_id())
        } else {
            None
        };
        let packet = encode_publish(topic, payload, qos, retain, packet_id);
        self.send_bytes(&packet).await
    }

    pub async fn subscribe(&mut self, topic: &str) -> Result<(), MqttError> {
        let pkt_id = self.allocate_packet_id();
        let packet = encode_subscribe(topic, pkt_id);
        self.send_bytes(&packet).await.map_err(|_| MqttError::SubscribeFailed)?;

        // Read packets until we get our SUBACK. After a reconnect, the broker
        // may send PUBACKs for QoS 1 publishes (up to 28 discovery + 1
        // availability = 29), or other packets before our SUBACK arrives.
        const MAX_SKIP: u8 = 50;
        let mut skipped = 0;

        loop {
            // Read fixed header byte
            let mut header_buf = [0u8; 1];
            self.read_exact(&mut header_buf, 1).await.map_err(|_| {
                warn!("MQTT SUBACK read failed");
                MqttError::SubscribeFailed
            })?;

            // Read remaining length (variable-byte encoded, up to 4 bytes)
            let mut rl_buf = [0u8; 4];
            let mut rl_bytes: usize = 0;
            loop {
                self.read_exact(&mut rl_buf[rl_bytes..rl_bytes + 1], 1).await.map_err(|_| {
                    warn!("MQTT packet remaining length read failed");
                    MqttError::SubscribeFailed
                })?;
                rl_bytes += 1;
                if rl_buf[rl_bytes - 1] & 0x80 == 0 || rl_bytes >= 4 {
                    break;
                }
            }

            let mut multiplier = 1usize;
            let mut remaining_len = 0usize;
            for i in 0..rl_bytes {
                remaining_len += ((rl_buf[i] & 0x7F) as usize) * multiplier;
                multiplier *= 128;
            }

            // Read the full remaining payload
            let mut payload_buf = [0u8; 128];
            if remaining_len > payload_buf.len() {
                warn!("MQTT packet payload too large: {} bytes", remaining_len);
                return Err(MqttError::SubscribeFailed);
            }
            if remaining_len > 0 {
                self.read_exact(&mut payload_buf[..remaining_len], remaining_len).await.map_err(|_| {
                    warn!("MQTT packet payload read failed");
                    MqttError::SubscribeFailed
                })?;
            }

            let packet_type = header_buf[0] >> 4;

            if header_buf[0] == 0x90 {
                // SUBACK — reassemble and validate
                let mut suback_buf = Vec::new();
                suback_buf.push(0x90);
                suback_buf.extend_from_slice(&rl_buf[..rl_bytes]);
                suback_buf.extend_from_slice(&payload_buf[..remaining_len]);

                if parse_suback(&suback_buf, pkt_id).is_err() {
                    warn!("MQTT SUBACK parse failed");
                    return Err(MqttError::SubscribeFailed);
                }

                self.last_outgoing = Instant::now();
                return Ok(());
            }

            // Not a SUBACK — skip it
            skipped += 1;
            if skipped > MAX_SKIP {
                warn!("MQTT: too many non-SUBACK packets ({})", skipped);
                return Err(MqttError::SubscribeFailed);
            }
            debug!(
                "MQTT subscribe: skipping packet type {} ({} bytes), waiting for SUBACK",
                packet_type, remaining_len
            );

            // Handle PINGREQ from broker (packet type 12) by sending PINGRESP
            if packet_type == 12 {
                let _ = self.send_bytes(&encode_pingresp()).await;
            }
        }
    }

    pub async fn recv(&mut self) -> Option<(String, Vec<u8>)> {
        #[allow(unused_assignments)]
        let mut read_retries: u16 = 0;

        loop {
            match self.maybe_ping().await {
                Ok(true) => {
                    read_retries = 0;
                }
                Ok(false) => {}
                Err(e) => {
                    self.last_disconnect = Some(alloc::format!("PING FAIL {:?}", e));
                    return None;
                }
            }

            if let Some(packet) = self.try_extract_packet() {
                read_retries = 0;
                match self.process_packets(&packet).await {
                    Some(Some(result)) => return Some(result),
                    Some(None) => continue,
                    None => {
                        self.last_disconnect = Some(alloc::format!("FATAL PKT type {}", packet[0] >> 4));
                        return None;
                    }
                }
            }

            // Race the socket read against a keep-alive timer. This ensures
            // we return to maybe_ping() at least every keep_alive/2 seconds
            // even when no data arrives, without relying on socket timeouts
            // (which can leave the socket in a bad state for writes).
            let mut buf = [0u8; 512];
            let transport = match self.transport.as_mut() {
                Some(t) => t,
                None => {
                    self.last_disconnect = Some(String::from("NO TRANSPORT"));
                    return None;
                }
            };
            let read_fut = transport.read(&mut buf);
            let ping_deadline = Duration::from_secs(self.keep_alive as u64 / 2);
            match select(read_fut, Timer::after(ping_deadline)).await {
                Either::First(read_result) => {
                    match read_result {
                        Ok(0) => {
                            self.last_disconnect = Some(alloc::format!("FIN retries={}", read_retries));
                            return None;
                        }
                        Ok(n) => {
                            read_retries = 0;
                            if self.rx_buffer.len() + n > RX_BUFFER_MAX_SIZE {
                                self.last_disconnect = Some(String::from("BUF OVERFLOW"));
                                self.rx_buffer.clear();
                                return None;
                            }
                            self.rx_buffer.extend_from_slice(&buf[..n]);
                        }
                        Err(_) => {
                            read_retries += 1;
                            if read_retries > 100 {
                                self.last_disconnect = Some(alloc::format!("STUCK retries={}", read_retries));
                                return None;
                            }
                            Timer::after(Duration::from_secs(1)).await;
                        }
                    }
                }
                Either::Second(_) => {
                    // Timer expired before data arrived — loop back to maybe_ping()
                    continue;
                }
            }
        }
    }

    fn try_extract_packet(&mut self) -> Option<Vec<u8>> {
        try_extract_packet(&mut self.rx_buffer)
    }

    /// Process a single extracted MQTT packet.
    ///
    /// Returns:
    /// - `Some(Some((topic, payload)))` — PUBLISH packet with topic and payload
    /// - `Some(None)` — packet handled internally (PUBACK, SUBACK, PINGRESP, etc.)
    /// - `None` — fatal: connection should be terminated (malformed PUBLISH, or
    ///   broker-initiated DISCONNECT)
    async fn process_packets(&mut self, packet: &[u8]) -> Option<Option<(String, Vec<u8>)>> {
        if packet.is_empty() { return Some(None); }
        let packet_type = packet[0] >> 4;

        match packet_type {
            3 => {
                let first_byte = packet[0];
                let qos = (first_byte >> 1) & 0x03;
                let (_remaining, header_size) = decode_remaining_length(packet)?;
                let mut idx = header_size;

                if idx + 2 > packet.len() { return None; }
                let topic_len = u16::from_be_bytes([packet[idx], packet[idx + 1]]) as usize;
                idx += 2;
                if idx + topic_len > packet.len() { return None; }
                let topic = String::from(core::str::from_utf8(&packet[idx..idx + topic_len]).unwrap_or(""));
                idx += topic_len;

                let mut pkt_id: Option<u16> = None;
                if qos > 0 {
                    if idx + 2 > packet.len() { return None; }
                    pkt_id = Some(u16::from_be_bytes([packet[idx], packet[idx + 1]]));
                    idx += 2;
                }

                if idx >= packet.len() { return None; }
                // MQTT 3.1.1: no properties field — payload starts immediately after topic (+ pkt_id if QoS>0)
                let payload = Vec::from(&packet[idx..]);

                if qos >= 1 {
                    if let Some(id) = pkt_id {
                        let puback = encode_puback(id);
                        if let Err(e) = self.send_bytes(&puback).await {
                            warn!("Failed to send PUBACK: {:?}", e);
                        }
                    }
                }

                Some(Some((topic, payload)))
            }
            4 => { debug!("MQTT PUBACK received"); Some(None) }
            9 => { debug!("MQTT SUBACK received"); Some(None) }
            13 => { debug!("MQTT PINGRESP"); Some(None) }
            12 => {
                let _ = self.send_bytes(&encode_pingresp()).await;
                Some(None)
            }
            14 => {
                // Broker-initiated DISCONNECT. Returning None (outer) causes
                // recv() to signal connection loss, triggering the reconnect
                // loop in mqtt_task.rs.
                warn!("MQTT DISCONNECT received from broker");
                None
            }
            _ => { debug!("MQTT packet type {} (unhandled)", packet_type); Some(None) }
        }
    }

    pub async fn publish_state(&mut self, status: &StatusUpdate, last_fault: Option<&str>, self_test: bool, sniff_mode: bool, retain: bool) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let state_topic = topics.state_topic();
        let json = status_to_json(status, last_fault, Some(crate::FIRMWARE_VERSION), self_test, sniff_mode);
        self.publish(&state_topic, json.as_bytes(), 1, retain).await
    }

    pub async fn publish_availability(&mut self, online: bool) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let avail_topic = topics.availability_topic();
        let payload = if online { "online" } else { "offline" };
        self.publish(&avail_topic, payload.as_bytes(), 1, true).await
    }

    pub async fn publish_availability_stale(&mut self) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let avail_topic = topics.availability_topic();
        self.publish(&avail_topic, b"stale", 1, true).await
    }

    /// Publish an alert message to the alert topic.
    /// `level` should be "warn" or "error". `message` describes the alert condition.
    /// `uptime_secs` is the device uptime in seconds.
    pub async fn publish_alert(
        &mut self,
        level: &str,
        message: &str,
        uptime_secs: u64,
    ) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let alert_topic = topics.alert_topic();
        let json = format!(
            r#"{{"level":"{}","message":"{}","timestamp":{}}}"#,
            level, message, uptime_secs
        );
        self.publish(&alert_topic, json.as_bytes(), 1, false).await
    }

    pub async fn publish_discovery(&mut self, celsius: bool) -> Result<(), MqttError> {
        let builder = DiscoveryBuilder::new(&self.device_id)
            .sw_version(crate::FIRMWARE_VERSION)
            .celsius(celsius);

        // Build and publish one entity at a time to avoid OOM on 32 KiB heap.
        // Each call to build() creates all 20 configs in a Vec, but we iterate
        // and publish immediately, dropping each payload after publish.
        let configs = builder.build_with_retain();
        let count = configs.len();

        for msg in &configs {
            if let Err(e) = self.publish(&msg.topic, msg.payload.as_bytes(), 1, msg.retain).await {
                warn!("Failed to publish discovery for {}: {:?}", msg.topic, e);
                // Continue publishing remaining entities — single failure
                // should not block others.
            }
        }

        info!("Published {} HA discovery configs", count);
        Ok(())
    }

    pub async fn subscribe_commands(&mut self) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);

        let cmd_topic = format!("{}/#", topics.command_topic());
        self.subscribe(&cmd_topic).await?;
        info!("Subscribed to command topic: {}", cmd_topic);

        let ota_topic = topics.ota_topic();
        self.subscribe(&ota_topic).await?;
        info!("Subscribed to OTA topic: {}", ota_topic);

        let ha_status_topic = topics.ha_status_topic();
        self.subscribe(&ha_status_topic).await?;
        info!("Subscribed to HA status topic: {}", ha_status_topic);

        Ok(())
    }

    pub fn is_ota_topic(&self, topic: &str) -> bool {
        let topics = TopicBuilder::new(&self.device_id);
        topic == topics.ota_topic()
    }

    pub fn is_ha_status_topic(&self, topic: &str) -> bool {
        topic == "homeassistant/status"
    }

    /// Check if a command is allowed under the rate limit.
    /// Returns `true` if the command should be forwarded.
    /// Returns `false` if the command exceeds the rate limit and should be dropped.
    /// Logs a warning when dropping.
    pub fn check_rate_limit(&mut self) -> bool {
        let now_ms = Instant::now().as_millis() as u64;
        if self.rate_limiter.check(now_ms) {
            true
        } else {
            warn!(
                "MQTT command rate limited: exceeded {} commands per {}s window",
                RATE_LIMIT_MAX_COMMANDS, RATE_LIMIT_WINDOW_MS / 1000
            );
            false
        }
    }

    /// Send MQTT DISCONNECT packet and flush the transport.
    /// Call this before OTA reboot to notify the broker cleanly.
    pub async fn disconnect(&mut self) {
        let _ = self.send_bytes(&encode_disconnect()).await;
        info!("MQTT DISCONNECT sent");
    }

    pub fn parse_ota_url(payload: &[u8]) -> Option<String> {
        launa_mqtt::parse_ota_url(payload)
    }
}
