//! OTA firmware update support.
//!
//! Provides a trait for OTA operations and a mock implementation for desktop testing.
//! The real ESP32 implementation will use `esp-ota` crate in the `app/` crate.

extern crate alloc;

pub trait OtaUpdate {
    /// Begin an OTA update, erasing the target partition.
    fn begin(&mut self) -> Result<(), OtaError>;
    /// Write a chunk of firmware data.
    fn write(&mut self, chunk: &[u8]) -> Result<(), OtaError>;
    /// Finalize the update and set the boot partition.
    fn finalize(&mut self) -> Result<(), OtaError>;
    /// Mark the current firmware as valid (prevents rollback on next boot).
    fn mark_valid(&mut self) -> Result<(), OtaError>;
    /// Rollback to the previous firmware and reboot.
    fn rollback_and_reboot(&mut self) -> Result<(), OtaError>;
}

#[derive(Debug)]
pub enum OtaError {
    BeginFailed,
    WriteFailed,
    FinalizeFailed,
    NoOtaPartition,
    InvalidFirmware,
    FlashError,
}

#[cfg(any(test, feature = "mock"))]
pub mod mock {
    use super::{OtaUpdate, OtaError, alloc::vec::Vec};

    /// Mock OTA updater for desktop testing.
    pub struct MockOta {
        pub firmware_data: Vec<u8>,
        pub finalized: bool,
        pub valid: bool,
        pub rolled_back: bool,
    }

    impl MockOta {
        pub fn new() -> Self {
            MockOta {
                firmware_data: Vec::new(),
                finalized: false,
                valid: false,
                rolled_back: false,
            }
        }
    }

    impl OtaUpdate for MockOta {
        fn begin(&mut self) -> Result<(), OtaError> {
            self.firmware_data.clear();
            self.finalized = false;
            Ok(())
        }

        fn write(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
            self.firmware_data.extend_from_slice(chunk);
            Ok(())
        }

        fn finalize(&mut self) -> Result<(), OtaError> {
            self.finalized = true;
            Ok(())
        }

        fn mark_valid(&mut self) -> Result<(), OtaError> {
            self.valid = true;
            Ok(())
        }

        fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
            self.rolled_back = true;
            Ok(())
        }
    }
}
