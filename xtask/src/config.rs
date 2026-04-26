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
        OtaConfig {
            serve_port: 8081,
        }
    }
}

fn default_mqtt_port() -> u16 {
    1883
}

fn default_ota_port() -> u16 {
    8081
}

pub fn config_path() -> PathBuf {
    crate::util::project_root().join("launa.toml")
}

pub fn load() -> anyhow::Result<Config> {
    load_inner(true)
}

/// Load config without checking that the serial port is physically present.
/// Use this for commands that operate over WiFi/MQTT (e.g. ota-flash).
pub fn load_without_serial_port_check() -> anyhow::Result<Config> {
    load_inner(false)
}

fn load_inner(check_serial_port: bool) -> anyhow::Result<Config> {
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

    config.validate(check_serial_port)?;
    Ok(config)
}

/// Validate device.id format: alphanumeric + underscore, 1-64 chars.
/// Returns Ok(()) if valid, Err with description otherwise.
pub fn validate_device_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() {
        bail!("device.id must be set");
    }
    if id.len() > 64 {
        bail!("device.id must be at most 64 characters");
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("device.id must contain only alphanumeric characters and underscores");
    }
    Ok(())
}

/// Validate MQTT port is in range 1-65535.
pub fn validate_mqtt_port(port: u16) -> anyhow::Result<()> {
    if port == 0 {
        bail!("mqtt.port must be between 1 and 65535");
    }
    Ok(())
}

impl Config {
    fn validate(&self, check_serial_port: bool) -> anyhow::Result<()> {
        let mut errors: Vec<String> = Vec::new();

        if self.wifi.ssid.is_empty() || self.wifi.ssid == "YourWiFiName" {
            errors.push("wifi.ssid must be set (not the placeholder)".to_string());
        }
        if self.wifi.password.is_empty() || self.wifi.password == "YourWiFiPassword" {
            errors.push("wifi.password must be set (not the placeholder)".to_string());
        }
        if self.mqtt.host.is_empty() || self.mqtt.host == "192.168.1.100" {
            errors.push("mqtt.host must be set to your broker address".to_string());
        }
        if let Err(e) = validate_device_id(&self.device.id) {
            errors.push(e.to_string());
        }
        if let Err(e) = validate_mqtt_port(self.mqtt.port) {
            errors.push(e.to_string());
        }

        // Check serial port existence (only if explicitly configured; auto-detect skips this)
        if check_serial_port && !self.device.serial_port.is_empty() {
            match serialport::available_ports() {
                Ok(ports) => {
                    let port_names: Vec<&str> =
                        ports.iter().map(|p| p.port_name.as_str()).collect();
                    if !port_names.contains(&self.device.serial_port.as_str()) {
                        errors.push(format!(
                            "device.serial_port '{}' not found. Available ports: {}",
                            self.device.serial_port,
                            if port_names.is_empty() {
                                "none".to_string()
                            } else {
                                port_names.join(", ")
                            }
                        ));
                    }
                }
                Err(_) => {
                    // Cannot enumerate ports (e.g. CI environment); skip check
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_device_id_valid() {
        assert!(validate_device_id("spa1").is_ok());
        assert!(validate_device_id("my_spa_01").is_ok());
        assert!(validate_device_id("A").is_ok());
        assert!(validate_device_id("_underscore").is_ok());
        assert!(validate_device_id("UPPERCASE").is_ok());
        assert!(validate_device_id("12345").is_ok());
    }

    #[test]
    fn test_validate_device_id_empty() {
        assert!(validate_device_id("").is_err());
    }

    #[test]
    fn test_validate_device_id_too_long() {
        let long_id = "a".repeat(65);
        assert!(validate_device_id(&long_id).is_err());
        // Exactly 64 chars is ok
        let max_id = "a".repeat(64);
        assert!(validate_device_id(&max_id).is_ok());
    }

    #[test]
    fn test_validate_device_id_invalid_chars() {
        assert!(validate_device_id("spa-1").is_err()); // hyphen
        assert!(validate_device_id("spa.1").is_err()); // dot
        assert!(validate_device_id("spa 1").is_err()); // space
        assert!(validate_device_id("spa/1").is_err()); // slash
        assert!(validate_device_id("spa@1").is_err()); // at sign
    }

    #[test]
    fn test_validate_mqtt_port_valid() {
        assert!(validate_mqtt_port(1).is_ok());
        assert!(validate_mqtt_port(1883).is_ok());
        assert!(validate_mqtt_port(65535).is_ok());
    }

    #[test]
    fn test_validate_mqtt_port_zero() {
        assert!(validate_mqtt_port(0).is_err());
    }
}
