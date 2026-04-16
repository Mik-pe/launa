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
    /// Verify the CRC32 of all written firmware data matches the expected value.
    ///
    /// Implementations that track a running CRC should compare it against
    /// `expected_crc` and return `Err(OtaError::HashMismatch)` on mismatch.
    /// The default implementation is a no-op (always succeeds).
    fn verify_hash(&mut self, _expected_crc: u32) -> Result<(), OtaError> {
        Ok(())
    }
    /// Validate the ESP32 image header magic byte (`0xE9`) in the first chunk.
    ///
    /// Called once before the first `write()`. Returns
    /// `Err(OtaError::InvalidImageHeader)` if the magic byte is wrong.
    /// The default implementation is a no-op (always succeeds).
    fn validate_first_chunk(&mut self, _chunk: &[u8]) -> Result<(), OtaError> {
        Ok(())
    }
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
    #[error("firmware CRC mismatch: expected {expected:#010x}, got {actual:#010x}")]
    HashMismatch { expected: u32, actual: u32 },
    #[error("invalid ESP32 image header magic")]
    InvalidImageHeader,
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
            OtaError::HashMismatch {
                expected: 0xDEADBEEF,
                actual: 0xCAFEBABE,
            }
            .to_string(),
            OtaError::InvalidImageHeader.to_string(),
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

    // --- Failure injection tests ---

    use mock::MockOta;

    #[test]
    fn test_mock_ota_fail_on_begin() {
        let mut ota = MockOta::new();
        // Default: begin succeeds
        assert!(ota.begin().is_ok());

        let mut ota = MockOta::new();
        ota.fail_on_begin = true;
        assert!(matches!(ota.begin(), Err(OtaError::BeginFailed)));
    }

    #[test]
    fn test_mock_ota_fail_on_write_after() {
        let mut ota = MockOta::new();
        ota.fail_on_write_after = Some(7);
        ota.begin().unwrap();

        // Write 5 bytes — should succeed (5 <= 7)
        assert!(ota.write(&[1, 2, 3, 4, 5]).is_ok());

        // Write 5 more — would cross the 7-byte boundary
        let result = ota.write(&[6, 7, 8, 9, 10]);
        assert!(matches!(
            result,
            Err(OtaError::WriteFailed { byte_offset: 7 })
        ));

        // firmware_data should only contain the first 5 bytes
        assert_eq!(ota.firmware_data.len(), 5);
    }

    #[test]
    fn test_mock_ota_fail_on_write_after_default_none() {
        let mut ota = MockOta::new();
        ota.begin().unwrap();
        // Default fail_on_write_after is None — unlimited writes
        assert!(ota.write(&[1, 2, 3, 4, 5]).is_ok());
        assert!(ota.write(&[6, 7, 8, 9, 10]).is_ok());
        assert_eq!(ota.firmware_data.len(), 10);
    }

    #[test]
    fn test_mock_ota_fail_on_finalize() {
        let mut ota = MockOta::new();
        ota.fail_on_finalize = true;
        ota.begin().unwrap();
        ota.write(&[0xAA, 0xBB]).unwrap();
        assert!(matches!(ota.finalize(), Err(OtaError::FinalizeFailed)));
        assert!(!ota.finalized);
    }

    #[test]
    fn test_mock_ota_fail_on_finalize_default_off() {
        let mut ota = MockOta::new();
        ota.begin().unwrap();
        ota.write(&[0xAA, 0xBB]).unwrap();
        assert!(ota.finalize().is_ok());
        assert!(ota.finalized);
    }

    #[test]
    fn test_mock_ota_firmware_size_exceeded() {
        let mut ota = MockOta::new();
        ota.begin().unwrap();
        // Write exactly MAX_FIRMWARE_SIZE bytes — should succeed
        let chunk = vec![0xFFu8; 1024];
        let full_chunks = MAX_FIRMWARE_SIZE / 1024;
        for _ in 0..full_chunks {
            assert!(ota.write(&chunk).is_ok());
        }
        // Write one more byte — should fail with InvalidFirmware
        assert!(matches!(ota.write(&[0x00]), Err(OtaError::InvalidFirmware)));
    }

    #[test]
    fn test_mock_ota_begin_while_in_progress() {
        let mut ota = MockOta::new();
        assert!(ota.begin().is_ok());
        // Second begin while in progress should fail
        assert!(matches!(ota.begin(), Err(OtaError::BeginFailed)));
    }

    #[test]
    fn test_mock_ota_write_before_begin() {
        let mut ota = MockOta::new();
        // Write without begin should fail
        assert!(matches!(
            ota.write(&[0x01, 0x02]),
            Err(OtaError::WriteFailed { byte_offset: 0 })
        ));
    }

    #[test]
    fn test_mock_ota_finalize_zero_bytes() {
        let mut ota = MockOta::new();
        ota.begin().unwrap();
        // Finalize with zero bytes written should fail
        assert!(matches!(ota.finalize(), Err(OtaError::FinalizeFailed)));
        assert!(!ota.finalized);
    }

    #[test]
    fn test_mock_ota_finalize_not_in_progress() {
        let mut ota = MockOta::new();
        // Finalize without begin should fail
        assert!(matches!(ota.finalize(), Err(OtaError::FinalizeFailed)));
    }

    #[test]
    fn test_mock_ota_default_all_off() {
        let mut ota = MockOta::new();
        assert!(!ota.fail_on_begin);
        assert!(ota.fail_on_write_after.is_none());
        assert!(!ota.fail_on_finalize);
        // Full happy path should work with defaults
        assert!(ota.begin().is_ok());
        assert!(ota.write(&[0xAA]).is_ok());
        assert!(ota.finalize().is_ok());
        assert!(ota.mark_valid().is_ok());
        assert!(ota.finalized);
        assert!(ota.valid);
    }
}

