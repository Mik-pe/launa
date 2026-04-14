//! NVS-based configuration storage.
//!
//! Stores WiFi credentials, MQTT broker address, and device ID in non-volatile storage.

use anyhow::{bail, Context, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspNvs, NvsPartitionId};
use log::info;

const NAMESPACE: &str = "launa";
const KEY_WIFI_SSID: &str = "wifi_ssid";
const KEY_WIFI_PASS: &str = "wifi_pass";
const KEY_MQTT_HOST: &str = "mqtt_host";
const KEY_MQTT_PORT: &str = "mqtt_port";
const KEY_MQTT_USER: &str = "mqtt_user";
const KEY_MQTT_PASS: &str = "mqtt_pass";
const KEY_DEVICE_ID: &str = "device_id";
const KEY_RS485_TX_PIN: &str = "rs485_tx";
const KEY_RS485_RX_PIN: &str = "rs485_rx";
const KEY_RS485_DE_PIN: &str = "rs485_de";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_user: String,
    pub mqtt_password: String,
    pub device_id: String,
    pub rs485_tx_pin: i32,
    pub rs485_rx_pin: i32,
    pub rs485_de_pin: i32,
}

impl AppConfig {
    pub fn load(nvs: &EspNvs<EspDefaultNvs>) -> Result<Self> {
        let wifi_ssid = nvs_get_str(nvs, KEY_WIFI_SSID)
            .context("WiFi SSID not configured")?;
        let wifi_password = nvs_get_str(nvs, KEY_WIFI_PASS)
            .context("WiFi password not configured")?;
        let mqtt_host = nvs_get_str(nvs, KEY_MQTT_HOST)
            .context("MQTT host not configured")?;
        let mqtt_port = nvs.get_i32(KEY_MQTT_PORT)
            .ok()
            .unwrap_or(1883) as u16;
        let mqtt_user = nvs_get_str(nvs, KEY_MQTT_USER).unwrap_or_default();
        let mqtt_password = nvs_get_str(nvs, KEY_MQTT_PASS).unwrap_or_default();
        let device_id = nvs_get_str(nvs, KEY_DEVICE_ID)
            .unwrap_or_else(|_| "launa_spa".to_string());
        let rs485_tx_pin = nvs.get_i32(KEY_RS485_TX_PIN).ok().unwrap_or(17);
        let rs485_rx_pin = nvs.get_i32(KEY_RS485_RX_PIN).ok().unwrap_or(16);
        let rs485_de_pin = nvs.get_i32(KEY_RS485_DE_PIN).ok().unwrap_or(4);

        info!("Config loaded: ssid={}, mqtt={}:{} device={}", wifi_ssid, mqtt_host, mqtt_port, device_id);

        Ok(AppConfig {
            wifi_ssid,
            wifi_password,
            mqtt_host,
            mqtt_port,
            mqtt_user,
            mqtt_password,
            device_id,
            rs485_tx_pin,
            rs485_rx_pin,
            rs485_de_pin,
        })
    }

    pub fn save(&self, nvs: &EspNvs<EspDefaultNvs>) -> Result<()> {
        nvs_put_str(nvs, KEY_WIFI_SSID, &self.wifi_ssid)?;
        nvs_put_str(nvs, KEY_WIFI_PASS, &self.wifi_password)?;
        nvs_put_str(nvs, KEY_MQTT_HOST, &self.mqtt_host)?;
        nvs.put_i32(KEY_MQTT_PORT, self.mqtt_port as i32)?;
        nvs_put_str(nvs, KEY_MQTT_USER, &self.mqtt_user)?;
        nvs_put_str(nvs, KEY_MQTT_PASS, &self.mqtt_password)?;
        nvs_put_str(nvs, KEY_DEVICE_ID, &self.device_id)?;
        nvs.put_i32(KEY_RS485_TX_PIN, self.rs485_tx_pin)?;
        nvs.put_i32(KEY_RS485_RX_PIN, self.rs485_rx_pin)?;
        nvs.put_i32(KEY_RS485_DE_PIN, self.rs485_de_pin)?;
        info!("Config saved to NVS");
        Ok(())
    }

    pub fn open_nvs() -> Result<EspNvs<EspDefaultNvs>> {
        EspNvs::new(NvsPartitionId::Default, NAMESPACE, true)
            .context("Failed to open NVS namespace")
    }
}

fn nvs_get_str(nvs: &EspNvs<EspDefaultNvs>, key: &str) -> Result<String> {
    let len = nvs.str_len(key).context("NVS key not found")?;
    let mut buf = vec![0u8; len + 1];
    nvs.get_str(key, &mut buf)
        .context("Failed to read NVS string")?;
    Ok(String::from_utf8_lossy(&buf[..len]).to_string())
}

fn nvs_put_str(nvs: &EspNvs<EspDefaultNvs>, key: &str, value: &str) -> Result<()> {
    nvs.set_str(key, value)
        .map_err(|e| anyhow::anyhow!("Failed to write NVS key {}: {:?}", key, e))
}

pub fn load_or_default(nvs: &EspNvs<EspDefaultNvs>) -> AppConfig {
    match AppConfig::load(nvs) {
        Ok(config) => config,
        Err(e) => {
            log::warn!("Config load failed, using defaults: {:?}", e);
            AppConfig {
                wifi_ssid: "YOUR_WIFI_SSID".to_string(),
                wifi_password: "YOUR_WIFI_PASSWORD".to_string(),
                mqtt_host: "192.168.1.100".to_string(),
                mqtt_port: 1883,
                mqtt_user: String::new(),
                mqtt_password: String::new(),
                device_id: "launa_spa".to_string(),
                rs485_tx_pin: 17,
                rs485_rx_pin: 16,
                rs485_de_pin: 4,
            }
        }
    }
}
