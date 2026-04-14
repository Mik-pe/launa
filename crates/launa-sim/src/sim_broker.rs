//! Simulated MQTT broker for integration testing.
//!
//! Records published messages and allows tests to verify that the controller
//! publishes correct state, discovery configs, and availability messages.

use std::collections::HashSet;
use launa_mqtt::topics::TopicBuilder;

/// A mock MQTT broker that records all publications for test verification.
pub struct SimBroker {
    device_id: String,
    published: Vec<(String, String)>,
    subscribed_topics: HashSet<String>,
}

impl SimBroker {
    pub fn new(device_id: &str) -> Self {
        SimBroker {
            device_id: device_id.to_string(),
            published: Vec::new(),
            subscribed_topics: HashSet::new(),
        }
    }

    /// Record a publication (simulates `mqtt_client.publish()`).
    pub fn publish(&mut self, topic: &str, payload: &str) {
        self.published.push((topic.to_string(), payload.to_string()));
    }

    /// Publish a discovery config.
    pub fn publish_discovery(&mut self, device_id: &str) {
        let configs = launa_mqtt::discovery::DiscoveryBuilder::new(device_id).build();
        for (topic, payload) in &configs {
            self.published.push((topic.clone(), payload.clone()));
        }
    }

    /// Publish availability status.
    pub fn publish_availability(&mut self, online: bool) {
        let topics = TopicBuilder::new(&self.device_id);
        let topic = topics.availability_topic();
        let payload = if online { "online" } else { "offline" };
        self.published.push((topic, payload.to_string()));
    }

    /// Publish spa state JSON.
    pub fn publish_state(&mut self, status: &launa_protocol::status::StatusUpdate) {
        let topics = TopicBuilder::new(&self.device_id);
        let topic = topics.state_topic();
        let json = launa_mqtt::state::status_to_json(status);
        self.published.push((topic, json));
    }

    /// Subscribe to a topic.
    pub fn subscribe(&mut self, topic: &str) {
        self.subscribed_topics.insert(topic.to_string());
    }

    /// Take all published messages, clearing the buffer.
    pub fn take_all(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.published)
    }

    /// Find the last state payload published.
    pub fn last_state(&self) -> Option<&str> {
        let state_topic = TopicBuilder::new(&self.device_id).state_topic();
        self.published.iter()
            .rev()
            .find(|(t, _)| t == &state_topic)
            .map(|(_, p)| p.as_str())
    }

    /// Find all discovery payloads published.
    pub fn discovery_payloads(&self) -> Vec<&str> {
        self.published.iter()
            .filter(|(t, _)| t.starts_with("homeassistant/"))
            .map(|(_, p)| p.as_str())
            .collect()
    }

    /// Count total publications.
    pub fn publish_count(&self) -> usize {
        self.published.len()
    }

    /// Count publications to a specific topic.
    pub fn count_topic(&self, topic: &str) -> usize {
        self.published.iter().filter(|(t, _)| t == topic).count()
    }

    /// Check if a topic was subscribed to.
    pub fn is_subscribed(&self, topic: &str) -> bool {
        self.subscribed_topics.contains(topic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launa_protocol::status::{StatusUpdate, HeatingMode, TemperatureScale, TempRange, PumpState, TimeFormat};

    fn sample_status() -> StatusUpdate {
        StatusUpdate {
            current_temp: Some(100.0),
            set_temp: 104.0,
            hour: 14,
            minute: 30,
            heating_mode: HeatingMode::Ready,
            temperature_scale: TemperatureScale::Fahrenheit,
            time_format: TimeFormat::Hour24,
            filter_mode: 0,
            is_heating: true,
            temp_range: TempRange::High,
            pump1: PumpState::Off,
            pump2: PumpState::Off,
            pump3: PumpState::Off,
            circ_pump: false,
            blower: false,
            mister: false,
            light1: false,
            is_priming: false,
            is_hold: false,
        }
    }

    #[test]
    fn test_publish_state() {
        let mut broker = SimBroker::new("test_spa");
        broker.publish_state(&sample_status());

        let state = broker.last_state().unwrap();
        assert!(state.contains("\"current_temp\":100"));
        assert!(state.contains("\"set_temp\":104"));
    }

    #[test]
    fn test_publish_discovery() {
        let mut broker = SimBroker::new("test_spa");
        broker.publish_discovery("test_spa");

        let discoveries = broker.discovery_payloads();
        assert_eq!(discoveries.len(), 14);
    }

    #[test]
    fn test_publish_availability() {
        let mut broker = SimBroker::new("test_spa");
        broker.publish_availability(true);

        let avail_topic = TopicBuilder::new("test_spa").availability_topic();
        assert_eq!(broker.count_topic(&avail_topic), 1);
    }

    #[test]
    fn test_take_all_clears() {
        let mut broker = SimBroker::new("test_spa");
        broker.publish_state(&sample_status());
        assert_eq!(broker.publish_count(), 1);

        let all = broker.take_all();
        assert_eq!(all.len(), 1);
        assert_eq!(broker.publish_count(), 0);
    }
}
