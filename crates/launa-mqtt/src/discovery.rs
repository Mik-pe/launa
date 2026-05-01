//! Home Assistant MQTT auto-discovery message generation.
//!
//! Generates JSON discovery payloads for 32 HA entities using `alloc` only
//! (no serde_json dependency), so this works in `no_std` environments.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::escape::escape_json_string;
use crate::topics::TopicBuilder;

/// A discovery config payload with its topic and retain flag.
#[derive(Debug, Clone)]
pub struct DiscoveryMessage {
    pub topic: String,
    pub payload: String,
    pub retain: bool,
}

/// Generates Home Assistant MQTT auto-discovery config payloads for a spa device.
///
/// Produces 32 discovery messages covering sensors, switches, lights, fans,
/// selects, numbers, and binary sensors. Each message is a JSON config payload
/// with its corresponding MQTT topic.
pub struct DiscoveryBuilder {
    device_id: String,
    device_name: String,
    device_model: String,
    sw_version: String,
    manufacturer: String,
    /// If true, temperature entities use Celsius. Default: false (Fahrenheit).
    celsius: bool,
}

impl DiscoveryBuilder {
    pub fn new(device_id: &str) -> Self {
        DiscoveryBuilder {
            device_id: String::from(device_id),
            device_name: String::from("Launa Spa"),
            device_model: String::from("BP6013G1"),
            sw_version: String::from("unknown"),
            manufacturer: String::from("Launa"),
            celsius: false,
        }
    }

    /// Set temperature scale to Celsius (default is Fahrenheit).
    pub fn celsius(mut self, celsius: bool) -> Self {
        self.celsius = celsius;
        self
    }

    pub fn device_name(mut self, name: &str) -> Self {
        self.device_name = String::from(name);
        self
    }

    pub fn device_model(mut self, model: &str) -> Self {
        self.device_model = String::from(model);
        self
    }

    pub fn manufacturer(mut self, manufacturer: &str) -> Self {
        self.manufacturer = String::from(manufacturer);
        self
    }

    pub fn sw_version(mut self, version: &str) -> Self {
        self.sw_version = String::from(version);
        self
    }

    /// Generate all discovery config payloads for the spa device.
    /// Returns a vec of (topic, json_payload) pairs with retain=false (backward compat).
    pub fn build(&self) -> Vec<(String, String)> {
        self.build_messages()
            .into_iter()
            .map(|m| (m.topic, m.payload))
            .collect()
    }

    /// Generate all discovery config payloads with retain=true.
    /// HA auto-discovery messages should be published with retain so they
    /// survive broker restarts.
    pub fn build_with_retain(&self) -> Vec<DiscoveryMessage> {
        self.build_messages()
            .into_iter()
            .map(|mut m| {
                m.retain = true;
                m
            })
            .collect()
    }

