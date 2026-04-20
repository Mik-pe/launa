use rumqttd::{Broker, Config as MqttConfig};

use crate::Config;

/// Build the MQTT broker config and return the Broker.
/// Caller must call `broker.start()` to begin accepting connections.
pub fn build(config: &Config) -> Result<Broker, Box<dyn std::error::Error>> {
    let mqtt_config = build_config(config)?;
    Ok(Broker::new(mqtt_config))
}

fn build_config(config: &Config) -> Result<MqttConfig, Box<dyn std::error::Error>> {
    let toml_str = format!(
        r#"
id = 0

[router]
id = 0
max_connections = 100
max_outgoing_packet_count = 200
max_segment_size = 10485760
max_segment_count = 10

[v4.1]
name = "tcp"
listen = "0.0.0.0:{}"
next_connection_delay_ms = 1
    [v4.1.connections]
    connection_timeout_ms = 60000
    max_payload_size = 20480
    max_inflight_count = 100
    dynamic_filters = true

[ws.1]
name = "websocket"
listen = "0.0.0.0:{}"
next_connection_delay_ms = 1
    [ws.1.connections]
    connection_timeout_ms = 60000
    max_client_id_len = 256
    throttle_delay_ms = 0
    max_payload_size = 20480
    max_inflight_count = 100
"#,
        config.mqtt_tcp_port, config.mqtt_ws_port
    );

    let config: MqttConfig = config::Config::builder()
        .add_source(config::File::from_str(&toml_str, config::FileFormat::Toml))
        .build()?
        .try_deserialize()?;

    Ok(config)
}
