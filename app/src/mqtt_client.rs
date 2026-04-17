//! MQTT v5 client over embassy-net TCP.
//!
//! Hand-rolled MQTT v5 protocol implementation. Handles: connect with
//! username/password, publish (QoS 0/1), subscribe, keepalive PINGREQ,
//! incoming PUBACK, packet reassembly, and reconnect.

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
use launa_core::{RateLimiter, RATE_LIMIT_MAX_COMMANDS, RATE_LIMIT_WINDOW_MS};
use launa_protocol::command::{Command, validate_set_temperature};
use launa_protocol::status::{TemperatureScale, TempRange, StatusUpdate};
use log::{info, warn, debug, error};

use crate::config::AppConfig;
use crate::mk_static;
use crate::net_util;

// ── MQTT command rate limiting ─────────────────────────────────────────
// RateLimiter is defined in launa-core with Clock trait injection.
// Constants RATE_LIMIT_MAX_COMMANDS (10) and RATE_LIMIT_WINDOW_MS (10_000)
// are re-exported from launa-core.

// ── MQTT action type (command vs timer) ────────────────────────────────

#[derive(Debug)]
pub enum MqttAction {
    Command(Command),
    StartPumpTimer { pump: u8, minutes: u32 },
}

// ── TCP transport wrapper ──────────────────────────────────────────────

pub struct TcpTransport {
    socket: TcpSocket<'static>,
}

#[derive(Debug)]
pub struct TransportError;

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
}

// ── MQTT client ────────────────────────────────────────────────────────

const DEFAULT_KEEP_ALIVE_SECS: u16 = 30;
const RX_BUFFER_MAX_SIZE: usize = 2048; // 2 KiB cap

