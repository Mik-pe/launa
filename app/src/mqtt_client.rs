//! MQTT v5 client over embassy-net TCP.
//!
//! Hand-rolled MQTT v5 protocol implementation. Handles: connect with
//! username/password, publish (QoS 0/1), subscribe, keepalive PINGREQ,
//! incoming PUBACK, packet reassembly, and reconnect.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::string::ToString;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::{self, Read, Write, ErrorType};
use launa_mqtt::topics::TopicBuilder;
use launa_mqtt::command_parser::{self, ParseResult};
use launa_mqtt::state::status_to_json;
use launa_protocol::command::{Command, validate_set_temperature};
use launa_protocol::status::{TemperatureScale, TempRange, StatusUpdate};
use log::{info, warn, debug, error};

use crate::config::AppConfig;
use crate::mk_static;
use crate::net_util;

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

pub struct MqttClient {
    transport: TcpTransport,
    stack: &'static Stack<'static>,
    pub device_id: String,
    keep_alive: u16,
    config_host: String,
    config_port: u16,
    config_user: String,
    config_password: String,
    next_packet_id: u16,
    last_outgoing: Instant,
    rx_buffer: Vec<u8>,
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
                    Ok(_) => Some(MqttAction::Command(Command::SetTemperature(temp))),
                    Err(e) => {
                        warn!("MQTT temperature {} rejected for {:?}/{:?}: {:?}", temp, s, r, e);
                        None
                    }
                }
            } else {
                Some(MqttAction::Command(Command::SetTemperature(temp)))
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
        let rx_buf = mk_static!([u8; 1024], [0u8; 1024]);
        let tx_buf = mk_static!([u8; 1024], [0u8; 1024]);
        let mut socket = TcpSocket::new(*stack, rx_buf, tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        let addr = net_util::parse_ip(&config.mqtt_host).unwrap_or([192, 168, 1, 100]);
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
            transport,
            stack,
            device_id: config.device_id.clone(),
            keep_alive: DEFAULT_KEEP_ALIVE_SECS,
            config_host: config.mqtt_host.clone(),
            config_port: config.mqtt_port,
            config_user: config.mqtt_user.clone(),
            config_password: config.mqtt_password.clone(),
            next_packet_id: 1,
            last_outgoing: Instant::now(),
            rx_buffer: Vec::new(),
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
        self.transport.write_all(data).await.map_err(|_| MqttError::PublishFailed)?;
        self.last_outgoing = Instant::now();
        Ok(())
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

        let rx_buf = mk_static!([u8; 1024], [0u8; 1024]);
        let tx_buf = mk_static!([u8; 1024], [0u8; 1024]);
        let mut socket = TcpSocket::new(*self.stack, rx_buf, tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        let addr = net_util::parse_ip(&self.config_host).unwrap_or([192, 168, 1, 100]);
        let endpoint = IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::from_octets(addr)),
            port: self.config_port,
        };

        socket.connect(endpoint).await.map_err(|e| {
            error!("MQTT reconnect TCP failed: {:?}", e);
            MqttError::ConnectionFailed
        })?;

        self.transport = TcpTransport::new(socket);
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
            | (1 << 2)
            | (1 << 3)
            | (1 << 4);

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
        let n = self.transport.read(&mut buf).await.map_err(|_| MqttError::ConnectionFailed)?;
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

        let mut buf = [0u8; 32];
        let _ = self.transport.read(&mut buf).await;
        self.last_outgoing = Instant::now();
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<(String, Vec<u8>)> {
        loop {
            if let Err(e) = self.maybe_ping().await {
                error!("MQTT keepalive ping failed: {:?}", e);
                return None;
            }

            if let Some(packet) = self.try_extract_packet() {
                return self.process_packet(&packet).await;
            }

            let mut buf = [0u8; 512];
            let n = match self.transport.read(&mut buf).await {
                Ok(0) => {
                    Timer::after(Duration::from_millis(10)).await;
                    continue;
                }
                Ok(n) => n,
                Err(_) => return None,
            };

            self.rx_buffer.extend_from_slice(&buf[..n]);
        }
    }

    fn try_extract_packet(&mut self) -> Option<Vec<u8>> {
        if self.rx_buffer.len() < 2 { return None; }
        let (remaining_len, header_size) = decode_remaining_length(&self.rx_buffer)?;
        let total_size = header_size + remaining_len;
        if self.rx_buffer.len() >= total_size {
            let packet = Vec::from(&self.rx_buffer[..total_size]);
            self.rx_buffer = Vec::from(&self.rx_buffer[total_size..]);
            Some(packet)
        } else {
            None
        }
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
                let props_len = packet[idx] as usize;
                idx += 1 + props_len;

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
        let json = status_to_json(status, last_fault, None);
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
        let device_id = self.device_id.clone();
        let topics = TopicBuilder::new(&device_id);
        let state_topic = topics.state_topic();
        let avail_topic = topics.availability_topic();
        let cmd_topic = topics.command_topic();

        let device_info = format!(
            r#"{{"identifiers":["{}"],"name":"Launa Spa","manufacturer":"Launa","model":"BP6013G1"}}"#,
            device_id
        );

        let mut configs: Vec<(String, String, String)> = alloc::vec![
            ("sensor".into(), "temperature".into(), format!(
                r#"{{"device":{},"name":"Water Temperature","unique_id":"{}_temperature","device_class":"temperature","unit_of_measurement":"°F","state_topic":"{}","value_template":"{{{{value_json.current_temp}}}}","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
            ("number".into(), "set_temperature".into(), format!(
                r#"{{"device":{},"name":"Set Temperature","unique_id":"{}_set_temp","device_class":"temperature","unit_of_measurement":"°F","min":50,"max":104,"step":1,"state_topic":"{}","command_topic":"{}/set_temperature","value_template":"{{{{value_json.set_temp}}}}","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            ("binary_sensor".into(), "heating".into(), format!(
                r#"{{"device":{},"name":"Heating","unique_id":"{}_heating","device_class":"heat","state_topic":"{}","value_template":"{{{{value_json.is_heating}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
        ];

        // Pumps 1-6
        for i in 1..=6u8 {
            let name = format!("Pump {}", i);
            let unique_id = format!("{}_pump{}", device_id, i);
            let value_template = format!("{{{{value_json.pump{}_on}}}}", i);
            let payload = format!(
                r#"{{"device":{},"name":"{}","unique_id":"{}","state_topic":"{}","command_topic":"{}/pump{}","value_template":"{}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, name, unique_id, state_topic, cmd_topic, i, value_template, avail_topic
            );
            configs.push(("switch".into(), format!("pump{}", i), payload));
        }

        // Lights 1-2
        for i in 1..=2u8 {
            let name = if i == 1 { "Spa Light".to_string() } else { format!("Light {}", i) };
            let unique_id = format!("{}_light{}", device_id, i);
            let value_template = format!("{{{{value_json.light{} }}}}", i);
            let payload = format!(
                r#"{{"device":{},"name":"{}","unique_id":"{}","state_topic":"{}","command_topic":"{}/light{}","value_template":"{}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, name, unique_id, state_topic, cmd_topic, i, value_template, avail_topic
            );
            configs.push(("light".into(), format!("light{}", i), payload));
        }

        configs.extend_from_slice(&[
            ("fan".into(), "blower".into(), format!(
                r#"{{"device":{},"name":"Blower","unique_id":"{}_blower","state_topic":"{}","command_topic":"{}/blower","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            ("select".into(), "heat_mode".into(), format!(
                r#"{{"device":{},"name":"Heat Mode","unique_id":"{}_heat_mode","state_topic":"{}","command_topic":"{}/heat_mode","value_template":"{{{{value_json.heating_mode}}}}","options":["ready","rest","ready_in_rest"],"availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            ("switch".into(), "circ_pump".into(), format!(
                r#"{{"device":{},"name":"Circulation Pump","unique_id":"{}_circ_pump","state_topic":"{}","value_template":"{{{{value_json.circ_pump}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
            ("select".into(), "temp_range".into(), format!(
                r#"{{"device":{},"name":"Temperature Range","unique_id":"{}_temp_range","state_topic":"{}","command_topic":"{}/temp_range","value_template":"{{{{value_json.temp_range}}}}","options":["high","low"],"availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            ("switch".into(), "hold_mode".into(), format!(
                r#"{{"device":{},"name":"Hold Mode","unique_id":"{}_hold_mode","state_topic":"{}","command_topic":"{}/hold_mode","value_template":"{{{{value_json.hold_mode}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, cmd_topic, avail_topic
            )),
            ("switch".into(), "mister".into(), format!(
                r#"{{"device":{},"name":"Mister","unique_id":"{}_mister","state_topic":"{}","value_template":"{{{{value_json.mister}}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
            ("sensor".into(), "fault".into(), format!(
                r#"{{"device":{},"name":"Last Fault","unique_id":"{}_fault","state_topic":"{}","value_template":"{{{{value_json.last_fault}}}}","availability_topic":"{}"}}"#,
                device_info, device_id, state_topic, avail_topic
            )),
            ("sensor".into(), "diagnostics".into(), format!(
                r#"{{"device":{},"name":"Diagnostics","unique_id":"{}_diagnostics","state_topic":"{}","availability_topic":"{}"}}"#,
                device_info, device_id, topics.diagnostics_topic(), avail_topic
            )),
            ("sensor".into(), "alert".into(), format!(
                r#"{{"device":{},"name":"Alert","unique_id":"{}_alert","state_topic":"{}","availability_topic":"{}"}}"#,
                device_info, device_id, topics.alert_topic(), avail_topic
            )),
        ]);

        for (component, object_id, payload) in &configs {
            let topic = format!("homeassistant/{}/{}/{}/config", component, device_id, object_id);
            if let Err(e) = self.publish(&topic, payload.as_bytes(), 1, true).await {
                warn!("Failed to publish discovery for {}: {:?}", object_id, e);
            }
        }

        info!("Published {} HA discovery configs", configs.len());
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

    /// Send MQTT DISCONNECT packet and flush the transport.
    /// Call this before OTA reboot to notify the broker cleanly.
    pub async fn disconnect(&mut self) {
        let _ = self.send_bytes(&[0xE0, 0x00]).await;
        info!("MQTT DISCONNECT sent");
    }

    pub fn parse_ota_url(payload: &[u8]) -> Option<String> {
        let s = core::str::from_utf8(payload).ok()?;
        let mut search_from = 0;
        while let Some(pos) = s[search_from..].find("\"url\"") {
            let abs_pos = search_from + pos;
            // Reject matches inside longer keys like "callback_url" or "image_url"
            if abs_pos > 0 {
                let ch_before = s.as_bytes()[abs_pos - 1];
                if ch_before == b'_' || ch_before.is_ascii_alphanumeric() {
                    search_from = abs_pos + 5;
                    continue;
                }
            }
            let after_key = &s[abs_pos + 5..];
            let after_key = after_key.trim_start();
            let after_key = after_key.strip_prefix(':')?;
            let after_key = after_key.trim_start();
            let after_key = after_key.strip_prefix('"')?;
            if let Some(end) = after_key.find('"') {
                return Some(String::from(&after_key[..end]));
            }
            return None;
        }
        None
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

fn decode_remaining_length(buf: &[u8]) -> Option<(usize, usize)> {
    if buf.is_empty() { return None; }
    let mut multiplier = 1usize;
    let mut value = 0usize;
    let mut idx = 1;
    loop {
        if idx >= buf.len() { return None; }
        let byte = buf[idx];
        value += ((byte & 0x7F) as usize) * multiplier;
        multiplier *= 128;
        idx += 1;
        if (byte & 0x80) == 0 { break; }
        if multiplier > 128 * 128 * 128 * 128 { return None; }
    }
    Some((value, idx))
}

