/// MQTT topic builder for Launa spa integration.

extern crate alloc;

use alloc::string::String;

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
}
