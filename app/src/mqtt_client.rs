//! MQTT v5 client over embassy-net TCP using rust-mqtt.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_time::{Duration, Timer};
use embedded_io_async::{self, Read, Write, ErrorType};
use launa_mqtt::topics::TopicBuilder;
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::{
    StatusUpdate, HeatingMode, TemperatureScale, TempRange, PumpState,
};
use log::{info, warn, debug, error};

use crate::config::AppConfig;

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
    async fn write(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.socket.write_all(buf).await.map_err(|_| TransportError)
    }
}

// ── MQTT client ────────────────────────────────────────────────────────

pub struct MqttClient {
    transport: TcpTransport,
    device_id: String,
}

#[derive(Debug)]
pub enum MqttError {
    ConnectionFailed,
    PublishFailed,
    SubscribeFailed,
    ReadFailed,
}

/// Build a status JSON manually (serde_json is behind std feature in launa-mqtt).
fn status_to_json(status: &StatusUpdate) -> String {
    let current_temp = match status.current_temp {
        Some(t) => format!("{}", t),
        None => String::from("null"),
    };
    let is_heating = if status.is_heating { "true" } else { "false" };
    let pump1_on = matches!(status.pump1, PumpState::Low | PumpState::High);
    let pump2_on = matches!(status.pump2, PumpState::Low | PumpState::High);
    let pump3_on = matches!(status.pump3, PumpState::Low | PumpState::High);

    let heating_mode = match status.heating_mode {
        HeatingMode::Ready => "ready",
        HeatingMode::Rest => "rest",
        HeatingMode::ReadyInRest => "ready_in_rest",
    };
    let temp_range = match status.temp_range {
        TempRange::High => "high",
        TempRange::Low => "low",
    };
    let temp_scale = match status.temperature_scale {
        TemperatureScale::Fahrenheit => "fahrenheit",
        TemperatureScale::Celsius => "celsius",
    };

    format!(
        "{{\"current_temp\":{},\"set_temp\":{},\"is_heating\":{},\"pump1_on\":{},\"pump2_on\":{},\"pump3_on\":{},\"light1\":{},\"blower\":{},\"circ_pump\":{},\"mister\":{},\"hold_mode\":{},\"heating_mode\":\"{}\",\"temp_range\":\"{}\",\"temp_scale\":\"{}\",\"hour\":{},\"minute\":{},\"last_fault\":null}}",
        current_temp,
        status.set_temp,
        is_heating,
        pump1_on,
        pump2_on,
        pump3_on,
        status.light1,
        status.blower,
        status.circ_pump,
        status.mister,
        status.is_hold,
        heating_mode,
        temp_range,
        temp_scale,
        status.hour,
        status.minute
    )
}

/// Parse incoming MQTT command (reimplemented since launa-mqtt command_parser is std-only).
pub fn parse_command(command_topic_base: &str, topic: &str, payload: &[u8]) -> Option<Command> {
    if !topic.starts_with(command_topic_base) {
        return None;
    }
    let suffix = &topic[command_topic_base.len()..];
    if !suffix.starts_with('/') {
        return None;
    }
    let subtopic = &suffix[1..];
    let payload_str = core::str::from_utf8(payload).ok()?;

    match subtopic {
        "pump1" => parse_toggle(payload_str, ToggleItem::Pump1),
        "pump2" => parse_toggle(payload_str, ToggleItem::Pump2),
        "pump3" => parse_toggle(payload_str, ToggleItem::Pump3),
        "light1" => parse_toggle(payload_str, ToggleItem::Light1),
        "blower" => parse_toggle(payload_str, ToggleItem::Blower),
        "heat_mode" => parse_toggle(payload_str, ToggleItem::HeatingMode),
        "temp_range" => parse_toggle(payload_str, ToggleItem::TemperatureRange),
        "hold_mode" => parse_toggle(payload_str, ToggleItem::HoldMode),
        "set_temperature" => {
            let temp: u8 = payload_str.parse().ok()?;
            Some(Command::SetTemperature(temp))
        }
        _ => None,
    }
}

fn parse_toggle(payload: &str, item: ToggleItem) -> Option<Command> {
    match payload {
        "true" | "false" => Some(Command::ToggleItem(item)),
        _ => None,
    }
}

