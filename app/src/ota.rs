//! OTA firmware update support.
//!
//! Uses `launa-esp-ota` crate for real OTA operations backed by `esp-storage::FlashStorage`.
//! The `EspOtaFlash` struct implements the `OtaUpdate` trait from `launa-ota`.

extern crate alloc;

use alloc::string::String;
use launa_esp_ota::{EspOtaFlash, Partition};
use launa_ota::{OtaError, OtaUpdate};
use log::{error, info, warn};

pub type EspOta = EspOtaFlash<esp_storage::FlashStorage<'static>>;

/// Create a new OTA updater. Detects the actual running partition
/// from otadata instead of hardcoding.
pub fn create_ota() -> EspOta {
    let flash = esp_storage::FlashStorage::new();
    let mut temp = EspOtaFlash::new(flash, Partition::Ota0);
    let running = temp.detect_running_partition().unwrap_or(Partition::Ota0);
    let flash = esp_storage::FlashStorage::new();
    EspOtaFlash::new(flash, running)
}

/// Perform an OTA update by downloading firmware from the given HTTP URL.
/// Currently stubbed -- will be implemented with embassy-net HTTP.
pub async fn perform_ota_update(firmware_url: &str) {
    info!("OTA update requested from: {} (not yet implemented)", firmware_url);

    let (host, port, path) = match parse_http_url(firmware_url) {
        Some(v) => v,
        None => {
            error!("OTA: invalid URL: {}", firmware_url);
            return;
        }
    };

    warn!(
        "OTA: HTTP download not yet implemented. URL: {}:{}{}",
        host, port, path
    );
}

/// Simple HTTP URL parser. Returns (host, port, path).
fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let url = url.strip_prefix("http://")?;
    let (host_port, path) = match url.find('/') {
        Some(idx) => (&url[..idx], &url[idx..]),
        None => (url, "/"),
    };

    let (host, port) = match host_port.find(':') {
        Some(idx) => {
            let port: u16 = host_port[idx + 1..].parse().ok()?;
            (String::from(&host_port[..idx]), port)
        }
        None => (String::from(host_port), 80),
    };

    Some((host, port, String::from(path)))
}
