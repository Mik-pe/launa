//! Simulated MQTT broker for integration testing.
//!
//! Records published messages and allows tests to verify that the controller
//! publishes correct state, discovery configs, and availability messages.
//!
//! # Features (all off by default for backward compatibility)
//!
//! - **QoS 1 tracking**: `publish_qos1()` assigns packet IDs, `puback()` clears them.
//!   Use `unacked_count()` and `assert_all_acked()` to verify delivery.
//! - **Subscription matching**: When subscriptions exist, `publish()` only records
//!   messages whose topic matches a subscribed pattern (exact match). With zero
//!   subscriptions, all messages are recorded (backward compatible).
//! - **In-order delivery**: Messages are recorded in FIFO order (natural `Vec::push`).
//! - **Loss rate**: `set_loss_rate(rate)` causes an approximate fraction of publishes
//!   to be silently dropped. Rate 0.0 = no loss, 1.0 = total loss.
//! - **Connection loss**: `simulate_disconnect()` drops all publishes. `simulate_reconnect()`
//!   restores. `dropped_count()` tracks messages lost during disconnect.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use launa_mqtt::topics::TopicBuilder;
use std::collections::{HashMap, HashSet};

use crate::lcg::lcg_next;

/// A mock MQTT broker that records all publications for test verification.
///
/// All features default to off, preserving identical behavior to the original
/// simple recorder. Enable features via the setter methods.
pub struct SimBroker {
    device_id: String,
    /// Recorded messages in FIFO order.
    published: Vec<(String, String)>,
    /// Active subscriptions (exact match). Empty = accept all (backward compat).
    subscribed_topics: HashSet<String>,
    /// QoS 1 tracking: unacked packet ID → (topic, payload).
    unacked: HashMap<u16, (String, String)>,
    /// Next packet ID for QoS 1 publishes. Monotonically increasing.
    next_packet_id: u16,
    /// Fraction of publishes silently dropped (0.0 = none, 1.0 = all).
    loss_rate: f32,
    /// Pseudo-random state for loss rate (simple LCG).
    loss_rng_state: u64,
    /// Whether the broker is "disconnected" (drops all publishes).
    disconnected: bool,
    /// Count of messages dropped during disconnect (cumulative, not reset on reconnect).
    dropped_during_disconnect: usize,
}

impl SimBroker {
    pub fn new(device_id: &str) -> Self {
        SimBroker {
            device_id: device_id.to_string(),
            published: Vec::new(),
            subscribed_topics: HashSet::new(),
            unacked: HashMap::new(),
            next_packet_id: 1,
            loss_rate: 0.0,
            loss_rng_state: 12345,
            disconnected: false,
            dropped_during_disconnect: 0,
        }
    }

    /// Record a publication (simulates `mqtt_client.publish()`).
    ///
    /// Respects subscription matching, loss rate, and disconnect state.
    /// When no subscriptions exist, all messages are recorded (backward compatible).
    /// When subscriptions exist, only messages matching a subscribed topic are recorded.
    pub fn publish(&mut self, topic: &str, payload: &str) {
        if !self.try_accept(topic) {
            return;
        }
        self.published
            .push((topic.to_string(), payload.to_string()));
    }

    /// Publish with QoS 1: assigns a packet ID and tracks until PUBACK.
    ///
    /// Returns the assigned packet ID, or 0 if the publish was dropped
    /// (due to disconnect or loss rate).
    pub fn publish_qos1(&mut self, topic: &str, payload: &str) -> u16 {
        if !self.try_accept(topic) {
            return 0;
        }
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        if self.next_packet_id == 0 {
            self.next_packet_id = 1;
        }
        self.published
            .push((topic.to_string(), payload.to_string()));
        self.unacked
            .insert(id, (topic.to_string(), payload.to_string()));
        id
    }

    /// Acknowledge a QoS 1 publish by packet ID.
    pub fn puback(&mut self, packet_id: u16) {
        self.unacked.remove(&packet_id);
    }

    /// Count of unacked QoS 1 messages.
    pub fn unacked_count(&self) -> usize {
        self.unacked.len()
    }

    /// Assert that all QoS 1 messages have been acked. Panics if any remain.
    pub fn assert_all_acked(&self) {
        if !self.unacked.is_empty() {
            panic!(
                "assert_all_acked: {} unacked QoS 1 messages remain",
                self.unacked.len()
            );
        }
    }

    /// Set the approximate fraction of publishes to silently drop.
    ///
    /// - `0.0`: no loss (default)
    /// - `1.0`: total loss
    /// - `0.5`: ~50% dropped (approximate, using a simple PRNG)
    pub fn set_loss_rate(&mut self, rate: f32) {
        self.loss_rate = rate.clamp(0.0, 1.0);
    }

    /// Simulate a network disconnect. Subsequent publishes are silently dropped.
    pub fn simulate_disconnect(&mut self) {
        self.disconnected = true;
    }

    /// Simulate a network reconnect. Publishes are recorded again.
    /// Messages lost during disconnect are gone; `dropped_count()` tracks the total.
    pub fn simulate_reconnect(&mut self) {
        self.disconnected = false;
    }

