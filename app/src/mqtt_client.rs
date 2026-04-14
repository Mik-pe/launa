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
use launa_mqtt::command_parser::{self, ParseResult};
use launa_mqtt::state::status_to_json;
use launa_protocol::command::Command;
use launa_protocol::status::StatusUpdate;
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

/// Parse incoming MQTT command using launa-mqtt's command parser.
/// Returns a Command only for valid commands; logs and drops invalid ones.
pub fn parse_command(command_topic_base: &str, topic: &str, payload: &[u8]) -> Option<Command> {
    match command_parser::parse_command(command_topic_base, topic, payload) {
        ParseResult::Valid(cmd) => Some(cmd),
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
        let topics = TopicBuilder::new(&self.device_id);
        let device_id = &self.device_id;
        let state_topic = topics.state_topic();
        let avail_topic = topics.availability_topic();
        let cmd_topic = topics.command_topic();

        // Build common device info JSON fragment
        let device_info = format!(
            r#"{{"identifiers":["{}"],"name":"Launa Spa","manufacturer":"Launa","model":"BP6013G1"}}"#,
            device_id
        );

        let configs: Vec<(&str, &str, String)> = alloc::vec![
            // Temperature sensor
            ("sensor", "temperature", format!(
                r#"{{"device":{},"name":"Water Temperature","unique_id":"{}_temperature","device_class":"temperature","unit_of_measurement":"°F","state_topic":"{}","value_template":"{{{{value_json.current_temp}}}}","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
            // Set temperature number
            ("number", "set_temperature", format!(
                r#"{{"device":{},"name":"Set Temperature","unique_id":"{}_set_temp","device_class":"temperature","unit_of_measurement":"°F","min":50,"max":104,"step":1,"state_topic":"{}","command_topic":"{}/set_temperature","value_template":"{{{{value_json.set_temp}}}}","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Heating binary sensor
            ("binary_sensor", "heating", format!(
                r#"{{"device":{},"name":"Heating","unique_id":"{}_heating","device_class":"heat","state_topic":"{}","value_template":"{{{{value_json.is_heating}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
            // Pump 1 switch
            ("switch", "pump1", format!(
                r#"{{"device":{},"name":"Pump 1","unique_id":"{}_pump1","state_topic":"{}","command_topic":"{}/pump1","value_template":"{{{{value_json.pump1_on}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Pump 2 switch
            ("switch", "pump2", format!(
                r#"{{"device":{},"name":"Pump 2","unique_id":"{}_pump2","state_topic":"{}","command_topic":"{}/pump2","value_template":"{{{{value_json.pump2_on}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Pump 3 switch
            ("switch", "pump3", format!(
                r#"{{"device":{},"name":"Pump 3","unique_id":"{}_pump3","state_topic":"{}","command_topic":"{}/pump3","value_template":"{{{{value_json.pump3_on}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Light
            ("light", "light1", format!(
                r#"{{"device":{},"name":"Spa Light","unique_id":"{}_light1","state_topic":"{}","command_topic":"{}/light1","value_template":"{{{{value_json.light1}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Blower fan
            ("fan", "blower", format!(
                r#"{{"device":{},"name":"Blower","unique_id":"{}_blower","state_topic":"{}","command_topic":"{}/blower","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Heat Mode select
            ("select", "heat_mode", format!(
                r#"{{"device":{},"name":"Heat Mode","unique_id":"{}_heat_mode","state_topic":"{}","command_topic":"{}/heat_mode","value_template":"{{{{value_json.heating_mode}}}}","options":["ready","rest","ready_in_rest"],"availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Circulation Pump switch
            ("switch", "circ_pump", format!(
                r#"{{"device":{},"name":"Circulation Pump","unique_id":"{}_circ_pump","state_topic":"{}","command_topic":"{}/circ_pump","value_template":"{{{{value_json.circ_pump}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Temperature Range select
            ("select", "temp_range", format!(
                r#"{{"device":{},"name":"Temperature Range","unique_id":"{}_temp_range","state_topic":"{}","command_topic":"{}/temp_range","value_template":"{{{{value_json.temp_range}}}}","options":["high","low"],"availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Hold Mode switch
            ("switch", "hold_mode", format!(
                r#"{{"device":{},"name":"Hold Mode","unique_id":"{}_hold_mode","state_topic":"{}","command_topic":"{}/hold_mode","value_template":"{{{{value_json.hold_mode}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Mister switch
            ("switch", "mister", format!(
                r#"{{"device":{},"name":"Mister","unique_id":"{}_mister","state_topic":"{}","command_topic":"{}/mister","value_template":"{{{{value_json.mister}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            // Fault sensor
            ("sensor", "fault", format!(
                r#"{{"device":{},"name":"Last Fault","unique_id":"{}_fault","state_topic":"{}","value_template":"{{{{value_json.last_fault}}}}","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
        ];

        for (component, object_id, payload) in &configs {
            let topic = format!(
                "homeassistant/{}/{}/{}/config",
                component, device_id, object_id
            );
            if let Err(e) = self.publish(topic, payload.as_bytes(), 1, true).await {
                warn!("Failed to publish discovery for {}: {:?}", object_id, e);
            }
        }

        info!("Published {} HA discovery configs", configs.len());
        Ok(())
    }

    pub async fn subscribe_commands(&mut self) -> Result<(), MqttError> {
        let topics = TopicBuilder::new(&self.device_id);

        // Subscribe to command wildcard
        let cmd_topic = format!("{}/#", topics.command_topic());
        self.subscribe(&cmd_topic).await?;
        info!("Subscribed to command topic: {}", cmd_topic);

        // Subscribe to OTA topic
        let ota_topic = topics.ota_topic();
        self.subscribe(&ota_topic).await?;
        info!("Subscribed to OTA topic: {}", ota_topic);

        // Subscribe to HA status for re-publishing discovery on HA restart
        let ha_status_topic = topics.ha_status_topic();
        self.subscribe(&ha_status_topic).await?;
        info!("Subscribed to HA status topic: {}", ha_status_topic);

        Ok(())
    }

    /// Check if a received topic is the OTA topic
    pub fn is_ota_topic(&self, topic: &str) -> bool {
        let topics = TopicBuilder::new(&self.device_id);
        topic == topics.ota_topic()
    }

    /// Check if a received topic is the HA status topic
    pub fn is_ha_status_topic(&self, topic: &str) -> bool {
        topic == "homeassistant/status"
    }

    /// Extract firmware URL from OTA payload. Payload is expected to be JSON: {"url":"http://..."}
    pub fn parse_ota_url(payload: &[u8]) -> Option<String> {
        let s = core::str::from_utf8(payload).ok()?;
        // Simple JSON parsing: find "url" key
        if let Some(start) = s.find(r#""url""#) {
            let after_key = &s[start + 5..];
            // Skip whitespace and colon
            let after_key = after_key.trim_start();
            let after_key = after_key.strip_prefix(':')?;
            let after_key = after_key.trim_start();
            // Find the opening quote
            let after_key = after_key.strip_prefix('"')?;
            // Find the closing quote
            if let Some(end) = after_key.find('"') {
                return Some(String::from(&after_key[..end]));
            }
        }
        None
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
