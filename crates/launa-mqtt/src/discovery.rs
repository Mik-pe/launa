//! Home Assistant MQTT auto-discovery message generation.
//!
//! Generates JSON discovery payloads for 20 HA entities using `alloc` only
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

        // AUX 1 switch
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "aux1",
            "AUX 1",
            "{{ value_json.aux1 }}",
            &format!("{}/aux1", cmd_topic),
        ));

        // AUX 2 switch
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "aux2",
            "AUX 2",
            "{{ value_json.aux2 }}",
            &format!("{}/aux2", cmd_topic),
        ));

        // Soak Mode switch
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "soak_mode",
            "Soak Mode",
            "{{ value_json.soak_mode }}",
            &format!("{}/soak_mode", cmd_topic),
        ));

        // Normal Operation switch
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "normal_operation",
            "Normal Operation",
            "{{ value_json.normal_operation }}",
            &format!("{}/normal_operation", cmd_topic),
        ));

        // Clear Notification switch
        configs.push(Self::make_switch(
            &topics,
            &self.device_id,
            &device_info,
            &origin,
            &state_topic,
            &avail_topic,
            "clear_notification",
            "Clear Notification",
            "{{ value_json.clear_notification }}",
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

        configs
    }

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

        assert!(
            configs.len() >= 27,
            "expected at least 27 discovery configs, got {}",
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

        assert!(
            messages.len() >= 27,
            "expected at least 27 discovery messages, got {}",
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
            assert!(
                v.get("state_topic").is_some(),
                "missing 'state_topic' in {}",
                topic
            );
            assert!(
                v.get("availability_topic").is_some(),
                "missing 'availability_topic' in {}",
                topic
            );
        }
    }

    // --- New toggle item discovery tests ---

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
        // All configs should produce valid JSON
        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));
        }
    }

    #[test]
    fn test_discovery_entity_count_increased() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();
        // Original was 20 entities, now we have:
        // - mister changed from sensor to switch (still 1 entity)
        // - circ_pump changed from sensor to switch (still 1 entity)
        // - light3, light4 added (2 new light entities)
        // - aux1, aux2, soak_mode, normal_operation, clear_notification added (5 new switches)
        // Total: 20 + 7 = 27
        assert!(
            configs.len() >= 27,
            "should have at least 27 entities after adding new toggle items, got {}",
            configs.len()
        );
    }

    // ── JSON escaping tests for discovery configs ─────────────────────

    /// Helper: build discovery configs with all special characters and verify
    /// every payload parses as valid JSON via serde_json::from_str.
    fn builder_with_special_chars() -> DiscoveryBuilder {
        DiscoveryBuilder::new("test_spa")
            .device_name(r#"My "Spa" \Unit\"#)
            .manufacturer("Mfr\nLine2\tTab")
            .device_model(r#"BP\6013"G1""#)
            .sw_version("v1.0\r\nbeta\x07")
    }

    #[test]
    fn test_discovery_special_chars_all_payloads_valid_json() {
        // VAL-PROTO-007: device_name with quotes/backslashes produces valid JSON
        let builder = builder_with_special_chars();
        let configs = builder.build();
        assert!(configs.len() >= 27, "expected at least 27 configs");

        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str).unwrap_or_else(|e| {
                panic!(
                    "Invalid JSON for topic {} with special chars: {}\nPayload: {}",
                    topic, e, json_str
                )
            });
        }
    }

    #[test]
    fn test_discovery_special_chars_device_name_escaped_in_device_block() {
        // VAL-PROTO-007: device_name with quotes/backslashes produces valid JSON
        let builder = builder_with_special_chars();
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            let device = v
                .get("device")
                .unwrap_or_else(|| panic!("Missing 'device' in topic {}", topic));
            // device_name contains quotes and backslashes — must be the original string
            assert_eq!(
                device["name"].as_str(),
                Some(r#"My "Spa" \Unit\"#),
                "device name mismatch in topic {}",
                topic
            );
        }
    }

    #[test]
    fn test_discovery_special_chars_manufacturer_escaped() {
        // VAL-PROTO-008: manufacturer with special chars produces valid JSON
        let builder = builder_with_special_chars();
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            let device = v.get("device").unwrap();
            assert_eq!(
                device["manufacturer"].as_str(),
                Some("Mfr\nLine2\tTab"),
                "manufacturer mismatch in topic {}",
                topic
            );
        }
    }

    #[test]
    fn test_discovery_special_chars_model_escaped() {
        // VAL-PROTO-008: model with special chars produces valid JSON
        let builder = builder_with_special_chars();
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            let device = v.get("device").unwrap();
            assert_eq!(
                device["model"].as_str(),
                Some(r#"BP\6013"G1""#),
                "model mismatch in topic {}",
                topic
            );
        }
    }

    #[test]
    fn test_discovery_special_chars_sw_version_escaped_in_device() {
        // VAL-PROTO-007: sw_version with special chars in device block
        let builder = builder_with_special_chars();
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
            let device = v.get("device").unwrap();
            assert_eq!(
                device["sw_version"].as_str(),
                Some("v1.0\r\nbeta\u{0007}"),
                "device sw_version mismatch in topic {}",
                topic
            );
        }
    }

    #[test]
    fn test_discovery_special_chars_sw_version_escaped_in_origin() {
        // VAL-PROTO-007: sw_version with special chars in origin block
        let builder = builder_with_special_chars();
        let configs = builder.build();

        for (topic, json_str) in &configs {
            let v = parse_json(json_str);
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
    fn test_discovery_special_chars_retain_payloads_valid_json() {
        // Verify retain payloads also produce valid JSON with special chars
        let builder = builder_with_special_chars();
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
    fn test_json_device_block_escapes_quotes() {
        let block = json_device_block("id1", r#"Name "Quoted""#, "Mfr", "Model", "1.0");
        let v = parse_json(&block);
        assert_eq!(v["name"].as_str(), Some(r#"Name "Quoted""#));
    }

    #[test]
    fn test_json_device_block_escapes_backslashes() {
        let block = json_device_block("id1", r#"Path\To\Spa"#, "Mfr", "Model", "1.0");
        let v = parse_json(&block);
        assert_eq!(v["name"].as_str(), Some(r#"Path\To\Spa"#));
    }

    #[test]
    fn test_json_device_block_escapes_control_chars() {
        let block = json_device_block("id1", "Before\x07After", "Mfr", "Model", "1.0");
        let v = parse_json(&block);
        assert_eq!(v["name"].as_str(), Some("Before\u{0007}After"));
    }

    #[test]
    fn test_json_device_block_escapes_newlines_and_tabs() {
        let block = json_device_block("id1", "Line1\nLine2\tTab", "Mfr", "Model", "1.0");
        let v = parse_json(&block);
        assert_eq!(v["name"].as_str(), Some("Line1\nLine2\tTab"));
    }

    #[test]
    fn test_json_device_block_escapes_all_fields() {
        let block = json_device_block(
            "id1",
            r#"Name"Quote"#,
            r#"Mfr\Backslash"#,
            "Model\nNewline",
            "v1.0\tTab",
        );
        let v = parse_json(&block);
        assert_eq!(v["name"].as_str(), Some(r#"Name"Quote"#));
        assert_eq!(v["manufacturer"].as_str(), Some(r#"Mfr\Backslash"#));
        assert_eq!(v["model"].as_str(), Some("Model\nNewline"));
        assert_eq!(v["sw_version"].as_str(), Some("v1.0\tTab"));
    }

    #[test]
    fn test_json_origin_escapes_sw_version() {
        let origin = json_origin("v1.0\nbeta\r\n\x07");
        let v = parse_json(&origin);
        assert_eq!(v["sw_version"].as_str(), Some("v1.0\nbeta\r\n\u{0007}"));
    }

    #[test]
    fn test_json_device_block_null_byte() {
        let block = json_device_block("id1", "Before\x00After", "Mfr", "Model", "1.0");
        let v = parse_json(&block);
        assert_eq!(v["name"].as_str(), Some("Before\u{0000}After"));
    }

    #[test]
    fn test_json_device_block_combined_special_chars() {
        // A single string with all types of special chars
        let name = alloc::format!("A\\B\"C\nD\rE\tF\x01G");
        let block = json_device_block("id1", &name, "Mfr", "Model", "1.0");
        let v = parse_json(&block);
        assert_eq!(
            v["name"].as_str(),
            Some(alloc::format!("A\\B\"C\nD\rE\tF\u{0001}G").as_str())
        );
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
