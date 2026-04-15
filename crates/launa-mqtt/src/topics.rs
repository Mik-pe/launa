/// MQTT topic builder for Launa spa integration.

extern crate alloc;

use alloc::string::String;
use alloc::string::ToString;

const BASE_TOPIC: &str = "launa";

/// Payload published to the availability topic when the device is online.
pub const AVAILABILITY_ONLINE: &str = "online";

/// Payload published to the availability topic when the device is offline.
pub const AVAILABILITY_OFFLINE: &str = "offline";

pub struct TopicBuilder {
    device_id: String,
}

impl TopicBuilder {
    pub fn new(device_id: &str) -> Self {
        TopicBuilder { device_id: device_id.into() }
    }

    pub fn state_topic(&self) -> String {
        alloc::format!("{}/{}/state", BASE_TOPIC, self.device_id)
    }

    pub fn command_topic(&self) -> String {
        alloc::format!("{}/{}/command", BASE_TOPIC, self.device_id)
    }

    pub fn availability_topic(&self) -> String {
        alloc::format!("{}/{}/availability", BASE_TOPIC, self.device_id)
    }

    pub fn discovery_topic(&self, component: &str, object_id: &str) -> String {
        alloc::format!("homeassistant/{}/{}/{}/config", component, self.device_id, object_id)
    }

    pub fn ota_topic(&self) -> String {
        alloc::format!("{}/{}/ota", BASE_TOPIC, self.device_id)
    }

    /// Topic for raw sniffer frames (passive RS-485 monitoring).
    pub fn sniff_topic(&self) -> String {
        alloc::format!("{}/{}/sniff", BASE_TOPIC, self.device_id)
    }

    /// Topic for subscribing to Home Assistant status (online/offline).
    /// Used to re-publish discovery when HA restarts.
    pub fn ha_status_topic(&self) -> String {
        "homeassistant/status".to_string()
    }
}

/// MQTT Last Will and Testament (LWT) configuration.
/// The MQTT broker publishes the offline payload to the availability topic
/// if the device disconnects ungracefully.
pub struct LwtConfig {
    pub topic: String,
    pub payload: &'static str,
    pub qos: u8,
    pub retain: bool,
}

/// Build LWT configuration for a device. This should be set during MQTT connect.
pub fn lwt_config(device_id: &str) -> LwtConfig {
    LwtConfig {
        topic: alloc::format!("{}/{}/availability", BASE_TOPIC, device_id),
        payload: AVAILABILITY_OFFLINE,
        qos: 1,
        retain: true,
    }
}

/// Birth message configuration.
/// Published immediately after successful MQTT connect to announce the device is online.
pub struct BirthConfig {
    pub topic: String,
    pub payload: &'static str,
    pub qos: u8,
    pub retain: bool,
}

/// Build birth message configuration for a device.
pub fn birth_config(device_id: &str) -> BirthConfig {
    BirthConfig {
        topic: alloc::format!("{}/{}/availability", BASE_TOPIC, device_id),
        payload: AVAILABILITY_ONLINE,
        qos: 1,
        retain: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_builder_state() {
        let t = TopicBuilder::new("spa_001");
        assert_eq!(t.state_topic(), "launa/spa_001/state");
    }

    #[test]
    fn test_topic_builder_command() {
        let t = TopicBuilder::new("spa_001");
        assert_eq!(t.command_topic(), "launa/spa_001/command");
    }

    #[test]
    fn test_topic_builder_availability() {
        let t = TopicBuilder::new("spa_001");
        assert_eq!(t.availability_topic(), "launa/spa_001/availability");
    }

    #[test]
    fn test_topic_builder_discovery() {
        let t = TopicBuilder::new("spa_001");
        assert_eq!(
            t.discovery_topic("sensor", "temperature"),
            "homeassistant/sensor/spa_001/temperature/config"
        );
    }

    #[test]
    fn test_topic_builder_ota() {
        let t = TopicBuilder::new("spa_001");
        assert_eq!(t.ota_topic(), "launa/spa_001/ota");
    }

    #[test]
    fn test_topic_builder_sniff() {
        let t = TopicBuilder::new("spa_001");
        assert_eq!(t.sniff_topic(), "launa/spa_001/sniff");
    }

    #[test]
    fn test_topic_builder_ha_status() {
        let t = TopicBuilder::new("spa_001");
        assert_eq!(t.ha_status_topic(), "homeassistant/status");
    }

    #[test]
    fn test_lwt_config() {
        let lwt = lwt_config("spa_001");
        assert_eq!(lwt.topic, "launa/spa_001/availability");
        assert_eq!(lwt.payload, "offline");
        assert_eq!(lwt.qos, 1);
        assert!(lwt.retain);
    }

    #[test]
    fn test_birth_config() {
        let birth = birth_config("spa_001");
        assert_eq!(birth.topic, "launa/spa_001/availability");
        assert_eq!(birth.payload, "online");
        assert_eq!(birth.qos, 1);
        assert!(birth.retain);
    }

    #[test]
    fn test_lwt_and_birth_same_topic() {
        // LWT and birth must publish to the same topic
        let lwt = lwt_config("spa_001");
        let birth = birth_config("spa_001");
        assert_eq!(lwt.topic, birth.topic);
    }

    #[test]
    fn test_availability_constants() {
        assert_eq!(AVAILABILITY_ONLINE, "online");
        assert_eq!(AVAILABILITY_OFFLINE, "offline");
        assert_ne!(AVAILABILITY_ONLINE, AVAILABILITY_OFFLINE);
    }
}
