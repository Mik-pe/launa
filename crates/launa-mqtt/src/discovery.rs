//! Home Assistant MQTT auto-discovery message generation.
//!
//! Generates JSON discovery payloads for 20 HA entities using `alloc` only
//! (no serde_json dependency), so this works in `no_std` environments.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

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
        for i in 1..=2 {
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

        // Circulation Pump sensor (read-only \u2014 protocol doesn't support toggling)
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("sensor", "circ_pump"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Circulation Pump","unique_id":"{}_circ_pump","state_topic":"{}","value_template":"{{{{ value_json.circ_pump }}}}","availability_topic":"{}"}}"#,
                device_info, origin, self.device_id, state_topic, avail_topic
            ),
            retain: false,
        });

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

        // Mister sensor (read-only \u2014 protocol doesn't support toggling)
        configs.push(DiscoveryMessage {
            topic: topics.discovery_topic("sensor", "mister"),
            payload: format!(
                r#"{{"device":{},"origin":{},"name":"Mister","unique_id":"{}_mister","state_topic":"{}","value_template":"{{{{ value_json.mister }}}}","availability_topic":"{}"}}"#,
                device_info, origin, self.device_id, state_topic, avail_topic
            ),
            retain: false,
        });

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
        device_id, name, manufacturer, model, sw_version
    )
}

/// Build the `"origin"` JSON block.
fn json_origin(sw_version: &str) -> String {
    format!(
        r#"{{"name":"launa-firmware","sw_version":"{}"}}"#,
        sw_version
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

        assert_eq!(configs.len(), 20);

        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));
        }
    }

    #[test]
    fn test_build_with_retain() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let messages = builder.build_with_retain();

        assert_eq!(messages.len(), 20);
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
}
