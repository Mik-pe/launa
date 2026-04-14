//! OTA firmware update using esp-hal-ota.

use launa_ota::{OtaError, OtaUpdate};
use esp_hal_ota::Ota;
use log::{info, warn, error};

pub struct EspOta {
    ota: Ota<esp_storage::FlashStorage>,
}

impl EspOta {
    pub fn new() -> Self {
        let flash = esp_storage::FlashStorage::new();
        let ota = Ota::new(flash);
        EspOta { ota }
    }
}

impl OtaUpdate for EspOta {
    fn begin(&mut self) -> Result<(), OtaError> {
        info!("OTA: beginning update");
        self.ota.begin().map_err(|e| {
            error!("OTA begin failed: {:?}", e);
            OtaError::BeginFailed
        })
    }

    fn write(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
        self.ota.write(chunk).map_err(|e| {
            error!("OTA write failed: {:?}", e);
            OtaError::WriteFailed
        })
    }

    fn finalize(&mut self) -> Result<(), OtaError> {
        info!("OTA: finalizing update");
        self.ota.end().map_err(|e| {
            error!("OTA finalize failed: {:?}", e);
            OtaError::FinalizeFailed
        })
    }

    fn mark_valid(&mut self) -> Result<(), OtaError> {
        info!("OTA: marking firmware valid");
        self.ota.mark_app_valid().map_err(|e| {
            error!("OTA mark_valid failed: {:?}", e);
            OtaError::FlashError
        })
    }

    fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
        warn!("OTA: rollback and reboot requested");
        self.ota.rollback_and_reboot().map_err(|e| {
            error!("OTA rollback failed: {:?}", e);
            OtaError::FlashError
        })
    }
}
