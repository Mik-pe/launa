//! NVS-based configuration storage using esp-nvs.

extern crate alloc;

use alloc::string::String;
use esp_hal::aes::Aes;
use esp_hal::rng::Rng;
use log::{error, info, warn};

use crate::crypto;

const NAMESPACE: &str = "launa";

const KEY_WIFI_SSID: &str = "wifi_ssid";
const KEY_WIFI_PASS: &str = "wifi_pass";
const KEY_MQTT_HOST: &str = "mqtt_host";
const KEY_MQTT_PORT: &str = "mqtt_port";
const KEY_MQTT_USER: &str = "mqtt_user";
const KEY_MQTT_PASS: &str = "mqtt_pass";
const KEY_DEVICE_ID: &str = "device_id";
const KEY_SELF_TEST: &str = "self_test";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_user: String,
    pub mqtt_password: String,
    pub device_id: String,
    pub self_test: bool,
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
            self_test: false,
        }
    }
}

impl AppConfig {
    /// Placeholder SSID that indicates no valid config has been flashed.
    const PLACEHOLDER_SSID: &str = "YOUR_WIFI_SSID";
    /// Placeholder WiFi password that indicates no valid config has been flashed.
    const PLACEHOLDER_WIFI_PASS: &str = "YOUR_WIFI_PASSWORD";

    pub fn load(
        nvs: &mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>,
        aes: &mut Aes<'_>,
        rng: &mut Rng,
    ) -> Self {
        let ns = esp_nvs::Key::from_str(NAMESPACE);

        let wifi_ssid = nvs_get_str(nvs, &ns, KEY_WIFI_SSID)
            .unwrap_or_else(|| String::from(Self::PLACEHOLDER_SSID));
        let wifi_password = nvs_get_str(nvs, &ns, KEY_WIFI_PASS)
            .map(|v| crypto::maybe_decrypt(&v, aes, rng))
            .unwrap_or_else(|| String::from(Self::PLACEHOLDER_WIFI_PASS));
        let mqtt_host = nvs_get_str(nvs, &ns, KEY_MQTT_HOST)
            .unwrap_or_else(|| String::from("192.168.1.100"));
        let mqtt_port = nvs.get::<u16>(&ns, &esp_nvs::Key::from_str(KEY_MQTT_PORT))
            .unwrap_or(1883);
        let mqtt_user = nvs_get_str(nvs, &ns, KEY_MQTT_USER)
            .unwrap_or_else(|| String::new());
        let mqtt_password = nvs_get_str(nvs, &ns, KEY_MQTT_PASS)
            .map(|v| crypto::maybe_decrypt(&v, aes, rng))
            .unwrap_or_else(|| String::new());
        let device_id = nvs_get_str(nvs, &ns, KEY_DEVICE_ID)
            .unwrap_or_else(|| String::from("launa_spa"));
        let self_test = nvs.get::<bool>(&ns, &esp_nvs::Key::from_str(KEY_SELF_TEST))
            .unwrap_or(false);

        let has_placeholder_creds = wifi_ssid == Self::PLACEHOLDER_SSID
            || wifi_password == Self::PLACEHOLDER_WIFI_PASS;

        if has_placeholder_creds {
            error!(
                "FATAL: No valid config found in NVS. \
                 Use 'cargo xtask config-flash' to write configuration."
            );
        }

        info!(
            "Config loaded: ssid=<{} chars> mqtt={}:{} device={}",
            wifi_ssid.len(),
            mqtt_host, mqtt_port, device_id
        );

        AppConfig {
            wifi_ssid,
            wifi_password,
            mqtt_host,
            mqtt_port,
            mqtt_user,
            mqtt_password,
            device_id,
            self_test,
        }
    }

    pub fn save(
        &self,
        nvs: &mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>,
        aes: &mut Aes<'_>,
        rng: &mut Rng,
    ) {
        let ns = esp_nvs::Key::from_str(NAMESPACE);
        nvs_set(nvs, &ns, KEY_WIFI_SSID, self.wifi_ssid.as_str());
        nvs_set(nvs, &ns, KEY_WIFI_PASS, crypto::encrypt(&self.wifi_password, aes, rng).as_str());
        nvs_set(nvs, &ns, KEY_MQTT_HOST, self.mqtt_host.as_str());
        nvs_set(nvs, &ns, KEY_MQTT_PORT, self.mqtt_port);
        nvs_set(nvs, &ns, KEY_MQTT_USER, self.mqtt_user.as_str());
        nvs_set(nvs, &ns, KEY_MQTT_PASS, crypto::encrypt(&self.mqtt_password, aes, rng).as_str());
        nvs_set(nvs, &ns, KEY_DEVICE_ID, self.device_id.as_str());
        nvs_set(nvs, &ns, KEY_SELF_TEST, self.self_test);
        info!("Config saved to NVS");
    }

    /// Open the NVS partition using the given flash peripheral.
    /// The default NVS partition on ESP32 starts at offset 0x9000, size 0x6000 (24 KiB).
    /// These must match the partition table in app/partitions.csv.
    ///
    /// Returns `Some(Nvs)` on success. Returns `None` on failure (e.g. corrupted
    /// NVS partition) after logging a warning — the caller should fall back to
    /// `AppConfig::default()`.
    ///
    /// Note: the flash peripheral is consumed by `FlashStorage::new()` regardless
    /// of whether NVS init succeeds. It is not recoverable on failure.
    pub fn open_nvs(
        flash: esp_hal::peripherals::FLASH<'static>,
    ) -> Option<esp_nvs::Nvs<esp_storage::FlashStorage<'static>>> {
        let flash_storage = esp_storage::FlashStorage::new(flash);
        match esp_nvs::Nvs::new(0x9000, 0x6000, flash_storage) {
            Ok(nvs) => Some(nvs),
            Err(e) => {
                warn!("Failed to open NVS partition: {:?}, falling back to default config", e);
                None
            }
        }
    }
}

fn nvs_get_str(
    nvs: &mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>,
    namespace: &esp_nvs::Key,
    key: &str,
) -> Option<String> {
    nvs.get::<String>(namespace, &esp_nvs::Key::from_str(key)).ok()
}

/// Write a value to NVS, logging a warning on failure.
fn nvs_set<R>(
    nvs: &mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>,
    namespace: &esp_nvs::Key,
    key: &str,
    value: R,
) where
    esp_nvs::Nvs<esp_storage::FlashStorage<'static>>: esp_nvs::Set<R>,
{
    if let Err(e) = nvs.set(namespace, &esp_nvs::Key::from_str(key), value) {
        warn!("NVS write failed for key '{}': {:?}", key, e);
    }
}