impl MqttClient {
    pub async fn connect(
        stack: &'static Stack<'static>,
        config: &AppConfig,
    ) -> Result<Self, MqttError> {
        // 1. Open TCP connection
        let mut socket = TcpSocket::new(stack);
        socket.set_timeout(Some(Duration::from_secs(10)));

        let addr = parse_ip(&config.mqtt_host).unwrap_or([192, 168, 1, 100]);
        let endpoint = embassy_net::IpEndpoint {
            addr: embassy_net::IpAddress::from(addr),
            port: config.mqtt_port,
        };

        socket.connect(endpoint).await.map_err(|e| {
            error!("MQTT TCP connect failed: {:?}", e);
            MqttError::ConnectionFailed
        })?;

        let transport = TcpTransport::new(socket);

        // 2. Send MQTT CONNECT packet (v5)
        let mut client = MqttClient {
            transport,
            device_id: config.device_id.clone(),
        };

        let client_id = format!("launa_{}", config.device_id);
        let topics = TopicBuilder::new(&config.device_id);
        let avail_topic = topics.availability_topic();

        // MQTT v5 CONNECT: fixed header + variable header + payload
        // We build the raw bytes manually for minimal dependencies
        client.send_connect(&client_id, &avail_topic).await?;

        info!("MQTT connected to {}:{}", config.mqtt_host, config.mqtt_port);
        Ok(client)
    }

    async fn send_connect(
        &mut self,
        client_id: &str,
        lwt_topic: &str,
    ) -> Result<(), MqttError> {
        // MQTT v5 CONNECT packet
        let connect_flags = 0x02 // Clean start
            | (1 << 2) // Will flag
            | (1 << 3) // Will retain
            | (1 << 4); // Will QoS 1
        let keep_alive: u16 = 30;

        // Variable header: protocol name + level + connect flags + keep alive + properties
        let mut var_header = Vec::new();
        // Protocol name: "MQTT"
        var_header.extend_from_slice(&[0x00, 0x04]);
        var_header.extend_from_slice(b"MQTT");
        var_header.push(0x05); // Protocol level 5
        var_header.push(connect_flags);
        var_header.extend_from_slice(&keep_alive.to_be_bytes());
        // Properties length = 0
        var_header.push(0x00);

        // Payload: client ID + LWT topic + LWT properties + LWT payload
        let mut payload = Vec::new();
        // Client ID
        let id_bytes = client_id.as_bytes();
        payload.extend_from_slice(&(id_bytes.len() as u16).to_be_bytes());
        payload.extend_from_slice(id_bytes);
        // Will topic
        let topic_bytes = lwt_topic.as_bytes();
        payload.extend_from_slice(&(topic_bytes.len() as u16).to_be_bytes());
        payload.extend_from_slice(topic_bytes);
        // Will properties length = 0
        payload.push(0x00);
        // Will payload: "offline"
        let will_payload = b"offline";
        payload.extend_from_slice(&(will_payload.len() as u16).to_be_bytes());
        payload.extend_from_slice(will_payload);

        let remaining_len = var_header.len() + payload.len();
        let mut packet = Vec::new();
        packet.push(0x10); // CONNECT packet type
        encode_remaining_length(&mut packet, remaining_len);
        packet.extend_from_slice(&var_header);
        packet.extend_from_slice(&payload);

        self.transport.write_all(&packet).await.map_err(|_| MqttError::ConnectionFailed)?;

        // Read CONNACK
        let mut buf = [0u8; 64];
        let n = self.transport.read(&mut buf).await.map_err(|_| MqttError::ConnectionFailed)?;
        if n < 4 || buf[0] != 0x20 {
            error!("MQTT CONNACK unexpected: {:?}", &buf[..n]);
            return Err(MqttError::ConnectionFailed);
        }
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
        packet.push(0x30 | qos_flag | retain_flag); // PUBLISH

        let topic_bytes = topic.as_bytes();
        let mut remaining = 2 + topic_bytes.len() + payload.len();
        if qos > 0 {
            remaining += 2; // packet identifier
        }
        encode_remaining_length(&mut packet, remaining);

        // Topic
        packet.extend_from_slice(&(topic_bytes.len() as u16).to_be_bytes());
        packet.extend_from_slice(topic_bytes);

        // Properties length = 0 (MQTT v5)
        packet.push(0x00);

        // Payload
        packet.extend_from_slice(payload);

        self.transport.write_all(&packet).await.map_err(|_| MqttError::PublishFailed)?;
        Ok(())
    }

