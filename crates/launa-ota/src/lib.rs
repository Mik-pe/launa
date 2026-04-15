//! OTA firmware update support.
//!
//! Provides a trait for OTA operations and a mock implementation for desktop testing.
//! The real ESP32 implementation will use `launa-esp-ota` crate in the `app/` crate.

#![no_std]

#[cfg(any(test, feature = "mock"))]
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

#[derive(Debug, thiserror::Error)]
pub enum OtaError {
    #[error("OTA begin failed")]
    BeginFailed,
    #[error("OTA write failed at byte offset {byte_offset}")]
    WriteFailed { byte_offset: usize },
    #[error("OTA finalize failed")]
    FinalizeFailed,
    #[error("no OTA partition found")]
    NoOtaPartition,
    #[error("invalid firmware")]
    InvalidFirmware,
    #[error("flash error at address {address:#x}")]
    FlashError { address: u32 },
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;

    #[test]
    fn test_ota_error_display() {
        // Every variant must produce a non-empty Display string
        let cases: Vec<String> = vec![
            OtaError::BeginFailed.to_string(),
            OtaError::WriteFailed { byte_offset: 0 }.to_string(),
            OtaError::FinalizeFailed.to_string(),
            OtaError::NoOtaPartition.to_string(),
            OtaError::InvalidFirmware.to_string(),
            OtaError::FlashError { address: 0 }.to_string(),
        ];
        for s in &cases {
            assert!(!s.is_empty(), "OtaError variant Display output is empty");
        }
    }

    #[test]
    fn test_ota_error_write_failed_byte_offset() {
        let err = OtaError::WriteFailed { byte_offset: 42 };
        if let OtaError::WriteFailed { byte_offset } = err {
            assert_eq!(byte_offset, 42);
        } else {
            panic!("Expected WriteFailed variant");
        }
    }

    #[test]
    fn test_ota_error_flash_error_address() {
        let err = OtaError::FlashError { address: 0x9000 };
        if let OtaError::FlashError { address } = err {
            assert_eq!(address, 0x9000);
        } else {
            panic!("Expected FlashError variant");
        }
    }
}

#[cfg(any(test, feature = "mock"))]
pub mod mock {
    use super::{alloc::vec::Vec, OtaError, OtaUpdate};

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
