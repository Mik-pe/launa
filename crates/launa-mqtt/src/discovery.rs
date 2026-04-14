//! Home Assistant MQTT auto-discovery message generation.

use std::string::String;
use std::vec::Vec;
use serde_json::json;
use crate::topics::TopicBuilder;

pub struct DiscoveryBuilder {
    device_id: String,
    device_name: String,
    device_model: String,
    sw_version: String,
    manufacturer: String,
}

impl DiscoveryBuilder {
    pub fn new(device_id: &str) -> Self {
        DiscoveryBuilder {
            device_id: device_id.to_string(),
            device_name: "Launa Spa".to_string(),
            device_model: "BP6013G1".to_string(),
            sw_version: env!("CARGO_PKG_VERSION").to_string(),
            manufacturer: "Launa".to_string(),
        }
    }

    pub fn device_name(mut self, name: &str) -> Self {
        self.device_name = name.to_string();
        self
    }

    pub fn device_model(mut self, model: &str) -> Self {
        self.device_model = model.to_string();
        self
    }

    /// Generate all discovery config payloads for the spa device.
    /// Returns a vec of (topic, json_payload) pairs.
    pub fn build(&self) -> Vec<(String, String)> {
        let topics = TopicBuilder::new(&self.device_id);
        let mut configs = Vec::new();

        let device_info = json!({
            "identifiers": [self.device_id],
            "name": self.device_name,
            "manufacturer": self.manufacturer,
            "model": self.device_model,
            "sw_version": self.sw_version,
        });

        let origin = json!({
            "name": "launa-firmware",
            "sw_version": self.sw_version,
        });

        let state_topic = topics.state_topic();
        let avail_topic = topics.availability_topic();
        let cmd_topic = topics.command_topic();

        // Temperature sensor
        configs.push(Self::make_sensor(
            &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
            "temperature", "Water Temperature", "temperature", "°F",
            "{{ value_json.current_temp }}",
        ));

        // Set temperature number
        configs.push((
            topics.discovery_topic("number", "set_temperature"),
            json!({
                "device": device_info,
                "origin": origin,
                "name": "Set Temperature",
                "unique_id": format!("{}_set_temp", self.device_id),
                "device_class": "temperature",
                "unit_of_measurement": "°F",
                "min": 50,
                "max": 104,
                "step": 1,
                "state_topic": state_topic,
                "command_topic": format!("{}/set_temperature", cmd_topic),
                "value_template": "{{ value_json.set_temp }}",
                "availability_topic": avail_topic,
            }).to_string(),
        ));

        // Heating state binary sensor
        configs.push(Self::make_binary_sensor(
            &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
            "heating", "Heating", "heat",
            "{{ value_json.is_heating }}",
        ));

        // Pump switches
        for i in 1..=3 {
            configs.push(Self::make_switch(
                &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
                &format!("pump{}", i), &format!("Pump {}", i),
                &format!("{{{{ value_json.pump{}_on }}}}", i),
                &format!("{}/pump{}", cmd_topic, i),
            ));
        }

        // Light
        configs.push((
            topics.discovery_topic("light", "light1"),
            json!({
                "device": device_info,
                "origin": origin,
                "name": "Spa Light",
                "unique_id": format!("{}_light1", self.device_id),
                "state_topic": state_topic,
                "command_topic": format!("{}/light1", cmd_topic),
                "value_template": "{{ value_json.light1 }}",
                "payload_on": "true",
                "payload_off": "false",
                "availability_topic": avail_topic,
            }).to_string(),
        ));

        // Blower fan
        configs.push((
            topics.discovery_topic("fan", "blower"),
            json!({
                "device": device_info,
                "origin": origin,
                "name": "Blower",
                "unique_id": format!("{}_blower", self.device_id),
                "state_topic": state_topic,
                "command_topic": format!("{}/blower", cmd_topic),
                "payload_on": "true",
                "payload_off": "false",
                "availability_topic": avail_topic,
            }).to_string(),
        ));

        // Heat Mode select
        configs.push(Self::make_select(
            &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
            "heat_mode", "Heat Mode",
            json!(["ready", "rest", "ready_in_rest"]),
            "{{ value_json.heating_mode }}",
            &format!("{}/heat_mode", cmd_topic),
        ));

        // Circulation Pump switch
        configs.push(Self::make_switch(
            &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
            "circ_pump", "Circulation Pump",
            "{{ value_json.circ_pump }}",
            &format!("{}/circ_pump", cmd_topic),
        ));

        // Temperature Range select
        configs.push(Self::make_select(
            &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
            "temp_range", "Temperature Range",
            json!(["high", "low"]),
            "{{ value_json.temp_range }}",
            &format!("{}/temp_range", cmd_topic),
        ));

        // Hold Mode switch
        configs.push(Self::make_switch(
            &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
            "hold_mode", "Hold Mode",
            "{{ value_json.hold_mode }}",
            &format!("{}/hold_mode", cmd_topic),
        ));

        // Mister switch
        configs.push(Self::make_switch(
            &topics, &self.device_id, &device_info, &origin, &state_topic, &avail_topic,
            "mister", "Mister",
            "{{ value_json.mister }}",
            &format!("{}/mister", cmd_topic),
        ));

        // Fault sensor
        configs.push((
            topics.discovery_topic("sensor", "fault"),
            json!({
                "device": device_info,
                "origin": origin,
                "name": "Last Fault",
                "unique_id": format!("{}_fault", self.device_id),
                "state_topic": state_topic,
                "value_template": "{{ value_json.last_fault }}",
                "availability_topic": avail_topic,
            }).to_string(),
        ));

        configs
    }