/// Maximum firmware size in bytes (~1.75 MiB partition).
pub const MAX_FIRMWARE_SIZE: usize = 1_835_008;

#[cfg(any(test, feature = "mock"))]
pub mod mock {
    use super::{alloc::vec::Vec, OtaError, OtaUpdate, MAX_FIRMWARE_SIZE};

    /// Mock OTA updater for desktop testing.
    pub struct MockOta {
        pub firmware_data: Vec<u8>,
        pub finalized: bool,
        pub valid: bool,
        pub rolled_back: bool,
        /// When true, `begin()` returns `Err(OtaError::BeginFailed)`.
        pub fail_on_begin: bool,
        /// When `Some(N)`, first N bytes of writes succeed, then `Err(OtaError::WriteFailed { byte_offset: N })`.
        pub fail_on_write_after: Option<usize>,
        /// When true, `finalize()` returns `Err(OtaError::FinalizeFailed)`.
        pub fail_on_finalize: bool,
        /// Internal: tracks whether an OTA session is in progress.
        in_progress: bool,
    }

    impl MockOta {
        pub fn new() -> Self {
            MockOta {
                firmware_data: Vec::new(),
                finalized: false,
                valid: false,
                rolled_back: false,
                fail_on_begin: false,
                fail_on_write_after: None,
                fail_on_finalize: false,
                in_progress: false,
            }
        }
    }

    impl OtaUpdate for MockOta {
        fn begin(&mut self) -> Result<(), OtaError> {
            if self.in_progress {
                return Err(OtaError::BeginFailed);
            }
            if self.fail_on_begin {
                return Err(OtaError::BeginFailed);
            }
            self.firmware_data.clear();
            self.finalized = false;
            self.in_progress = true;
            Ok(())
        }

        fn write(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
            if !self.in_progress {
                return Err(OtaError::WriteFailed {
                    byte_offset: self.firmware_data.len(),
                });
            }
            if self.firmware_data.len() + chunk.len() > MAX_FIRMWARE_SIZE {
                return Err(OtaError::InvalidFirmware);
            }
            if let Some(limit) = self.fail_on_write_after {
                if self.firmware_data.len() + chunk.len() > limit {
                    return Err(OtaError::WriteFailed { byte_offset: limit });
                }
            }
            self.firmware_data.extend_from_slice(chunk);
            Ok(())
        }

        fn finalize(&mut self) -> Result<(), OtaError> {
            if !self.in_progress {
                return Err(OtaError::FinalizeFailed);
            }
            if self.firmware_data.is_empty() {
                return Err(OtaError::FinalizeFailed);
            }
            if self.fail_on_finalize {
                return Err(OtaError::FinalizeFailed);
            }
            self.finalized = true;
            self.in_progress = false;
            Ok(())
        }

        fn mark_valid(&mut self) -> Result<(), OtaError> {
            self.valid = true;
            Ok(())
        }

        fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
            self.rolled_back = true;
            self.in_progress = false;
            Ok(())
        }

        fn validate_first_chunk(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
            if chunk.is_empty() || chunk[0] != 0xE9 {
                return Err(OtaError::InvalidImageHeader);
            }
            Ok(())
        }
    }
}