    pub async fn subscribe(&mut self, topic: &str) -> Result<(), MqttError> {
        let mut packet = Vec::new();
        packet.push(0x82); // SUBSCRIBE

        let topic_bytes = topic.as_bytes();
        let remaining = 2 + 1 + 2 + topic_bytes.len() + 1; // pkt id + prop len + topic filter + sub options
        encode_remaining_length(&mut packet, remaining);

        // Packet identifier
        packet.extend_from_slice(&1u16.to_be_bytes());
        // Properties length = 0
        packet.push(0x00);
        // Topic filter
        packet.extend_from_slice(&(topic_bytes.len() as u16).to_be_bytes());
        packet.extend_from_slice(topic_bytes);
        // Subscription options: QoS 1, no no-local, retain as published, retain handling 0
        packet.push(0x01);

        self.transport.write_all(&packet).await.map_err(|_| MqttError::SubscribeFailed)?;

        // Read SUBACK (skip)
        let mut buf = [0u8; 32];
        let _ = self.transport.read(&mut buf).await;

        Ok(())
    }

    /// Read next incoming PUBLISH message. Returns (topic, payload).
    /// Handles PINGREQ/PINGRESP internally.
    pub async fn recv(&mut self) -> Option<(String, Vec<u8>)> {
        let mut buf = [0u8; 512];
        loop {
            let n = match self.transport.read(&mut buf).await {
                Ok(0) => continue,
                Ok(n) => n,
                Err(_) => return None,
            };

            if n == 0 {
                continue;
            }

            let packet_type = buf[0] >> 4;

            match packet_type {
                // PUBLISH
                3 => {
                    let (_header, remaining_start) = decode_remaining_length(&buf);
                    if remaining_start >= n {
                        continue;
                    }
                    let topic_len = u16::from_be_bytes([buf[remaining_start], buf[remaining_start + 1]]) as usize;
                    let topic_start = remaining_start + 2;
                    let topic_end = topic_start + topic_len;
                    if topic_end > n {
                        continue;
                    }
                    let topic = String::from(core::str::from_utf8(&buf[topic_start..topic_end]).unwrap_or(""));
                    // Skip properties length byte
                    let payload_start = topic_end + 1; // skip MQTT v5 properties length
                    let payload = if payload_start < n {
                        Vec::from(&buf[payload_start..n])
                    } else {
                        Vec::new()
                    };
                    return Some((topic, payload));
                }
                // PINGRESP
                13 => {
                    debug!("MQTT PINGRESP");
                    continue;
                }
                // PINGREQ -> respond with PINGRESP
                12 => {
                    let pingresp = [0xD0, 0x00];
                    let _ = self.transport.write_all(&pingresp).await;
                    continue;
                }
                _ => {
                    debug!("MQTT packet type {}", packet_type);
                    continue;
                }
            }
        }
    }

    pub async fn publish_state(&mut self, status: &StatusUpdate) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let state_topic = topics.state_topic();
        let json = status_to_json(status);
        self.publish(&state_topic, json.as_bytes(), 1, false).await
    }

    pub async fn publish_availability(&mut self, online: bool) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let avail_topic = topics.availability_topic();
        let payload = if online { "online" } else { "offline" };
        self.publish(&avail_topic, payload.as_bytes(), 1, true).await
    }

    pub async fn publish_discovery(&mut self) -> Result<(), MqttError> {
        // TODO: Generate HA auto-discovery configs and publish them.
        // The full DiscoveryBuilder is behind the "std" feature in launa-mqtt.
        // For now we skip discovery -- HA entities can be configured manually,
        // or we can port discovery generation to no_std later.
        info!("HA discovery publish skipped (not yet ported to no_std)");
        Ok(())
    }

    pub async fn subscribe_commands(&mut self) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);
        let cmd_topic = format!("{}/#", topics.command_topic());
        self.subscribe(&cmd_topic).await?;
        info!("Subscribed to command topic: {}", cmd_topic);
        Ok(())
    }
}

// ── MQTT encoding helpers ──────────────────────────────────────────────

fn encode_remaining_length(buf: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut byte = (len & 0x7F) as u8;
        len >>= 7;
        if len > 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if len == 0 {
            break;
        }
    }
}

fn decode_remaining_length(buf: &[u8]) -> (usize, usize) {
    let mut multiplier = 1;
    let mut value = 0;
    let mut idx = 1;
    loop {
        let byte = buf[idx];
        value += ((byte & 0x7F) as usize) * multiplier;
        multiplier *= 128;
        idx += 1;
        if (byte & 0x80) == 0 {
            break;
        }
    }
    (value, idx)
}

/// Parse a dotted IPv4 address string into [u8; 4].
fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<u8> = s.split('.')
        .filter_map(|p| p.parse::<u8>().ok())
        .collect();
    if parts.len() == 4 {
        Some([parts[0], parts[1], parts[2], parts[3]])
    } else {
        None
    }
}
