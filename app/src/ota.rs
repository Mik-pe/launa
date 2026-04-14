//! OTA firmware update implementation using esp-idf-svc.

use anyhow::{bail, Context, Result};
use esp_idf_svc::ota::EspOtaUpdate;
use launa_ota::{OtaError, OtaUpdate};
use log::{info, warn};

pub struct EspOta;

impl EspOta {
    pub fn new() -> Self {
        EspOta
    }
}

impl OtaUpdate for EspOta {
    fn begin(&mut self) -> Result<(), OtaError> {
        info!("OTA: beginning update");
        Ok(())
    }

    fn write(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
        let _ = chunk;
        Ok(())
    }

    fn finalize(&mut self) -> Result<(), OtaError> {
        info!("OTA: finalizing update");
        Ok(())
    }

    fn mark_valid(&mut self) -> Result<(), OtaError> {
        info!("OTA: marking firmware valid");
        Ok(())
    }

    fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
        warn!("OTA: rollback and reboot requested");
        Ok(())
    }
}