    /// Internal: generate all discovery messages.
    fn build_messages(&self) -> Vec<DiscoveryMessage> {
        let topics = TopicBuilder::new(&self.device_id);
        let mut configs = Vec::new();

        let device_info = json_device_block(
            &self.device_id,
            &self.device_name,
            &self.manufacturer,
            &self.device_model,
            &self.sw_version,
        );
        let origin = json_origin(&self.sw_version);

        let state_topic = topics.state_topic();
        let avail_topic = topics.availability_topic();
        let cmd_topic = topics.command_topic();

        let (temp_unit, temp_min, temp_max, temp_step) = if self.celsius {
            ("°C", 10, 40, "0.5")
        } else {
            ("°F", 50, 104, "1")
        };

        // Temperature sensor
        configs.push(Self::make_sensor(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "temperature",
            "Water Temperature",
            "temperature",
            temp_unit,
            "{{ value_json.current_temp }}",
        ));

        // Set temperature number
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("number", "set_temperature"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Set Temperature","unique_id":"{}_set_temp","device_class":"temperature","unit_of_measurement":"{}","min":{},"max":{},"step":{},"state_topic":"{}","command_topic":"{}/set_temperature","value_template":"{{{{ value_json.set_temp }}}}","availability_topic":"{}"}}"#,
                device_info, origin, self.device_id, temp_unit, temp_min, temp_max, temp_step, state_topic, cmd_topic, avail_topic
            ),
            retain: false,
        });

        // Heating state binary sensor
        configs.push(Self::make_binary_sensor(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "heating",
            "Heating",
            "heat",
            "{{ value_json.is_heating }}",
        ));

        // Pump switches
        for i in 1..=6 {
            configs.push(Self::make_switch(
                &topics,
                &self.device_id,
                &device_info,
                &origin,
                &state_topic,
                &avail_topic,
                &format!("pump{}", i),
                &format!("Pump {}", i),
                &format!("{{{{ value_json.pump{}_on }}}}", i),
                &format!("{}/pump{}", cmd_topic, i),
            ));
        }

        // Lights
        for i in 1..=4 {
            let name = if i == 1 {
                String::from("Spa Light")
            } else {
                format!("Spa Light {}", i)
            };
            configs.push(DiscoveryMessage {
                topic: topics.discovery_topic("light", &format!("light{}", i)),
                payload: format!(
                    r#"{{"device":{},"origin":{},"name":"{}","unique_id":"{}_light{}","state_topic":"{}","command_topic":"{}/light{}","value_template":"{{{{ value_json.light{} }}}}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                    device_info, origin, name, self.device_id, i, state_topic, cmd_topic, i, i, avail_topic
                ),
                retain: false,
            });
        }

        // Blower fan
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("fan", "blower"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Blower","unique_id":"{}_blower","state_topic":"{}","command_topic":"{}/blower","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info, origin, self.device_id, state_topic, cmd_topic, avail_topic
            ),
            retain: false,
        });

        // Heat Mode select
        configs.push(Self::make_select(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "heat_mode",
            "Heat Mode",
            r#"["ready","rest","ready_in_rest"]"#,
            "{{ value_json.heating_mode }}",
            &format!("{}/heat_mode", cmd_topic),
        ));

        // Circulation Pump switch (now toggleable via ToggleItem::CirculationPump)
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "circulation_pump",
            "Circulation Pump",
            "{{ value_json.circ_pump }}",
            &format!("{}/circulation_pump", cmd_topic),
        ));

        // Temperature Range select
        configs.push(Self::make_select(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "temp_range",
            "Temperature Range",
            r#"["high","low"]"#,
            "{{ value_json.temp_range }}",
            &format!("{}/temp_range", cmd_topic),
        ));

        // Hold Mode switch
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "hold_mode",
            "Hold Mode",
            "{{ value_json.hold_mode }}",
            &format!("{}/hold_mode", cmd_topic),
        ));

        // AUX 1 switch (optimistic — no state feedback in Balboa protocol)
        configs.push(Self::make_switch_optimistic(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &avail_topic,
            "aux1",
            "AUX 1",
            &format!("{}/aux1", cmd_topic),
        ));

        // AUX 2 switch (optimistic — no state feedback in Balboa protocol)
        configs.push(Self::make_switch_optimistic(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &avail_topic,
            "aux2",
            "AUX 2",
            &format!("{}/aux2", cmd_topic),
        ));

        // Soak Mode switch (optimistic — no state feedback in Balboa protocol)
        configs.push(Self::make_switch_optimistic(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &avail_topic,
            "soak_mode",
            "Soak Mode",
            &format!("{}/soak_mode", cmd_topic),
        ));

        // Normal Operation switch (optimistic — no state feedback in Balboa protocol)
        configs.push(Self::make_switch_optimistic(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &avail_topic,
            "normal_operation",
            "Normal Operation",
            &format!("{}/normal_operation", cmd_topic),
        ));

        // Clear Notification switch (optimistic — no state feedback in Balboa protocol)
        configs.push(Self::make_switch_optimistic(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &avail_topic,
            "clear_notification",
            "Clear Notification",
            &format!("{}/clear_notification", cmd_topic),
        ));

        // Mister switch (now toggleable via ToggleItem::Mister)
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "mister",
            "Mister",
            "{{ value_json.mister }}",
            &format!("{}/mister", cmd_topic),
        ));

        // Fault sensor
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("sensor", "fault"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Last Fault","unique_id":"{}_fault","state_topic":"{}","value_template":"{{{{ value_json.last_fault }}}}","availability_topic":"{}"}}"#,
                device_info, origin, self.device_id, state_topic, avail_topic
            ),
            retain: false,
        });

        // Diagnostics sensor
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("sensor", "diagnostics"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Diagnostics","unique_id":"{}_diagnostics","state_topic":"{}","availability_topic":"{}","entity_category":"diagnostic"}}"#,
                device_info, origin, self.device_id, topics.diagnostics_topic(), avail_topic
            ),
            retain: false,
        });

        // Alert sensor
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("sensor", "alert"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Alert","unique_id":"{}_alert","state_topic":"{}","availability_topic":"{}","entity_category":"diagnostic"}}"#,
                device_info, origin, self.device_id, topics.alert_topic(), avail_topic
            ),
            retain: false,
        });

        // Sniff mode switch (passive RS-485 frame capture to MQTT)
        configs.push(Self::make_switch_optimistic(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &avail_topic,
            "sniff",
            "Sniff Mode",
            &format!("{}/sniff", cmd_topic),
        ));

        // Firmware version sensor (diagnostic, reads from state JSON)
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("sensor", "firmware_version"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Firmware Version","unique_id":"{}_firmware_version","state_topic":"{}","value_template":"{{{{value_json.firmware_version}}}}","availability_topic":"{}","entity_category":"diagnostic","icon":"mdi:information-outline"}}"#,
                device_info, origin, self.device_id, state_topic, avail_topic
            ),
            retain: false,
        });

        // Reboot button (optimistic switch — sends ON to command/reboot)
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("button", "reboot"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Reboot","unique_id":"{}_reboot","command_topic":"{}/reboot","payload_press":"ON","availability_topic":"{}","entity_category":"config","icon":"mdi:restart"}}"#,
                device_info, origin, self.device_id, cmd_topic, avail_topic
            ),
            retain: false,
        });

        // Clock sensor (displays current spa time HH:MM)
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("sensor", "clock"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Clock","unique_id":"{}_clock","state_topic":"{}","value_template":"{{{{ \"%02d:%02d\" | format(value_json.hour|default(0), value_json.minute|default(0)) }}}}","availability_topic":"{}","icon":"mdi:clock-outline","entity_category":"diagnostic"}}"#,
                device_info, origin, self.device_id, state_topic, avail_topic
            ),
            retain: false,
        });

        // Set time text input (accepts HH:MM)
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("text", "set_time"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Set Time","unique_id":"{}_set_time","command_topic":"{}/set_time","availability_topic":"{}","pattern":"^([0-9]|1[0-9]|2[0-3]):[0-5][0-9]$","icon":"mdi:clock-edit-outline","entity_category":"config"}}"#,
                device_info, origin, self.device_id, cmd_topic, avail_topic
            ),
            retain: false,
        });

        configs
    }

    #[allow(clippy::too_many_arguments)]
    fn make_sensor(
        topics: &TopicBuilder,
        device_id: &str,
        device_info: &str,
        origin: &str,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        device_class: &str,
        unit: &str,
        value_template: &str,
    ) -> DiscoveryMessage {
        DiscoveryMessage {
            topic: topics.discovery_topic("sensor", object_id),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"{}","unique_id":"{}_{}","device_class":"{}","unit_of_measurement":"{}","state_topic":"{}","value_template":"{}","availability_topic":"{}"}}"#,
                device_info,
                origin,
                name,
                device_id,
                object_id,
                device_class,
                unit,
                state_topic,
                value_template,
                avail_topic
            ),
            retain: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_binary_sensor(
        topics: &TopicBuilder,
        device_id: &str,
        device_info: &str,
        origin: &str,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        device_class: &str,
        value_template: &str,
    ) -> DiscoveryMessage {
        DiscoveryMessage {
            topic: topics.discovery_topic("binary_sensor", object_id),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"{}","unique_id":"{}_{}","device_class":"{}","state_topic":"{}","value_template":"{}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info,
                origin,
                name,
                device_id,
                object_id,
                device_class,
                state_topic,
                value_template,
                avail_topic
            ),
            retain: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_switch(
        topics: &TopicBuilder,
        device_id: &str,
        device_info: &str,
        origin: &str,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        value_template: &str,
        command_topic: &str,
    ) -> DiscoveryMessage {
        DiscoveryMessage {
            topic: topics.discovery_topic("switch", object_id),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"{}","unique_id":"{}_{}","state_topic":"{}","command_topic":"{}","value_template":"{}","payload_on":"true","payload_off":"false","availability_topic":"{}"}}"#,
                device_info,
                origin,
                name,
                device_id,
                object_id,
                state_topic,
                command_topic,
                value_template,
                avail_topic
            ),
            retain: false,
        }
    }

    /// Build an optimistic switch entity for commands that have no state feedback.
    /// HA will assume the state changed immediately after sending a command.
    #[allow(clippy::too_many_arguments)]
    fn make_switch_optimistic(
        topics: &TopicBuilder,
        device_id: &str,
        device_info: &str,
        origin: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        command_topic: &str,
    ) -> DiscoveryMessage {
        DiscoveryMessage {
            topic: topics.discovery_topic("switch", object_id),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"{}","unique_id":"{}_{}","command_topic":"{}","payload_on":"true","payload_off":"false","optimistic":true,"availability_topic":"{}"}}"#,
                device_info, origin, name, device_id, object_id, command_topic, avail_topic
            ),
            retain: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_select(
        topics: &TopicBuilder,
        device_id: &str,
        device_info: &str,
        origin: &str,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        options: &str,
        value_template: &str,
        command_topic: &str,
    ) -> DiscoveryMessage {
        DiscoveryMessage {
            topic: topics.discovery_topic("select", object_id),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"{}","unique_id":"{}_{}","state_topic":"{}","command_topic":"{}","value_template":"{}","options":{},"availability_topic":"{}"}}"#,
                device_info,
                origin,
                name,
                device_id,
                object_id,
                state_topic,
                command_topic,
                value_template,
                options,
                avail_topic
            ),
            retain: false,
        }
    }
}