    fn make_sensor(
        topics: &TopicBuilder,
        device_id: &str,
        device: &serde_json::Value,
        origin: &serde_json::Value,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        device_class: &str,
        unit: &str,
        value_template: &str,
    ) -> (String, String) {
        (
            topics.discovery_topic("sensor", object_id),
            json!({
                "device": device,
                "origin": origin,
                "name": name,
                "unique_id": format!("{}_{}", device_id, object_id),
                "device_class": device_class,
                "unit_of_measurement": unit,
                "state_topic": state_topic,
                "value_template": value_template,
                "availability_topic": avail_topic,
            }).to_string(),
        )
    }

    fn make_binary_sensor(
        topics: &TopicBuilder,
        device_id: &str,
        device: &serde_json::Value,
        origin: &serde_json::Value,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        device_class: &str,
        value_template: &str,
    ) -> (String, String) {
        (
            topics.discovery_topic("binary_sensor", object_id),
            json!({
                "device": device,
                "origin": origin,
                "name": name,
                "unique_id": format!("{}_{}", device_id, object_id),
                "device_class": device_class,
                "state_topic": state_topic,
                "value_template": value_template,
                "payload_on": "true",
                "payload_off": "false",
                "availability_topic": avail_topic,
            }).to_string(),
        )
    }

    fn make_switch(
        topics: &TopicBuilder,
        device_id: &str,
        device: &serde_json::Value,
        origin: &serde_json::Value,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        value_template: &str,
        command_topic: &str,
    ) -> (String, String) {
        (
            topics.discovery_topic("switch", object_id),
            json!({
                "device": device,
                "origin": origin,
                "name": name,
                "unique_id": format!("{}_{}", device_id, object_id),
                "state_topic": state_topic,
                "command_topic": command_topic,
                "value_template": value_template,
                "payload_on": "true",
                "payload_off": "false",
                "availability_topic": avail_topic,
            }).to_string(),
        )
    }

    fn make_select(
        topics: &TopicBuilder,
        device_id: &str,
        device: &serde_json::Value,
        origin: &serde_json::Value,
        state_topic: &str,
        avail_topic: &str,
        object_id: &str,
        name: &str,
        options: serde_json::Value,
        value_template: &str,
        command_topic: &str,
    ) -> (String, String) {
        (
            topics.discovery_topic("select", object_id),
            json!({
                "device": device,
                "origin": origin,
                "name": name,
                "unique_id": format!("{}_{}", device_id, object_id),
                "state_topic": state_topic,
                "command_topic": command_topic,
                "value_template": value_template,
                "options": options,
                "availability_topic": avail_topic,
            }).to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_generates_valid_json() {
        let builder = DiscoveryBuilder::new("test_spa_001");
        let configs = builder.build();

        assert_eq!(configs.len(), 14);

        for (topic, json_str) in &configs {
            let _: serde_json::Value = serde_json::from_str(json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON for topic {}: {}", topic, e));
        }
    }
}
