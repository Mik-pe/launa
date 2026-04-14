//! NVS-based configuration storage using esp-nvs.

extern crate alloc;

use alloc::string::String;
use alloc::format;
use log::{info, warn};

const NAMESPACE: &str = "launa";

const KEY_WIFI_SSID: &str = "wifi_ssid";
const KEY_WIFI_PASS: &str = "wifi_pass";
const KEY_MQTT_HOST: &str = "mqtt_host";
const KEY_MQTT_PORT: &str = "mqtt_port";
const KEY_MQTT_USER: &str = "mqtt_user";
const KEY_MQTT_PASS: &str = "mqtt_pass";
const KEY_DEVICE_ID: &str = "device_id";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_user: String,
    pub mqtt_password: String,
    pub device_id: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            wifi_ssid: String::from("YOUR_WIFI_SSID"),
            wifi_password: String::from("YOUR_WIFI_PASSWORD"),
            mqtt_host: String::from("192.168.1.100"),
            mqtt_port: 1883,
            mqtt_user: String::new(),
            mqtt_password: String::new(),
            device_id: String::from("launa_spa"),
        }
    }
}

impl AppConfig {
    pub fn load(nvs: &mut esp_nvs::Nvs) -> Self {
        let wifi_ssid = nvs_get_str(nvs, KEY_WIFI_SSID)
            .unwrap_or_else(|| String::from("YOUR_WIFI_SSID"));
        let wifi_password = nvs_get_str(nvs, KEY_WIFI_PASS)
            .unwrap_or_else(|| String::from("YOUR_WIFI_PASSWORD"));
        let mqtt_host = nvs_get_str(nvs, KEY_MQTT_HOST)
            .unwrap_or_else(|| String::from("192.168.1.100"));
        let mqtt_port = nvs.get_u16(KEY_MQTT_PORT).unwrap_or(1883);
        let mqtt_user = nvs_get_str(nvs, KEY_MQTT_USER)
            .unwrap_or_else(|| String::new());
        let mqtt_password = nvs_get_str(nvs, KEY_MQTT_PASS)
            .unwrap_or_else(|| String::new());
        let device_id = nvs_get_str(nvs, KEY_DEVICE_ID)
            .unwrap_or_else(|| String::from("launa_spa"));

        info!(
            "Config loaded: ssid={} mqtt={}:{} device={}",
            wifi_ssid, mqtt_host, mqtt_port, device_id
        );

        AppConfig {
            wifi_ssid,
            wifi_password,
            mqtt_host,
            mqtt_port,
            mqtt_user,
            mqtt_password,
            device_id,
        }
    }

    pub fn save(&self, nvs: &mut esp_nvs::Nvs) {
        let _ = nvs.set_str(KEY_WIFI_SSID, &self.wifi_ssid);
        let _ = nvs.set_str(KEY_WIFI_PASS, &self.wifi_password);
        let _ = nvs.set_str(KEY_MQTT_HOST, &self.mqtt_host);
        let _ = nvs.set_u16(KEY_MQTT_PORT, self.mqtt_port);
        let _ = nvs.set_str(KEY_MQTT_USER, &self.mqtt_user);
        let _ = nvs.set_str(KEY_MQTT_PASS, &self.mqtt_password);
        let _ = nvs.set_str(KEY_DEVICE_ID, &self.device_id);
        info!("Config saved to NVS");
    }

    pub fn open_nvs(flash: esp_storage::FlashStorage) -> esp_nvs::Nvs {
        esp_nvs::Nvs::new(flash, NAMESPACE).unwrap_or_else(|_| {
            warn!("Failed to open NVS namespace 'launa', using defaults");
            panic!("NVS init failed")
        })
    }
}

fn nvs_get_str(nvs: &esp_nvs::Nvs, key: &str) -> Option<String> {
    // esp-nvs returns Option<&str> from get_str
    nvs.get_str(key).map(|s| String::from(s))
}