pub struct MqttClient {
    transport: Option<TcpTransport>,
    stack: &'static Stack<'static>,
    /// TCP socket buffers reused across reconnects to avoid leaking static memory.
    /// Wrapped in `UnsafeCell` so we can safely reborrow the interior via `get_mut()`
    /// after dropping the previous `TcpSocket`, without raw-pointer aliasing UB.
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
                match validate_set_temperature(temp, s, r) {
                    Ok(_) => {
                        // Convert display value to protocol wire value.
                        // In Celsius mode, wire value = display * 2 (e.g. 38°C → 76).
                        // Fahrenheit display values ARE wire values (no conversion).
                        let wire_value = match s {
                            TemperatureScale::Celsius => temp.saturating_mul(2),
                            TemperatureScale::Fahrenheit => temp,
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
        socket.set_timeout(Some(Duration::from_secs(10)));

        let addr = net_util::resolve_host(stack, &config.mqtt_host).await.unwrap_or([192, 168, 1, 100]);
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
            rx_buffer: Vec::new(),
            rate_limiter: RateLimiter::new(),
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

    pub async fn maybe_ping(&mut self) -> Result<(), MqttError> {
        let half_keepalive = Duration::from_secs(self.keep_alive as u64 / 2);
        if self.last_outgoing.elapsed() >= half_keepalive {
            debug!("MQTT sending PINGREQ (keepalive)");
            self.send_bytes(&[0xC0, 0x00]).await?;
        }
        Ok(())
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
        socket.set_timeout(Some(Duration::from_secs(10)));

        let addr = net_util::resolve_host(self.stack, &self.config_host).await.unwrap_or([192, 168, 1, 100]);
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
        let mut connect_flags = 0x02u8
            | (1 << 2)   // Clean Start
            | (1 << 3)   // Will Flag
            | (1 << 4)   // Will QoS 1
            | (1 << 5);  // Will Retain

        if username.is_some() { connect_flags |= 1 << 7; }
        if password.is_some() { connect_flags |= 1 << 6; }

        let keep_alive = self.keep_alive;
        let mut var_header = Vec::new();
        var_header.extend_from_slice(&[0x00, 0x04]);
        var_header.extend_from_slice(b"MQTT");
        var_header.push(0x05);
        var_header.push(connect_flags);
        var_header.extend_from_slice(&keep_alive.to_be_bytes());
        var_header.push(0x00);

        let mut payload = Vec::new();
        append_lp_string(&mut payload, client_id);
        append_lp_string(&mut payload, lwt_topic);
        payload.push(0x00);
        let will_payload = b"offline";
        payload.extend_from_slice(&(will_payload.len() as u16).to_be_bytes());
        payload.extend_from_slice(will_payload);
        if let Some(user) = username { append_lp_string(&mut payload, user); }
        if let Some(pass) = password { append_lp_string(&mut payload, pass); }

        let remaining_len = var_header.len() + payload.len();
        let mut packet = Vec::new();
        packet.push(0x10);
        encode_remaining_length(&mut packet, remaining_len);
        packet.extend_from_slice(&var_header);
        packet.extend_from_slice(&payload);

        self.send_bytes(&packet).await.map_err(|_| MqttError::ConnectionFailed)?;

        let mut buf = [0u8; 64];
        let n = self.read_exact(&mut buf, 4).await.map_err(|_| MqttError::ConnectionFailed)?;
        if n < 4 || buf[0] != 0x20 {
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
        let retain_flag = if retain { 0x01 } else { 0x00 };
        let qos_flag = (qos & 0x03) << 1;
        let mut packet = Vec::new();
        packet.push(0x30 | qos_flag | retain_flag);

        let topic_bytes = topic.as_bytes();
        let mut remaining = 2 + topic_bytes.len() + 1 + payload.len();
        if qos > 0 { remaining += 2; }
        encode_remaining_length(&mut packet, remaining);
        packet.extend_from_slice(&(topic_bytes.len() as u16).to_be_bytes());
        packet.extend_from_slice(topic_bytes);

        if qos > 0 {
            let pkt_id = self.allocate_packet_id();
            packet.extend_from_slice(&pkt_id.to_be_bytes());
        }

        packet.push(0x00);
        packet.extend_from_slice(payload);

        self.send_bytes(&packet).await
    }

    pub async fn subscribe(&mut self, topic: &str) -> Result<(), MqttError> {
        let mut packet = Vec::new();
        packet.push(0x82);

        let topic_bytes = topic.as_bytes();
        let remaining = 2 + 1 + 2 + topic_bytes.len() + 1;
        encode_remaining_length(&mut packet, remaining);

        let pkt_id = self.allocate_packet_id();
        packet.extend_from_slice(&pkt_id.to_be_bytes());
        packet.push(0x00);
        packet.extend_from_slice(&(topic_bytes.len() as u16).to_be_bytes());
        packet.extend_from_slice(topic_bytes);
        packet.push(0x01);

        self.send_bytes(&packet).await.map_err(|_| MqttError::SubscribeFailed)?;

        // Read SUBACK: fixed header byte + variable-byte remaining length + payload
        let mut header_buf = [0u8; 5]; // max 1 + 4 bytes for fixed header
        self.read_exact(&mut header_buf, 1).await.map_err(|_| {
            warn!("MQTT SUBACK read failed");
            MqttError::SubscribeFailed
        })?;

        if header_buf[0] != 0x90 {
            warn!("MQTT SUBACK unexpected type: 0x{:02X}", header_buf[0]);
            return Err(MqttError::SubscribeFailed);
        }

        // Read remaining length (variable-byte encoded, up to 4 bytes)
        let mut rl_buf = [0u8; 4];
        let mut rl_bytes = 1;
        self.read_exact(&mut rl_buf[..1], 1).await.map_err(|_| {
            warn!("MQTT SUBACK remaining length read failed");
            MqttError::SubscribeFailed
        })?;

        while rl_buf[rl_bytes - 1] & 0x80 != 0 && rl_bytes < 4 {
            self.read_exact(&mut rl_buf[rl_bytes..rl_bytes + 1], 1).await.map_err(|_| {
                warn!("MQTT SUBACK remaining length read failed");
                MqttError::SubscribeFailed
            })?;
            rl_bytes += 1;
        }

        let remaining_len = match decode_remaining_length(&rl_buf[..rl_bytes]) {
            Some((len, _)) => len,
            None => {
                warn!("MQTT SUBACK invalid remaining length encoding");
                return Err(MqttError::SubscribeFailed);
            }
        };

        // Read the full remaining payload
        let mut payload_buf = [0u8; 64];
        if remaining_len > payload_buf.len() {
            warn!("MQTT SUBACK payload too large: {} bytes", remaining_len);
            return Err(MqttError::SubscribeFailed);
        }
        self.read_exact(&mut payload_buf[..remaining_len], remaining_len).await.map_err(|_| {
            warn!("MQTT SUBACK payload read failed");
            MqttError::SubscribeFailed
        })?;

        // Parse packet identifier (first 2 bytes of payload)
        if remaining_len < 3 {
            warn!("MQTT SUBACK payload too short");
            return Err(MqttError::SubscribeFailed);
        }
        let ack_pkt_id = u16::from_be_bytes([payload_buf[0], payload_buf[1]]);
        if ack_pkt_id != pkt_id {
            warn!("MQTT SUBACK packet ID mismatch: expected {}, got {}", pkt_id, ack_pkt_id);
            return Err(MqttError::SubscribeFailed);
        }

        // Decode property length (variable-byte)
        let (props_len, props_header_size) = match decode_remaining_length(&payload_buf[2..]) {
            Some(v) => v,
            None => {
                warn!("MQTT SUBACK invalid property length");
                return Err(MqttError::SubscribeFailed);
            }
        };

        let return_code_idx = 2 + props_header_size + props_len;
        if return_code_idx >= remaining_len {
            warn!("MQTT SUBACK missing return code");
            return Err(MqttError::SubscribeFailed);
        }
        let return_code = payload_buf[return_code_idx];
        if return_code == 0x80 {
            warn!("MQTT SUBACK subscription failed (return code 0x80)");
            return Err(MqttError::SubscribeFailed);
        }

        self.last_outgoing = Instant::now();
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<(String, Vec<u8>)> {
        /// Maximum number of consecutive TCP read errors before giving up
        /// and triggering a reconnect. This prevents transient network
        /// glitches (brief packet loss, TCP retransmission timeout) from
        /// causing unnecessary MQTT reconnect cycles.
        const MAX_READ_RETRIES: u8 = 3;

        #[allow(unused_assignments)]
        let mut read_retries: u8 = 0;

        loop {
            if let Err(e) = self.maybe_ping().await {
                error!("MQTT keepalive ping failed: {:?}", e);
                return None;
            }

            if let Some(packet) = self.try_extract_packet() {
                read_retries = 0;
                return self.process_packet(&packet).await;
            }

            let mut buf = [0u8; 512];
            let transport = match self.transport.as_mut() {
                Some(t) => t,
                None => return None,
            };
            let n = match transport.read(&mut buf).await {
                Ok(0) => {
                    Timer::after(Duration::from_millis(10)).await;
                    continue;
                }
                Ok(n) => n,
                Err(_) => {
                    read_retries += 1;
                    if read_retries <= MAX_READ_RETRIES {
                        warn!(
                            "MQTT TCP read error (attempt {}/{})",
                            read_retries, MAX_READ_RETRIES
                        );
                        Timer::after(Duration::from_millis(100)).await;
                        continue;
                    }
                    error!(
                        "MQTT TCP read failed after {} retries, triggering reconnect",
                        MAX_READ_RETRIES
                    );
                    return None;
                }
            };

            // Successful read resets the retry counter
            read_retries = 0;
            self.rx_buffer.extend_from_slice(&buf[..n]);

            if self.rx_buffer.len() > RX_BUFFER_MAX_SIZE {
                warn!(
                    "MQTT rx_buffer exceeded {} bytes ({}), treating as protocol error",
                    RX_BUFFER_MAX_SIZE, self.rx_buffer.len()
                );
                self.rx_buffer.clear();
                return None;
            }
        }
    }

    fn try_extract_packet(&mut self) -> Option<Vec<u8>> {
        try_extract_packet(&mut self.rx_buffer)
    }

    async fn process_packet(&mut self, packet: &[u8]) -> Option<(String, Vec<u8>)> {
        if packet.is_empty() { return None; }
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
                let (props_len, props_header_size) = match decode_remaining_length(&packet[idx..]) {
                    Some(v) => v,
                    None => return None,
                };
                idx += props_header_size + props_len;

                let payload = if idx < packet.len() {
                    Vec::from(&packet[idx..])
                } else {
                    Vec::new()
                };

                if qos >= 1 {
                    if let Some(id) = pkt_id {
                        let puback = [0x40, 0x02, (id >> 8) as u8, (id & 0xFF) as u8];
                        if let Err(e) = self.send_bytes(&puback).await {
                            warn!("Failed to send PUBACK: {:?}", e);
                        }
                    }
                }

                Some((topic, payload))
            }
            4 => { debug!("MQTT PUBACK received"); None }
            9 => { debug!("MQTT SUBACK received"); None }
            13 => { debug!("MQTT PINGRESP"); None }
            12 => {
                let _ = self.send_bytes(&[0xD0, 0x00]).await;
                None
            }
            14 => { warn!("MQTT DISCONNECT received"); None }
            _ => { debug!("MQTT packet type {} (unhandled)", packet_type); None }
        }
    }

    pub async fn publish_state(&mut self, status: &StatusUpdate, last_fault: Option<&str>) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let state_topic = topics.state_topic();
        let json = status_to_json(status, last_fault, Some(crate::FIRMWARE_VERSION));
        self.publish(&state_topic, json.as_bytes(), 1, false).await
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

    pub async fn publish_discovery(&mut self) -> Result<(), MqttError> {
        let builder = DiscoveryBuilder::new(&self.device_id)
            .sw_version(crate::FIRMWARE_VERSION);

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
        let _ = self.send_bytes(&[0xE0, 0x00]).await;
        info!("MQTT DISCONNECT sent");
    }

    pub fn parse_ota_url(payload: &[u8]) -> Option<String> {
        launa_mqtt::parse_ota_url(payload)
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn append_lp_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    buf.extend_from_slice(bytes);
}

fn encode_remaining_length(buf: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut byte = (len & 0x7F) as u8;
        len >>= 7;
        if len > 0 { byte |= 0x80; }
        buf.push(byte);
        if len == 0 { break; }
    }
}


