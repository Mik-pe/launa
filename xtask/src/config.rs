use anyhow::{bail, Context};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub wifi: WifiConfig,
    pub mqtt: MqttConfig,
    pub device: DeviceConfig,
    #[serde(default)]
    pub ota: OtaConfig,
}

#[derive(Debug, Deserialize)]
pub struct WifiConfig {
    pub ssid: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct MqttConfig {
    pub host: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct DeviceConfig {
    pub id: String,
    pub serial_port: String,
}

#[derive(Debug, Deserialize)]
pub struct OtaConfig {
    #[serde(default = "default_ota_port")]
    pub serve_port: u16,
}

impl Default for OtaConfig {
    fn default() -> Self {
        OtaConfig { serve_port: 8080 }
    }
}

fn default_mqtt_port() -> u16 {
    1883
}

fn default_ota_port() -> u16 {
    8080
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must be inside project root")
        .to_path_buf()
}

pub fn config_path() -> PathBuf {
    project_root().join("launa.toml")
}

pub fn load() -> anyhow::Result<Config> {
    let path = config_path();
    if !path.exists() {
        bail!(
            "Config file not found: {}\nCopy launa.example.toml to launa.toml and fill in your values.",
            path.display()
        );
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let config: Config =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;

    config.validate()?;
    Ok(config)
}

impl Config {
    fn validate(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();

        if self.wifi.ssid.is_empty() || self.wifi.ssid == "YourWiFiName" {
            errors.push("wifi.ssid must be set (not the placeholder)");
        }
        if self.wifi.password.is_empty() || self.wifi.password == "YourWiFiPassword" {
            errors.push("wifi.password must be set (not the placeholder)");
        }
        if self.mqtt.host.is_empty() || self.mqtt.host == "192.168.1.100" {
            errors.push("mqtt.host must be set to your broker address");
        }
        if self.device.id.is_empty() {
            errors.push("device.id must be set");
        }
        if self.device.serial_port.is_empty() {
            errors.push("device.serial_port must be set (e.g. COM3 or /dev/ttyUSB0)");
        }

        if !errors.is_empty() {
            bail!(
                "Config validation failed:\n  - {}\nEdit launa.toml to fix these issues.",
                errors.join("\n  - ")
            );
        }

        Ok(())
    }
}