    /// Count of messages dropped during disconnect (cumulative).
    pub fn dropped_count(&self) -> usize {
        self.dropped_during_disconnect
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Check if a publish should be accepted based on current state.
    /// Returns false (and increments counters) if the message should be dropped.
    fn try_accept(&mut self, topic: &str) -> bool {
        // Disconnect takes priority
        if self.disconnected {
            self.dropped_during_disconnect += 1;
            return false;
        }

        // Loss rate
        if self.loss_rate > 0.0 && self.loss_roll() {
            return false;
        }

        // Subscription matching (empty = accept all)
        if !self.subscribed_topics.is_empty() && !self.subscribed_topics.contains(topic) {
            return false;
        }

        true
    }

    /// Simple LCG PRNG roll for loss rate. Returns true if this message should be dropped.
    fn loss_roll(&mut self) -> bool {
        lcg_next(&mut self.loss_rng_state);
        // Use lower 16 bits mapped to [0, 1)
        let fraction = (self.loss_rng_state as u16) as f32 / (u16::MAX as f32);
        fraction < self.loss_rate
    }

    // -----------------------------------------------------------------------
    // Original methods (preserved)
    // -----------------------------------------------------------------------

    /// Publish a discovery config.
    pub fn publish_discovery(&mut self, device_id: &str) {
        let configs = launa_mqtt::discovery::DiscoveryBuilder::new(device_id).build();
        for (topic, payload) in &configs {
            // Discovery bypasses subscription/loss/disconnect (internal operation)
            self.published.push((topic.clone(), payload.clone()));
        }
    }

    /// Publish availability status.
    pub fn publish_availability(&mut self, online: bool) {
        let topics = TopicBuilder::new(&self.device_id);
        let topic = topics.availability_topic();
        let payload = if online { "online" } else { "offline" };
        // Availability bypasses subscription/loss/disconnect (internal operation)
        self.published.push((topic, payload.to_string()));
    }

    /// Publish spa state JSON.
    pub fn publish_state(&mut self, status: &launa_protocol::status::StatusUpdate) {
        let topics = TopicBuilder::new(&self.device_id);
        let topic = topics.state_topic();
        let json = launa_mqtt::state::status_to_json(status, None, None, false, false);
        // State bypasses subscription/loss/disconnect (internal operation)
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
        self.published
            .iter()
            .rev()
            .find(|(t, _)| t == &state_topic)
            .map(|(_, p)| p.as_str())
    }

    /// Find all discovery payloads published.
    pub fn discovery_payloads(&self) -> Vec<&str> {
        self.published
            .iter()
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
    use launa_protocol::status::{
        HeatingMode, PumpState, StatusUpdate, TempRange, TemperatureScale, TimeFormat,
    };
    use launa_protocol::Temperature;

    fn sample_status() -> StatusUpdate {
        StatusUpdate {
            current_temp: Some(Temperature::fahrenheit(100.0)),
            set_temp: Temperature::fahrenheit(104.0),
            hour: 14,
            minute: 30,
            heating_mode: HeatingMode::Ready,
            temperature_scale: TemperatureScale::Fahrenheit,
            time_format: TimeFormat::Hour24,
            filter_mode: 0,
            is_heating: true,
            temp_range: TempRange::High,
            pumps: [PumpState::Off; 6],
            circ_pump: false,
            blower: false,
            mister: false,
            lights: [false; 4],
            is_priming: false,
            is_hold: false,
            notification_type: 0,
            panel_locked: false,
            settings_lock: false,
            m8_cycle_time: 0,
            sensor_a_temp: Some(Temperature::fahrenheit(98.0)),
            sensor_b_temp: None,
            hold_timer_minutes: None,
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
        assert_eq!(discoveries.len(), 29);
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

    #[test]
    fn test_simbroker_qos1_puback() {
        let mut broker = SimBroker::new("test_spa");

        // QoS 1 publish assigns packet IDs starting from 1
        let id1 = broker.publish_qos1("topic/a", "payload1");
        assert_eq!(id1, 1);

        let id2 = broker.publish_qos1("topic/b", "payload2");
        assert_eq!(id2, 2);

        // Both are unacked
        assert_eq!(broker.unacked_count(), 2);

        // ACK the first one
        broker.puback(id1);
        assert_eq!(broker.unacked_count(), 1);

        // ACK the second one
        broker.puback(id2);
        assert_eq!(broker.unacked_count(), 0);

        // assert_all_acked should not panic now
        broker.assert_all_acked();
    }

    #[test]
    #[should_panic(expected = "unacked QoS 1 messages")]
    fn test_simbroker_assert_all_acked_panics() {
        let mut broker = SimBroker::new("test_spa");
        broker.publish_qos1("topic/a", "payload1");
        broker.assert_all_acked(); // should panic
    }

    #[test]
    fn test_simbroker_subscription_filtering() {
        let mut broker = SimBroker::new("test_spa");

        // No subscriptions — publish should be recorded (backward compatible default)
        broker.publish("any/topic", "payload");
        assert_eq!(broker.publish_count(), 1);

        // Subscribe to topic A
        broker.subscribe("topic/a");
        broker.subscribe("topic/b");

        // Publish to subscribed topic A — should be recorded
        broker.publish("topic/a", "hello");
        assert_eq!(broker.count_topic("topic/a"), 1);

        // Publish to subscribed topic B — should be recorded
        broker.publish("topic/b", "world");
        assert_eq!(broker.count_topic("topic/b"), 1);

        // Publish to unsubscribed topic C — should NOT be recorded
        broker.publish("topic/c", "dropped");
        assert_eq!(broker.count_topic("topic/c"), 0);

        // Total: 1 (before subscribe) + 2 (subscribed) = 3
        assert_eq!(broker.publish_count(), 3);
    }

    #[test]
    fn test_simbroker_loss_rate() {
        // Test loss rate in isolation (no subscription filtering interference)
        let mut broker = SimBroker::new("test_spa");

        // Rate 0.0: no loss (all recorded)
        broker.set_loss_rate(0.0);
        for _ in 0..100 {
            broker.publish("topic", "payload");
        }
        assert_eq!(broker.publish_count(), 100);

        broker.take_all();

        // Rate 1.0: total loss (none recorded)
        broker.set_loss_rate(1.0);
        for _ in 0..100 {
            broker.publish("topic", "payload");
        }
        assert_eq!(broker.publish_count(), 0);

        // Fresh broker for 0.5 rate test
        let mut broker2 = SimBroker::new("test_spa");
        broker2.set_loss_rate(0.5);
        for _ in 0..1000 {
            broker2.publish("topic", "payload");
        }
        let count = broker2.publish_count();
        // With 1000 samples, expect roughly 400-600 (allowing wide margin)
        assert!(
            count > 300 && count < 700,
            "Expected ~500 publishes with 0.5 loss rate, got {}",
            count
        );
    }

    #[test]
    fn test_simbroker_disconnect_drops() {
        let mut broker = SimBroker::new("test_spa");
        broker.publish("before", "still works");
        assert_eq!(broker.publish_count(), 1);

        // Disconnect
        broker.simulate_disconnect();

        // Publishes during disconnect are silently dropped
        broker.publish("topic/a", "dropped1");
        broker.publish("topic/b", "dropped2");
        broker.publish("topic/c", "dropped3");

        // Count unchanged — the 3 publishes were dropped
        assert_eq!(broker.publish_count(), 1);

        // dropped_count tracks the losses
        assert_eq!(broker.dropped_count(), 3);
    }

    #[test]
    fn test_simbroker_reconnect_restores() {
        let mut broker = SimBroker::new("test_spa");

        // Normal publish works
        broker.publish("before", "works");
        assert_eq!(broker.publish_count(), 1);

        // Disconnect
        broker.simulate_disconnect();
        broker.publish("lost", "dropped during disconnect");
        assert_eq!(broker.publish_count(), 1);
        assert_eq!(broker.dropped_count(), 1);

        // Reconnect
        broker.simulate_reconnect();

        // Publishes work again
        broker.publish("after", "restored");
        assert_eq!(broker.publish_count(), 2);
        assert_eq!(broker.count_topic("after"), 1);

        // Lost messages are gone (not re-delivered)
        assert_eq!(broker.count_topic("lost"), 0);

        // dropped_count still tracks historical losses
        assert_eq!(broker.dropped_count(), 1);
    }

    #[test]
    fn test_simbroker_default_identical_to_current() {
        // All new features off by default — behavior identical to original
        let mut broker = SimBroker::new("test_spa");

        // publish() works as before (no subscription filtering when no subs)
        broker.publish("topic/a", "payload");
        assert_eq!(broker.publish_count(), 1);

        // No loss by default
        for _ in 0..50 {
            broker.publish("topic/b", "payload");
        }
        assert_eq!(broker.count_topic("topic/b"), 50);

        // Not disconnected
        assert_eq!(broker.dropped_count(), 0);

        // unacked_count is 0 (no QoS tracking until publish_qos1 used)
        assert_eq!(broker.unacked_count(), 0);
        broker.assert_all_acked(); // should not panic
    }

    #[test]
    fn test_simbroker_disconnect_drops_qos1() {
        let mut broker = SimBroker::new("test_spa");

        broker.simulate_disconnect();

        // QoS 1 publish during disconnect should be dropped
        let id = broker.publish_qos1("topic/a", "dropped");
        assert_eq!(id, 0); // ID 0 indicates failure
        assert_eq!(broker.unacked_count(), 0);
        assert_eq!(broker.dropped_count(), 1);
    }

    #[test]
    fn test_simbroker_loss_rate_does_not_affect_dropped_count() {
        let mut broker = SimBroker::new("test_spa");

        // Loss rate only affects recording, not dropped_count (which is for disconnects)
        broker.set_loss_rate(1.0);
        for _ in 0..10 {
            broker.publish("topic", "payload");
        }
        assert_eq!(broker.publish_count(), 0);
        assert_eq!(broker.dropped_count(), 0); // Not a disconnect loss
    }
}
