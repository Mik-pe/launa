//! OTA firmware update support.
//!
//! TODO: Replace with `launa-esp-ota` crate (custom ESP32 OTA using esp-storage directly).
//! The previous `esp-hal-ota` dependency is broken with nightly Rust >=1.90 (concat_idents removed).
//! The new crate will implement partition management, flash writes, and boot marker directly
//! using esp-storage, without the broken third-party dependency.

extern crate alloc;

use alloc::string::String;
use alloc::format;
use launa_ota::{OtaError, OtaUpdate};
use log::{info, warn, error};

pub struct EspOta;

impl EspOta {
    pub fn new() -> Self {
        EspOta
    }

    /// Mark the current firmware as valid. Call this after successful boot
    /// (WiFi + MQTT connected). Prevents auto-rollback.
    pub fn mark_valid(&mut self) -> Result<(), OtaError> {
        info!("OTA: marking firmware valid (stub)");
        // TODO: Implement with launa-esp-ota
        Ok(())
    }
}

impl OtaUpdate for EspOta {
    fn begin(&mut self) -> Result<(), OtaError> {
        warn!("OTA: begin stub -- not yet implemented");
        Err(OtaError::BeginFailed)
    }

    fn write(&mut self, _chunk: &[u8]) -> Result<(), OtaError> {
        Err(OtaError::WriteFailed)
    }

    fn finalize(&mut self) -> Result<(), OtaError> {
        Err(OtaError::FinalizeFailed)
    }

    fn mark_valid(&mut self) -> Result<(), OtaError> {
        self.mark_valid()
    }

    fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
        warn!("OTA: rollback and reboot requested (stub)");
        Err(OtaError::FlashError)
    }
}

/// Perform an OTA update by downloading firmware from the given HTTP URL.
/// Currently stubbed -- will be implemented with launa-esp-ota + embassy-net HTTP.
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