// \u2500\u2500 JSON helper functions \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// Build the `"device"` JSON block.
fn json_device_block(
    device_id: &str,
    name: &str,
    manufacturer: &str,
    model: &str,
    sw_version: &str,
) -> String {
    format!(
        r#"{{"identifiers":["{}"],"name":"{}","manufacturer":"{}","model":"{}","sw_version":"{}"}}"#,
        escape_json_string(device_id),
        escape_json_string(name),
        escape_json_string(manufacturer),
        escape_json_string(model),
        escape_json_string(sw_version),
    )
}

/// Build the `"origin"` JSON block.
fn json_origin(sw_version: &str) -> String {
    format!(
        r#"{{"name":"launa-firmware","sw_version":"{}"}}"#,
        escape_json_string(sw_version),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse a JSON payload string into a serde_json::Value for assertions.
    /// Only available in test builds (which use the std feature).
    fn parse_json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).expect("valid JSON")
    }

    #[test]
    fn test_discovery_generates_valid_json() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        assert_eq!(
            configs.len(),
            32,
            "expected exactly 32 discovery configs, got {}",
            configs.len()
        );

        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));
        }
    }

    #[test]
    fn test_build_with_retain() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let messages = builder.build_with_retain();

        assert_eq!(
            messages.len(),
            32,
            "expected exactly 32 discovery messages, got {}",
            messages.len()
        );
        for msg in &messages {
            assert!(msg.retain, "discovery messages should have retain=true");
            let _: serde_json::Value = serde_json::from_str(&msg.payload)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", msg.topic, e));
        }
    }

    #[test]
    fn test_discovery_topics_match_pattern() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        for (topic, _) in &configs {
            let parts: Vec<&str> = topic.split('/').collect();
            assert_eq!(parts[0], "homeassistant");
            assert_eq!(parts[2], "test_spa_001");
            assert_eq!(parts.last(), Some(&"config"));
        }
    }

    #[test]
    fn test_discovery_unique_ids() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        let mut unique_ids = Vec::new();
        for (_, json_str) in &configs {
            let v: serde_json::Value = serde_json::from_str(json_str).unwrap();
            let uid = v["unique_id"].as_str().unwrap().to_string();
            assert!(
                uid.starts_with("test_spa_001_"),
                "unique_id {} should start with device_id",
                uid
            );
            unique_ids.push(uid);
        }

        let mut sorted = unique_ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), unique_ids.len(), "duplicate unique_ids found");
    }

    #[test]
    fn test_discovery_command_topics() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        for (_, json_str) in &configs {
            let v: serde_json::Value = serde_json::from_str(json_str).unwrap();
            if let Some(cmd_topic) = v.get("command_topic").and_then(|t| t.as_str()) {
                assert!(
                    cmd_topic.starts_with("launa/test_spa_001/command/"),
                    "command_topic {} should be under device command base",
                    cmd_topic
                );
            }
        }
    }

    #[test]
    fn test_discovery_includes_origin_in_all_payloads() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            let origin = v
                .get("origin")
                .unwrap_or_else(|| panic!("Missing 'origin' field in payload for topic {}", topic));
            assert_eq!(origin["name"].as_str(), Some("launa-firmware"));
            assert!(
                origin.get("sw_version").is_some(),
                "origin missing sw_version"
            );
        }
    }

    #[test]
    fn test_discovery_device_includes_sw_version() {
        let builder = DiscoveryBuilder::new("test_spa_001").sw_version("1.2.3");
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            let device = v
                .get("device")
                .unwrap_or_else(|| panic!("Missing 'device' field in payload for topic {}", topic));
            assert_eq!(
                device["sw_version"].as_str(),
                Some("1.2.3"),
                "device sw_version mismatch in topic {}",
                topic
            );
        }
    }

    #[test]
    fn test_diagnostic_entities_have_entity_category() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        let mut diag_count = 0;
        for (topic, json_str) in &configs {
            if topic.contains("/diagnostics/") || topic.contains("/alert/") {
                let v = parse_json(json_str);
                let category = v.get("entity_category").and_then(|c| c.as_str());
                assert_eq!(
                    category,
                    Some("diagnostic"),
                    "entity_category should be 'diagnostic' for topic {}",
                    topic
                );
                diag_count += 1;
            }
        }
        assert_eq!(diag_count, 2, "should find exactly 2 diagnostic entities");
    }

    #[test]
    fn test_discovery_builder_field_parity() {
        let builder = DiscoveryBuilder::new("test_spa_001").sw_version("0.1.0");
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            assert!(v.get("device").is_some(), "missing 'device' in {}", topic);
            assert!(v.get("origin").is_some(), "missing 'origin' in {}", topic);
            assert!(v.get("name").is_some(), "missing 'name' in {}", topic);
            assert!(
                v.get("unique_id").is_some(),
                "missing 'unique_id' in {}",
                topic
            );
            let is_optimistic = v
                .get("optimistic")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Button entities are command-only (have payload_press, no state_topic)
            // Text entities are command-only for set_time
            let is_button = v.get("payload_press").is_some();
            let is_text = topic.contains("/text/");
            if !is_optimistic && !is_button && !is_text {
                assert!(
                    v.get("state_topic").is_some(),
                    "missing 'state_topic' in {}",
                    topic
                );
            }
            assert!(
                v.get("availability_topic").is_some(),
                "missing 'availability_topic' in {}",
                topic
            );
        }
    }

    // --- Toggle item discovery tests ---

    #[test]
    fn test_discovery_includes_mister_switch() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let mister = configs.iter().find(|(t, _)| t.contains("/mister"));
        assert!(mister.is_some(), "mister discovery entity missing");
        let v = parse_json(&mister.unwrap().1);
        assert!(
            v.get("command_topic").is_some(),
            "mister should be a switch with command_topic"
        );
        assert!(v["unique_id"].as_str().unwrap().contains("mister"));
    }

    #[test]
    fn test_discovery_includes_circulation_pump_switch() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let circ = configs
            .iter()
            .find(|(t, _)| t.contains("/circulation_pump"));
        assert!(circ.is_some(), "circulation_pump discovery entity missing");
        let v = parse_json(&circ.unwrap().1);
        assert!(
            v.get("command_topic").is_some(),
            "circulation_pump should be a switch with command_topic"
        );
        assert!(v["unique_id"]
            .as_str()
            .unwrap()
            .contains("circulation_pump"));
    }

    #[test]
    fn test_discovery_includes_light3() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let light3 = configs.iter().find(|(t, _)| t.contains("/light3"));
        assert!(light3.is_some(), "light3 discovery entity missing");
        let v = parse_json(&light3.unwrap().1);
        assert!(
            v.get("command_topic").is_some(),
            "light3 should have command_topic"
        );
        assert!(v["unique_id"].as_str().unwrap().contains("light3"));
    }

    #[test]
    fn test_discovery_includes_light4() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let light4 = configs.iter().find(|(t, _)| t.contains("/light4"));
        assert!(light4.is_some(), "light4 discovery entity missing");
        let v = parse_json(&light4.unwrap().1);
        assert!(
            v.get("command_topic").is_some(),
            "light4 should have command_topic"
        );
        assert!(v["unique_id"].as_str().unwrap().contains("light4"));
    }

    #[test]
    fn test_discovery_includes_aux1_switch() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let aux1 = configs.iter().find(|(t, _)| t.contains("/aux1"));
        assert!(aux1.is_some(), "aux1 discovery entity missing");
        let v = parse_json(&aux1.unwrap().1);
        assert!(v.get("command_topic").is_some());
        assert!(v["unique_id"].as_str().unwrap().contains("aux1"));
    }

    #[test]
    fn test_discovery_includes_aux2_switch() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let aux2 = configs.iter().find(|(t, _)| t.contains("/aux2"));
        assert!(aux2.is_some(), "aux2 discovery entity missing");
        let v = parse_json(&aux2.unwrap().1);
        assert!(v.get("command_topic").is_some());
        assert!(v["unique_id"].as_str().unwrap().contains("aux2"));
    }

    #[test]
    fn test_discovery_includes_soak_mode_switch() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let soak = configs.iter().find(|(t, _)| t.contains("/soak_mode"));
        assert!(soak.is_some(), "soak_mode discovery entity missing");
        let v = parse_json(&soak.unwrap().1);
        assert!(v.get("command_topic").is_some());
        assert!(v["unique_id"].as_str().unwrap().contains("soak_mode"));
    }

    #[test]
    fn test_discovery_includes_normal_operation_switch() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let norm = configs
            .iter()
            .find(|(t, _)| t.contains("/normal_operation"));
        assert!(norm.is_some(), "normal_operation discovery entity missing");
        let v = parse_json(&norm.unwrap().1);
        assert!(v.get("command_topic").is_some());
        assert!(v["unique_id"]
            .as_str()
            .unwrap()
            .contains("normal_operation"));
    }

    #[test]
    fn test_discovery_includes_clear_notification_switch() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        let clear = configs
            .iter()
            .find(|(t, _)| t.contains("/clear_notification"));
        assert!(
            clear.is_some(),
            "clear_notification discovery entity missing"
        );
        let v = parse_json(&clear.unwrap().1);
        assert!(v.get("command_topic").is_some());
        assert!(v["unique_id"]
            .as_str()
            .unwrap()
            .contains("clear_notification"));
    }

    #[test]
    fn test_discovery_new_entities_have_valid_json() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));
        }
    }

    #[test]
    fn test_discovery_entity_count_increased() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        assert!(
            configs.len() >= 27,
            "should have at least 27 entities (base count), got {}",
            configs.len()
        );
    }

    // --- JSON escaping tests for discovery configs ---
    // Consolidated from 12 individual tests into 3 focused tests:
    // 1. All payloads produce valid JSON with special characters
    // 2. Device block fields round-trip correctly through JSON parsing
    // 3. Normal (non-special) names still work correctly

    fn builder_with_special_chars() -> DiscoveryBuilder {
        DiscoveryBuilder::new("test_spa")
            .device_name(r#"My "Spa" \Unit\"#)
            .manufacturer("Mfr\nLine2\tTab")
            .device_model(r#"BP\6013"G1""#)
            .sw_version("v1.0\r\nbeta\x07")
    }

    #[test]
    fn test_discovery_special_chars_all_payloads_valid_json() {
        // Verify that special characters in device name, manufacturer, model,
        // and sw_version produce valid JSON in ALL discovery payloads.
        let builder = builder_with_special_chars();
        let configs = builder.build();
        assert!(
            configs.len() >= 27,
            "expected at least 27 configs (base count)"
        );

        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str).unwrap_or_else(|e| {
                panic!(
                    "Invalid JSON for topic {} with special chars: {}\nPayload: {}",
                    topic, e, json_str
                )
            });
        }

        // Also verify retain payloads produce valid JSON
        let messages = builder.build_with_retain();
        for msg in &messages {
            let _: serde_json::Value = serde_json::from_str(&msg.payload).unwrap_or_else(|e| {
                panic!(
                    "Invalid JSON for topic {} with special chars (retain): {}\nPayload: {}",
                    msg.topic, e, msg.payload
                )
            });
        }
    }

    #[test]
    fn test_discovery_special_chars_fields_round_trip() {
        // Verify all special-character fields round-trip through JSON correctly.
        // This consolidates the individual tests for device_name, manufacturer,
        // model, sw_version in device block, and sw_version in origin block.
        let builder = builder_with_special_chars();
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            let device = v
                .get("device")
                .unwrap_or_else(|| panic!("Missing 'device' in topic {}", topic));
            assert_eq!(
                device["name"].as_str(),
                Some(r#"My "Spa" \Unit\"#),
                "device name mismatch in topic {}",
                topic
            );
            assert_eq!(
                device["manufacturer"].as_str(),
                Some("Mfr\nLine2\tTab"),
                "manufacturer mismatch in topic {}",
                topic
            );
            assert_eq!(
                device["model"].as_str(),
                Some(r#"BP\6013"G1""#),
                "model mismatch in topic {}",
                topic
            );
            assert_eq!(
                device["sw_version"].as_str(),
                Some("v1.0\r\nbeta\u{0007}"),
                "device sw_version mismatch in topic {}",
                topic
            );

            let origin = v.get("origin").unwrap();
            assert_eq!(
                origin["sw_version"].as_str(),
                Some("v1.0\r\nbeta\u{0007}"),
                "origin sw_version mismatch in topic {}",
                topic
            );
        }
    }

    #[test]
    fn test_json_device_block_escapes_all_fields() {
        // Consolidated test for json_device_block and json_origin helpers:
        // quotes, backslashes, control chars, newlines, tabs, null byte,
        // and combined special chars.
        let cases: &[(&str, &str, &str, &str)] = &[
            // (name, manufacturer, model, sw_version)
            (r#"Name "Quoted""#, "Mfr", "Model", "1.0"), // quotes
            (r#"Path\To\Spa"#, "Mfr", "Model", "1.0"),   // backslashes
            ("Before\x07After", "Mfr", "Model", "1.0"),  // control char
            ("Line1\nLine2\tTab", "Mfr", "Model", "1.0"), // newline + tab
            ("Before\x00After", "Mfr", "Model", "1.0"),  // null byte
        ];

        for &(name, mfr, model, ver) in cases {
            let block = json_device_block("id1", name, mfr, model, ver);
            let v = parse_json(&block);
            assert_eq!(v["name"].as_str(), Some(name), "name round-trip failed");
            assert_eq!(v["manufacturer"].as_str(), Some(mfr));
            assert_eq!(v["model"].as_str(), Some(model));
            assert_eq!(v["sw_version"].as_str(), Some(ver));
        }

        // Test combined special chars in a single string
        let combined = alloc::format!("A\\B\"C\nD\rE\tF\x01G");
        let block = json_device_block("id1", &combined, "Mfr", "Model", "1.0");
        let v = parse_json(&block);
        assert_eq!(
            v["name"].as_str(),
            Some(alloc::format!("A\\B\"C\nD\rE\tF\u{0001}G").as_str())
        );

        // Test json_origin helper
        let origin = json_origin("v1.0\nbeta\r\n\x07");
        let v = parse_json(&origin);
        assert_eq!(v["sw_version"].as_str(), Some("v1.0\nbeta\r\n\u{0007}"));
    }

    #[test]
    fn test_discovery_normal_names_still_work() {
        // Ensure normal (non-special) names still work correctly
        let builder = DiscoveryBuilder::new("spa1")
            .device_name("My Spa")
            .manufacturer("Acme Corp")
            .device_model("BP6013G1")
            .sw_version("1.0.0");
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            assert_eq!(v["device"]["name"].as_str(), Some("My Spa"), "in {}", topic);
            assert_eq!(
                v["device"]["manufacturer"].as_str(),
                Some("Acme Corp"),
                "in {}",
                topic
            );
            assert_eq!(
                v["device"]["model"].as_str(),
                Some("BP6013G1"),
                "in {}",
                topic
            );
            assert_eq!(
                v["device"]["sw_version"].as_str(),
                Some("1.0.0"),
                "in {}",
                topic
            );
        }
    }
}
