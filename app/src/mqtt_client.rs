//! MQTT client wrapper for Home Assistant integration.
//!
//! Connects to MQTT broker, publishes HA auto-discovery, subscribes to command topics,
//! and handles LWT (last will) for availability.

use anyhow::{Context, Result};
use embedded_svc::mqtt::client::{ConnectionType, EventPayload, MqttClientSettings, PubData, QoS};
use esp_idf_svc::mqtt::client::{EspAsyncMqttClient, EspMqttClient};
use launa_mqtt::command_parser::parse_command;
use launa_mqtt::discovery::DiscoveryBuilder;
use launa_mqtt::state::status_to_json;
use launa_mqtt::topics::TopicBuilder;
use launa_protocol::command::Command;
use launa_protocol::status::StatusUpdate;
use log::{debug, info, warn};
use std::sync::mpsc::Sender;

pub struct MqttContext {
    pub device_id: String,
    pub command_tx: Sender<Command>,
}

pub fn create_mqtt_client(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    device_id: &str,
    command_tx: Sender<Command>,
) -> Result<EspMqttClient<'static>> {
    let client_id = format!("launa_{}", device_id);
    let availability_topic = TopicBuilder::new(device_id).availability_topic();
    let discovery_configs = DiscoveryBuilder::new(device_id).build();

    let broker_url = if user.is_empty() {
        format!("mqtt://{}:{}", host, port)
    } else {
        format!("mqtt://{}:{}@{}:{}", user, password, host, port)
    };

    let ctx = MqttContext {
        device_id: device_id.to_string(),
        command_tx,
    };

    let avail_topic_for_lwt = availability_topic.clone();

    let client = EspMqttClient::new(
        broker_url,
        MqttClientSettings {
            client_id: Some(client_id.as_str()),
            keep_alive_interval: Some(std::time::Duration::from_secs(30)),
            last_will_topic: Some(avail_topic_for_lwt.as_str()),
            last_will_payload: Some(b"offline"),
            last_will_qos: QoS::AtLeastOnce,
            last_will_retain: true,
            ..Default::default()
        },
        move |event| {
            handle_mqtt_event(event, &ctx, &discovery_configs);
        },
    )
    .context("Failed to create MQTT client")?;

    info!("MQTT client created, connecting to {}:{}", host, port);
    Ok(client)
}

fn handle_mqtt_event(
    event: EventPayload,
    ctx: &MqttContext,
    discovery_configs: &[(String, String)],
) {
    match event {
        EventPayload::Connected(state) => {
            info!("MQTT connected (session present: {})", state.session_present);

            let topics = TopicBuilder::new(&ctx.device_id);
            let avail_topic = topics.availability_topic();

            // Publish online status
            let client_state = state; // borrow the state
            // Note: publish is done through the client returned from the constructor
            // The event handler doesn't have direct access to publish.
            // We'll handle initial publishing in the main loop instead.

            info!("MQTT connected, will publish discovery and subscribe");
        }

        EventPayload::Subscribed(topic_id) => {
            debug!("MQTT subscribed to topic ID: {}", topic_id);
        }

        EventPayload::Received { topic, data, .. } => {
            let topic_str = std::str::from_utf8(topic).unwrap_or("");
            debug!("MQTT received on '{}': {} bytes", topic_str, data.len());

            let cmd_base = TopicBuilder::new(&ctx.device_id).command_topic();

            if topic_str.starts_with(&cmd_base) {
                if let Some(cmd) = parse_command(&cmd_base, topic_str, data) {
                    info!("MQTT command: {:?}", cmd);
                    if ctx.command_tx.send(cmd).is_err() {
                        warn!("Command channel closed, dropping command");
                    }
                }
            }
        }

        EventPayload::Disconnected => {
            warn!("MQTT disconnected");
        }

        _ => {}
    }
}

/// Publish HA auto-discovery configs to the MQTT broker.
pub fn publish_discovery(client: &mut EspMqttClient, device_id: &str) -> Result<()> {
    let configs = DiscoveryBuilder::new(device_id).build();

    for (topic, payload) in &configs {
        client.publish(
            PubData::new(topic, QoS::AtLeastOnce, true, payload.as_bytes()),
        )?;
        debug!("Published discovery to {}", topic);
    }

    info!("Published {} discovery configs", configs.len());
    Ok(())
}

/// Publish current spa state as JSON to the state topic.
pub fn publish_state(
    client: &mut EspMqttClient,
    device_id: &str,
    status: &StatusUpdate,
) -> Result<()> {
    let topics = TopicBuilder::new(device_id);
    let state_topic = topics.state_topic();
    let json = status_to_json(status);

    client.publish(
        PubData::new(&state_topic, QoS::AtLeastOnce, false, json.as_bytes()),
    )?;

    debug!("Published state to {}", state_topic);
    Ok(())
}

/// Publish online/offline to the availability topic.
pub fn publish_availability(
    client: &mut EspMqttClient,
    device_id: &str,
    online: bool,
) -> Result<()> {
    let topics = TopicBuilder::new(device_id);
    let avail_topic = topics.availability_topic();
    let payload = if online { "online" } else { "offline" };

    client.publish(
        PubData::new(&avail_topic, QoS::AtLeastOnce, true, payload.as_bytes()),
    )?;

    debug!("Published availability: {}", payload);
    Ok(())
}

/// Subscribe to command topics.
pub fn subscribe_commands(client: &mut EspMqttClient, device_id: &str) -> Result<()> {
    let topics = TopicBuilder::new(device_id);
    let cmd_topic = format!("{}/#", topics.command_topic());

    client.subscribe(cmd_topic.as_str(), QoS::AtLeastOnce)?;
    info!("Subscribed to command topic: {}", cmd_topic);
    Ok(())
}
