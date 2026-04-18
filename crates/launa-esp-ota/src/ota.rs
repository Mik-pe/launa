//! ESP32 OTA update state machine.
//!
//! Implements the `OtaUpdate` trait with begin/write/finalize/rollback
//! operations, managing partition selection, CRC verification, and
//! HMAC-SHA256 signature checking.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use launa_ota::{OtaError, OtaUpdate};
use log::{debug, info, warn};

use crate::crypto::{crc32_update, hmac_sha256_update, truncate_signature, SigningKey};
use crate::flash::Partition;
use crate::flash::{
    aligned_write, detect_running_partition, erase_range, set_boot_partition, WORD_SIZE,
};

/// ESP32 OTA updater backed by any `embedded-storage` NorFlash implementation.
///
/// Uses `esp_storage::FlashStorage` on target, but accepts any `NorFlash`
/// for testability.
pub struct EspOtaFlash<S> {
    pub(crate) flash: S,
    pub(crate) running: Partition,
    pub(crate) target: Partition,
    pub(crate) write_offset: u32,
    pub(crate) bytes_written: u32,
    pub(crate) in_progress: bool,
    /// Running CRC32 (CRC-32/MPEG-2) of all firmware data written so far.
    pub(crate) firmware_crc: u32,
    /// Whether the first chunk's ESP32 image header magic has been validated.
    pub(crate) first_chunk_validated: bool,
    /// Buffered partial word bytes not yet flushed to flash.
    /// NOR flash requires word-aligned (4-byte) writes; this buffer holds
    /// 0–3 bytes that didn't fit in the last aligned write. On the next
    /// `write()` call they are prepended to form complete words.
    pub(crate) pending_bytes: [u8; 3],
    /// Number of valid bytes in `pending_bytes` (0..=3).
    pub(crate) pending_len: usize,
    /// Accumulated firmware data for HMAC-SHA256 signature verification.
    /// NOTE: This accumulates the full firmware in RAM, which will OOM on
    /// real hardware (1.25 MiB firmware vs 32 KiB heap). Signature verification
    /// via `verify_signature()` is therefore not usable in production until
    /// incremental HMAC is implemented. CRC-32 verification is always safe.
    /// Retained for desktop testing only.
    pub(crate) firmware_data: alloc::vec::Vec<u8>,
}

impl<S> EspOtaFlash<S>
where
    S: NorFlash,
{
    /// Create a new OTA updater.
    ///
    /// `flash` is the flash storage backend (e.g. `esp_storage::FlashStorage`).
    /// `running` indicates which OTA partition the current firmware booted from.
    pub fn new(flash: S, running: Partition) -> Self {
        let target = match running {
            Partition::Ota0 => Partition::Ota1,
            Partition::Ota1 => Partition::Ota0,
        };

        EspOtaFlash {
            flash,
            running,
            target,
            write_offset: 0,
            bytes_written: 0,
            in_progress: false,
            firmware_crc: 0xFFFFFFFF,
            first_chunk_validated: false,
            pending_bytes: [0xFF; 3],
            pending_len: 0,
            firmware_data: alloc::vec::Vec::new(),
        }
    }

    /// Get the target partition for the update.
    pub fn target_partition(&self) -> Partition {
        self.target
    }

    /// Get the currently running partition.
    pub fn running_partition(&self) -> Partition {
        self.running
    }

    /// Consume self and return the flash storage backend.
    pub fn into_flash(self) -> S {
        self.flash
    }

    /// Default signing key for firmware verification.
    ///
    /// **WARNING**: This is a placeholder key for development/testing only.
    /// In production, this MUST be replaced with a key derived from ESP32
    /// eFuse BLOCK3 (similar to NVS encryption in crypto.rs). Any attacker
    /// with access to this source code can forge valid firmware signatures.
    ///
    /// Until a proper per-device provisioning flow is implemented, HMAC
    /// signature verification should NOT be relied upon for production
    /// OTA security. Use CRC-32 as a corruption check and rely on
    /// network-level security (TLS) for OTA integrity.
    pub fn default_signing_key() -> [u8; 32] {
        [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
            0x32, 0x10, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98,
            0x76, 0x54, 0x32, 0x10,
        ]
    }
}

impl<S> EspOtaFlash<S>
where
    S: NorFlash + ReadNorFlash,
{
    /// Determine which partition is currently booted by reading otadata.
    /// Returns the partition with the higher sequence number.
    pub fn detect_running_partition(&mut self) -> Result<Partition, OtaError> {
        detect_running_partition(&mut self.flash)
    }
}

impl<S> OtaUpdate for EspOtaFlash<S>
where
    S: NorFlash,
{
    fn begin(&mut self) -> Result<(), OtaError> {
        if self.in_progress {
            return Err(OtaError::BeginFailed);
        }

        info!(
            "OTA: beginning update to {:?} (offset 0x{:08X})",
            self.target,
            self.target.offset()
        );

        erase_range(&mut self.flash, self.target, 0, self.target.size())?;

        self.write_offset = 0;
        self.bytes_written = 0;
        self.in_progress = true;
        self.firmware_crc = 0xFFFFFFFF;
        self.first_chunk_validated = false;
        self.pending_len = 0;
        self.firmware_data.clear();

        info!("OTA: target partition erased");
        Ok(())
    }

    fn write(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
        if !self.in_progress {
            return Err(OtaError::WriteFailed { byte_offset: 0 });
        }

        if chunk.is_empty() {
            return Ok(());
        }

        // Validate ESP32 image header magic on first chunk
        if !self.first_chunk_validated {
            if chunk[0] != 0xE9 {
                warn!(
                    "OTA: invalid ESP32 image header magic: 0x{:02X}, expected 0xE9",
                    chunk[0]
                );
                return Err(OtaError::InvalidImageHeader);
            }
            self.first_chunk_validated = true;
        }

        if self.write_offset + chunk.len() as u32 > self.target.size() {
            warn!(
                "OTA: write overflow: offset {} + {} > partition size {}",
                self.write_offset,
                chunk.len(),
                self.target.size()
            );
            return Err(OtaError::WriteFailed {
                byte_offset: self.bytes_written as usize,
            });
        }

        // Accumulate CRC of firmware data (before any buffering)
        self.firmware_crc = crc32_update(self.firmware_crc, chunk);

        // Accumulate firmware data for HMAC-SHA256 signature verification
        self.firmware_data.extend_from_slice(chunk);

        // Prepend any pending partial-word bytes from the previous write.
        let combined: alloc::vec::Vec<u8>;
        let data = if self.pending_len > 0 {
            combined = self.pending_bytes[..self.pending_len]
                .iter()
                .chain(chunk.iter())
                .copied()
                .collect();
            self.pending_len = 0;
            &combined[..]
        } else {
            chunk
        };

        // Split data into whole words and a trailing partial word.
        let whole_words = data.len() / WORD_SIZE as usize;
        let remainder = data.len() % WORD_SIZE as usize;

        // Write all whole-word-aligned chunks.
        if whole_words > 0 {
            let aligned_end = whole_words * WORD_SIZE as usize;
            aligned_write(
                &mut self.flash,
                self.target,
                self.write_offset,
                &data[..aligned_end],
            )?;
            self.write_offset += aligned_end as u32;
        }

        // Buffer any trailing partial word for the next write.
        if remainder > 0 {
            let tail_start = whole_words * WORD_SIZE as usize;
            self.pending_bytes[..remainder].copy_from_slice(&data[tail_start..data.len()]);
            self.pending_len = remainder;
            // Note: write_offset does NOT advance for pending bytes yet —
            // they haven't been committed to flash.
        }

        self.bytes_written += chunk.len() as u32;

        if self.bytes_written % (32 * 1024) < chunk.len() as u32 {
            debug!(
                "OTA: written {} KiB / {} KiB",
                self.bytes_written / 1024,
                self.target.size() / 1024
            );
        }

        Ok(())
    }

    fn finalize(&mut self) -> Result<(), OtaError> {
        if !self.in_progress {
            return Err(OtaError::FinalizeFailed);
        }

        if self.bytes_written == 0 {
            warn!("OTA: finalize called with zero bytes written — refusing to boot into empty partition");
            self.in_progress = false;
            return Err(OtaError::FinalizeFailed);
        }

        // Flush any remaining partial-word bytes to flash.
        if self.pending_len > 0 {
            let pending: [u8; 3] = self.pending_bytes;
            let len = self.pending_len;
            aligned_write(
                &mut self.flash,
                self.target,
                self.write_offset,
                &pending[..len],
            )?;
            self.write_offset += len as u32;
            self.pending_len = 0;
        }

        info!(
            "OTA: finalizing {} bytes written to {:?}",
            self.bytes_written, self.target
        );

        set_boot_partition(&mut self.flash, self.target)?;

        self.in_progress = false;
        info!(
            "OTA: boot partition set to {:?}, reboot to apply",
            self.target
        );
        Ok(())
    }

    fn mark_valid(&mut self) -> Result<(), OtaError> {
        info!("OTA: marking firmware as valid (boot confirmed)");
        let detected = self.detect_running_partition()?;
        if detected == self.running {
            debug!(
                "OTA: running partition {:?} confirmed in otadata",
                self.running
            );
        } else {
            warn!(
                "OTA: detected {:?} but expected running {:?}, updating otadata",
                detected, self.running
            );
            set_boot_partition(&mut self.flash, self.running)?;
        }
        Ok(())
    }

    fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
        info!("OTA: rolling back to {:?}", self.running);
        set_boot_partition(&mut self.flash, self.running)?;
        info!("OTA: otadata updated for rollback. Caller must reset.");
        Ok(())
    }

    fn verify_hash(&mut self, expected_crc: u32) -> Result<(), OtaError> {
        if self.firmware_crc != expected_crc {
            warn!(
                "OTA: CRC mismatch: expected {:#010X}, actual {:#010X}",
                expected_crc, self.firmware_crc
            );
            return Err(OtaError::HashMismatch {
                expected: expected_crc,
                actual: self.firmware_crc,
            });
        }
        info!("OTA: firmware CRC verified: {:#010X}", self.firmware_crc);
        Ok(())
    }

    fn validate_first_chunk(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
        if chunk.is_empty() || chunk[0] != 0xE9 {
            warn!(
                "OTA: invalid ESP32 image header magic in pre-check: 0x{:02X}",
                chunk.first().copied().unwrap_or(0)
            );
            return Err(OtaError::InvalidImageHeader);
        }
        Ok(())
    }

    fn verify_signature(&mut self, expected_signature: u32) -> Result<(), OtaError> {
        // In production, the signing key would come from eFuse or compile-time constant.
        // For verification, the caller provides the key via a separate mechanism.
        // This method verifies against the default signing key.
        let key = SigningKey::new(Self::default_signing_key());
        let hmac = hmac_sha256_update(&key, &self.firmware_data);
        let actual = truncate_signature(&hmac);
        if actual != expected_signature {
            warn!(
                "OTA: signature mismatch: expected {:#010X}, actual {:#010X}",
                expected_signature, actual
            );
            return Err(OtaError::SignatureMismatch {
                expected: expected_signature,
                actual,
            });
        }
        info!("OTA: firmware signature verified: {:#010X}", actual);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{crc32, hmac_sha256};
    use crate::flash::{
        set_boot_partition, u32_from_be, OTADATA_OFFSET, OTA_0_SIZE, OTA_1_OFFSET, OTA_1_SIZE,
        OTA_ENTRY_SIZE, OTA_SEQ_OFFSET, OTA_SEQ_SIZE,
    };
    use embedded_storage::nor_flash::{ErrorType, NorFlashError};

    struct MockFlash {
        data: alloc::vec::Vec<u8>,
    }

    #[derive(Debug)]
    struct FlashError;

    impl NorFlashError for FlashError {
        fn kind(&self) -> embedded_storage::nor_flash::NorFlashErrorKind {
            embedded_storage::nor_flash::NorFlashErrorKind::Other
        }
    }

    impl MockFlash {
        fn new(size: usize) -> Self {
            MockFlash {
                data: alloc::vec![0xFFu8; size],
            }
        }
    }

    impl ErrorType for MockFlash {
        type Error = FlashError;
    }

    impl ReadNorFlash for MockFlash {
        const READ_SIZE: usize = 1;

        fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let off = offset as usize;
            if off + bytes.len() > self.data.len() {
                return Err(FlashError);
            }
            bytes.copy_from_slice(&self.data[off..off + bytes.len()]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.data.len()
        }
    }

    impl NorFlash for MockFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 4096;

        fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            let from = from as usize;
            let to = to as usize;
            if from >= self.data.len() || to > self.data.len() {
                return Err(FlashError);
            }
            for b in &mut self.data[from..to] {
                *b = 0xFF;
            }
            Ok(())
        }

        fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let off = offset as usize;
            if off + bytes.len() > self.data.len() {
                return Err(FlashError);
            }
            for (i, &b) in bytes.iter().enumerate() {
                self.data[off + i] = b;
            }
            Ok(())
        }
    }

    fn total_flash_size() -> usize {
        (OTA_1_OFFSET + OTA_1_SIZE) as usize
    }

    #[test]
    fn test_begin_erase_target_partition() {
        let mut flash = MockFlash::new(total_flash_size());
        let start = OTA_1_OFFSET as usize;
        flash.data[start..start + 16].copy_from_slice(&[0xAA; 16]);

        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        assert!(ota.begin().is_ok());
        assert_eq!(ota.target_partition(), Partition::Ota1);

        let inner = &ota.flash;
        for &b in &inner.data[start..start + 16] {
            assert_eq!(b, 0xFF);
        }
    }

    #[test]
    fn test_write_and_finalize() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        ota.begin().unwrap();

        let firmware: &[u8] = &[0xE9, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        ota.write(firmware).unwrap();
        ota.write(firmware).unwrap();
        ota.finalize().unwrap();

        let inner = &ota.flash;
        let base = OTA_1_OFFSET as usize;
        assert_eq!(&inner.data[base..base + 8], firmware);
        assert_eq!(&inner.data[base + 8..base + 16], firmware);

        let slot1_start = OTADATA_OFFSET as usize + OTA_ENTRY_SIZE;
        let entry = &inner.data[slot1_start..slot1_start + OTA_ENTRY_SIZE];
        let seq = u32_from_be(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
        assert!(seq > 0, "OTA slot 1 sequence should be > 0, got {}", seq);
    }

    #[test]
    fn test_mark_valid() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota1);
        ota.mark_valid().unwrap();
    }

    #[test]
    fn test_rollback() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota1);
        ota.rollback_and_reboot().unwrap();

        let inner = &ota.flash;
        let slot_start = OTADATA_OFFSET as usize + OTA_ENTRY_SIZE;
        let entry = &inner.data[slot_start..slot_start + OTA_ENTRY_SIZE];
        let seq = u32_from_be(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
        assert!(seq > 0, "Rollback slot should have seq > 0, got {}", seq);
    }

    #[test]
    fn test_detect_running_partition() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        let detected = ota.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota0);
    }

    #[test]
    fn test_write_overflow_rejected() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();
        let mut big_data = alloc::vec![0xAAu8; OTA_0_SIZE as usize + 1];
        big_data[0] = 0xE9; // Valid ESP32 image header magic
        assert!(ota.write(&big_data).is_err());
    }

    #[test]
    fn test_write_without_begin_rejected() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        assert!(ota.write(&[1, 2, 3, 4]).is_err());
    }

    #[test]
    fn test_finalize_without_begin_rejected() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        assert!(ota.finalize().is_err());
    }

    #[test]
    fn test_target_is_opposite_of_running() {
        let flash = MockFlash::new(total_flash_size());
        let ota = EspOtaFlash::new(flash, Partition::Ota0);
        assert_eq!(ota.target_partition(), Partition::Ota1);

        let flash = MockFlash::new(total_flash_size());
        let ota = EspOtaFlash::new(flash, Partition::Ota1);
        assert_eq!(ota.target_partition(), Partition::Ota0);
    }

    #[test]
    fn test_finalize_empty_image_rejected() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();
        // Finalize without writing any data should fail
        let result = ota.finalize();
        assert!(
            result.is_err(),
            "finalize with zero bytes written should return an error"
        );
        assert!(
            !ota.in_progress,
            "in_progress should be reset after empty finalize rejection"
        );
    }

    #[test]
    fn test_full_ota_cycle() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // OTA update to ota_1
        ota.begin().unwrap();
        let mut fw = alloc::vec![0xABu8; 1024];
        fw[0] = 0xE9; // Valid ESP32 image header magic
        ota.write(&fw).unwrap();
        ota.finalize().unwrap();

        // Simulate boot from ota_1
        let mut ota2 = EspOtaFlash::new(ota.flash, Partition::Ota1);
        ota2.mark_valid().unwrap();

        let detected = ota2.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota1);
    }

    #[test]
    fn test_invalid_image_header_rejected() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();
        // First byte NOT 0xE9 → should be rejected
        let result = ota.write(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(
            matches!(result, Err(OtaError::InvalidImageHeader)),
            "Expected InvalidImageHeader, got {:?}",
            result
        );
    }

    #[test]
    fn test_valid_image_header_accepted() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();
        // First byte 0xE9 → should succeed
        let result = ota.write(&[0xE9, 0x01, 0x02, 0x03]);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
    }

    #[test]
    fn test_header_validated_only_once() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();
        // First write with valid header
        ota.write(&[0xE9, 0x01, 0x02, 0x03]).unwrap();
        // Second write without 0xE9 header — should succeed (already validated)
        let result = ota.write(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
    }

    #[test]
    fn test_crc_accumulation_across_writes() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let chunk1: &[u8] = &[0xE9, 0x01, 0x02, 0x03];
        let chunk2: &[u8] = &[0x04, 0x05, 0x06, 0x07];
        ota.write(chunk1).unwrap();
        ota.write(chunk2).unwrap();

        // Compute expected CRC: crc32 of the concatenated data
        let all_data: alloc::vec::Vec<u8> = [chunk1, chunk2].concat();
        let expected_crc = crc32(&all_data);
        assert_eq!(ota.firmware_crc, expected_crc);
    }

    #[test]
    fn test_verify_hash_matches() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let chunk1: &[u8] = &[0xE9, 0x01, 0x02, 0x03];
        let chunk2: &[u8] = &[0x04, 0x05, 0x06, 0x07];
        ota.write(chunk1).unwrap();
        ota.write(chunk2).unwrap();

        let all_data: alloc::vec::Vec<u8> = [chunk1, chunk2].concat();
        let expected_crc = crc32(&all_data);

        assert!(ota.verify_hash(expected_crc).is_ok());
    }

    #[test]
    fn test_verify_hash_mismatch() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        ota.write(&[0xE9, 0x01, 0x02, 0x03]).unwrap();

        let result = ota.verify_hash(0x00000000);
        assert!(
            matches!(
                result,
                Err(OtaError::HashMismatch {
                    expected: 0x00000000,
                    actual: _
                })
            ),
            "Expected HashMismatch, got {:?}",
            result
        );
    }

    #[test]
    fn test_validate_first_chunk_valid() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        let result = ota.validate_first_chunk(&[0xE9, 0x01, 0x02, 0x03]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_first_chunk_invalid() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        let result = ota.validate_first_chunk(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(
            matches!(result, Err(OtaError::InvalidImageHeader)),
            "Expected InvalidImageHeader, got {:?}",
            result
        );
    }

    #[test]
    fn test_validate_first_chunk_empty() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        let result = ota.validate_first_chunk(&[]);
        assert!(
            matches!(result, Err(OtaError::InvalidImageHeader)),
            "Expected InvalidImageHeader for empty chunk, got {:?}",
            result
        );
    }

    #[test]
    fn test_crc_resets_on_begin() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();
        ota.write(&[0xE9, 0x01, 0x02, 0x03]).unwrap();
        assert_ne!(ota.firmware_crc, 0xFFFFFFFF);

        // Finalize the first session, then begin a new one
        ota.finalize().unwrap();
        ota.begin().unwrap();
        assert_eq!(ota.firmware_crc, 0xFFFFFFFF);
        assert!(!ota.first_chunk_validated);
    }

    #[test]
    fn test_write_empty_chunk_ok() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();
        // Empty chunk should succeed and not trigger header validation
        assert!(ota.write(&[]).is_ok());
        // Still not validated, so next write with non-0xE9 should fail
        assert!(matches!(
            ota.write(&[0xDE, 0xAD]),
            Err(OtaError::InvalidImageHeader)
        ));
    }

    /// Helper: read the raw 32-byte otadata entry for a given slot index (0 or 1).
    fn read_otadata_entry(flash: &MockFlash, slot: usize) -> [u8; OTA_ENTRY_SIZE] {
        let offset = OTADATA_OFFSET as usize + slot * OTA_ENTRY_SIZE;
        let mut buf = [0u8; OTA_ENTRY_SIZE];
        buf.copy_from_slice(&flash.data[offset..offset + OTA_ENTRY_SIZE]);
        buf
    }

    /// Helper: extract the sequence number from an otadata entry.
    fn seq_from_entry(entry: &[u8; OTA_ENTRY_SIZE]) -> u32 {
        let raw = u32_from_be(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
        if raw == 0xFFFFFFFF {
            0
        } else {
            raw
        }
    }

    #[test]
    fn test_both_otadata_slots_survive_sequential_writes() {
        // Write slot 0 first, then slot 1, and verify slot 0 is still intact.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // First: write to slot 0 via set_boot_partition(Ota0)
        // We need direct access, so use rollback which targets the running partition
        // EspOtaFlash running=Ota0 → set_boot_partition targets Ota0
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        let slot0_entry_after_first = read_otadata_entry(&ota.flash, 0);
        let slot0_seq_after_first = seq_from_entry(&slot0_entry_after_first);
        assert_eq!(
            slot0_seq_after_first, 1,
            "slot 0 seq should be 1 after first write"
        );

        // Now write to slot 1
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        let slot1_entry = read_otadata_entry(&ota.flash, 1);
        let slot1_seq = seq_from_entry(&slot1_entry);
        assert_eq!(slot1_seq, 1, "slot 1 seq should be 1");

        // Verify slot 0 is STILL intact (not destroyed by slot 1 erase)
        let slot0_entry_after_second = read_otadata_entry(&ota.flash, 0);
        let slot0_seq_after_second = seq_from_entry(&slot0_entry_after_second);
        assert_eq!(
            slot0_seq_after_second, slot0_seq_after_first,
            "slot 0 sequence must survive writing slot 1"
        );
        assert_eq!(
            slot0_entry_after_second, slot0_entry_after_first,
            "slot 0 full entry must be identical after writing slot 1"
        );
    }

    #[test]
    fn test_both_otadata_slots_survive_alternating_writes() {
        // Write to slots in alternating order multiple times.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // slot 0 → seq 1
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 0)), 1);
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 0);

        // slot 1 → seq 1
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 0)),
            1,
            "slot 0 lost after slot 1 write"
        );
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 1);

        // slot 0 → seq 2
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 0)), 2);
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 1)),
            1,
            "slot 1 lost after slot 0 write"
        );

        // slot 1 → seq 2
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 0)),
            2,
            "slot 0 lost after slot 1 write"
        );
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 2);
    }

    #[test]
    fn test_detect_running_after_shared_sector_writes() {
        // Verify detect_running_partition works correctly after multiple
        // set_boot_partition calls that go through read-modify-write.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // Boot from ota_1 (slot 1 gets higher seq)
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        let detected = ota.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota1);

        // Switch back to ota_0 (slot 0 gets higher seq)
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap(); // seq increments again
        let detected = ota.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota0);
    }

    #[test]
    fn test_unaligned_consecutive_writes_correct() {
        // VAL-CORE-015: Write chunk of 5 bytes then chunk of 3 bytes.
        // Read back first 8 bytes must match exact data with no corruption
        // from alignment padding.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let chunk1: &[u8] = &[0xE9, 0x01, 0x02, 0x03, 0x04]; // 5 bytes (unaligned)
        let chunk2: &[u8] = &[0x05, 0x06, 0x07]; // 3 bytes (unaligned)
        ota.write(chunk1).unwrap();
        ota.write(chunk2).unwrap();

        // Read back data from target partition
        let inner = &ota.flash;
        let base = OTA_1_OFFSET as usize;
        let first_8 = &inner.data[base..base + 8];
        assert_eq!(
            first_8,
            &[0xE9, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
            "consecutive unaligned writes must produce correct data"
        );

        // Padding bytes (bytes 8..12) should be 0xFF
        let padding = &inner.data[base + 8..base + 12];
        assert!(
            padding.iter().all(|&b| b == 0xFF),
            "padding bytes should be 0xFF, got {:?}",
            padding
        );
    }

    #[test]
    fn test_single_byte_chunks_write_offset() {
        // VAL-CORE-016: Write 8 individual single-byte chunks.
        // Read back must show sequential data at correct positions.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let data: [u8; 8] = [0xE9, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22];
        for (i, &byte) in data.iter().enumerate() {
            ota.write(&[byte]).unwrap();
            // bytes_written advances by 1 each time
            assert_eq!(
                ota.bytes_written,
                (i + 1) as u32,
                "bytes_written after byte {}",
                i
            );
        }

        // After 8 single-byte writes:
        // - First 4 bytes form word 0, flushed to flash at offset 0
        // - Next 4 bytes form word 1, flushed to flash at offset 4
        // So write_offset should be 8 (two complete words)
        assert_eq!(
            ota.write_offset, 8,
            "write_offset should be 8 after 8 bytes"
        );
        assert_eq!(ota.pending_len, 0, "no pending bytes after 8 (aligned)");

        // Read back from flash — data should be contiguous
        let inner = &ota.flash;
        let base = OTA_1_OFFSET as usize;
        assert_eq!(
            &inner.data[base..base + 8],
            &[0xE9, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22],
            "8 single-byte writes should produce contiguous data"
        );
    }

    #[test]
    fn test_aligned_chunks_no_padding_gap() {
        // VAL-CORE-017: Two 4-byte aligned writes: write_offset must be 4
        // after first, 8 after second.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let chunk1: &[u8] = &[0xE9, 0x01, 0x02, 0x03]; // 4 bytes (aligned)
        let chunk2: &[u8] = &[0x04, 0x05, 0x06, 0x07]; // 4 bytes (aligned)

        ota.write(chunk1).unwrap();
        assert_eq!(
            ota.write_offset, 4,
            "write_offset should be 4 after first aligned write"
        );

        ota.write(chunk2).unwrap();
        assert_eq!(
            ota.write_offset, 8,
            "write_offset should be 8 after second aligned write"
        );
    }

    #[test]
    fn test_bytes_written_excludes_padding() {
        // VAL-CORE-018: After writing 5 + 3 bytes, bytes_written must equal 8
        // (actual data), while write_offset may differ due to alignment.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let chunk1: &[u8] = &[0xE9, 0x01, 0x02, 0x03, 0x04]; // 5 bytes
        let chunk2: &[u8] = &[0x05, 0x06, 0x07]; // 3 bytes

        ota.write(chunk1).unwrap();
        assert_eq!(
            ota.bytes_written, 5,
            "bytes_written should be 5 after first chunk"
        );
        // 5 bytes: first 4 written as a word, 1 byte pending
        assert_eq!(
            ota.write_offset, 4,
            "write_offset should be 4 (one word flushed) after 5-byte chunk"
        );
        assert_eq!(ota.pending_len, 1, "1 byte pending");

        ota.write(chunk2).unwrap();
        assert_eq!(
            ota.bytes_written, 8,
            "bytes_written should be 8 after both chunks (5+3)"
        );
        // Pending 1 byte + 3 new = 4 bytes = one more word → flushed
        // write_offset: 4 + 4 = 8
        assert_eq!(
            ota.write_offset, 8,
            "write_offset should be 8 after both chunks"
        );
        assert_eq!(ota.pending_len, 0, "no pending bytes");

        // bytes_written == write_offset in this case (8 == 8)
        // because the total (5+3=8) is word-aligned.
        // The distinction matters more when total is unaligned:
    }

    #[test]
    fn test_bytes_written_vs_write_offset_unaligned() {
        // Supplementary: verify bytes_written differs from write_offset when
        // the total data written is not word-aligned.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        // Write exactly 5 bytes (total unaligned)
        ota.write(&[0xE9, 0x01, 0x02, 0x03, 0x04]).unwrap();
        assert_eq!(ota.bytes_written, 5);
        assert_eq!(ota.write_offset, 4); // only first word flushed
        assert_eq!(ota.pending_len, 1); // 1 byte pending
        assert_ne!(
            ota.bytes_written, ota.write_offset,
            "bytes_written must differ from write_offset when pending bytes exist"
        );
    }

    #[test]
    fn test_unaligned_write_across_sector_boundary() {
        // VAL-CORE-019: Write 4093 bytes then 7 bytes straddling sector
        // boundary. Data must be correct at boundary.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        // First chunk: 4093 bytes (ends at byte 4093, 3 bytes before sector end)
        let mut chunk1 = alloc::vec![0xAAu8; 4093];
        chunk1[0] = 0xE9; // Valid ESP32 image header
                          // Put distinctive bytes at the end of chunk1 and start of chunk2
        chunk1[4090] = 0xDE;
        chunk1[4091] = 0xAD;
        chunk1[4092] = 0xBE;

        // Second chunk: 7 bytes straddling into next sector
        let chunk2: &[u8] = &[0xEF, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];

        ota.write(&chunk1).unwrap();
        ota.write(chunk2).unwrap();

        // Finalize to flush any pending bytes
        ota.finalize().unwrap();

        // Read back from flash
        let inner = &ota.flash;
        let base = OTA_1_OFFSET as usize;

        // Verify tail of chunk1
        assert_eq!(
            inner.data[base + 4090],
            0xDE,
            "byte at offset 4090 should be 0xDE"
        );
        assert_eq!(
            inner.data[base + 4091],
            0xAD,
            "byte at offset 4091 should be 0xAD"
        );
        assert_eq!(
            inner.data[base + 4092],
            0xBE,
            "byte at offset 4092 should be 0xBE (end of chunk1)"
        );

        // Verify start of chunk2 — data is contiguous since we buffered partial words
        assert_eq!(
            inner.data[base + 4093],
            0xEF,
            "byte at offset 4093 should be 0xEF (start of chunk2)"
        );
        assert_eq!(
            inner.data[base + 4094],
            0x01,
            "byte at offset 4094 should be 0x01"
        );

        // bytes_written tracks actual data
        assert_eq!(
            ota.bytes_written, 4100,
            "bytes_written should be 4100 (4093 + 7)"
        );
    }

    #[test]
    fn test_verify_signature_matches() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let firmware: &[u8] = &[0xE9, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        ota.write(firmware).unwrap();

        // Compute the expected signature
        let key = SigningKey::new(EspOtaFlash::<MockFlash>::default_signing_key());
        let hmac = hmac_sha256(&key, firmware);
        let expected_sig = truncate_signature(&hmac);

        assert!(
            ota.verify_signature(expected_sig).is_ok(),
            "Signature verification should succeed with correct signature"
        );
    }

    #[test]
    fn test_verify_signature_mismatch() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let firmware: &[u8] = &[0xE9, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        ota.write(firmware).unwrap();

        // Use a wrong signature
        let result = ota.verify_signature(0xDEADBEEF);
        assert!(
            matches!(
                result,
                Err(OtaError::SignatureMismatch {
                    expected: 0xDEADBEEF,
                    actual: _
                })
            ),
            "Expected SignatureMismatch, got {:?}",
            result
        );
    }

    #[test]
    fn test_verify_signature_tampered_firmware() {
        // Sign original firmware, then tamper with data → verification fails
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let original: &[u8] = &[0xE9, 0x01, 0x02, 0x03];
        ota.write(original).unwrap();

        // Compute expected signature for original
        let key = SigningKey::new(EspOtaFlash::<MockFlash>::default_signing_key());
        let hmac = hmac_sha256(&key, original);
        let original_sig = truncate_signature(&hmac);

        // Tamper: modify firmware_data in-place (simulating bit-flip attack)
        ota.firmware_data[2] = 0xFF;

        let result = ota.verify_signature(original_sig);
        assert!(
            matches!(result, Err(OtaError::SignatureMismatch { .. })),
            "Tampered firmware should fail signature verification, got {:?}",
            result
        );
    }

    #[test]
    fn test_verify_signature_across_multiple_chunks() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);
        ota.begin().unwrap();

        let chunk1: &[u8] = &[0xE9, 0x01, 0x02, 0x03];
        let chunk2: &[u8] = &[0x04, 0x05, 0x06, 0x07];
        let chunk3: &[u8] = &[0x08, 0x09, 0x0A, 0x0B];
        ota.write(chunk1).unwrap();
        ota.write(chunk2).unwrap();
        ota.write(chunk3).unwrap();

        // Compute expected signature over concatenated data
        let all_data: alloc::vec::Vec<u8> = [chunk1, chunk2, chunk3].concat();
        let key = SigningKey::new(EspOtaFlash::<MockFlash>::default_signing_key());
        let hmac = hmac_sha256(&key, &all_data);
        let expected_sig = truncate_signature(&hmac);

        assert!(
            ota.verify_signature(expected_sig).is_ok(),
            "Signature verification should succeed across multiple chunks"
        );
    }

    #[test]
    fn test_signature_resets_on_begin() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // First OTA session
        ota.begin().unwrap();
        let fw1: &[u8] = &[0xE9, 0x01, 0x02, 0x03];
        ota.write(fw1).unwrap();
        let key = SigningKey::new(EspOtaFlash::<MockFlash>::default_signing_key());
        let hmac1 = hmac_sha256(&key, fw1);
        let sig1 = truncate_signature(&hmac1);
        assert!(ota.verify_signature(sig1).is_ok());

        // Reset and new session with different firmware
        ota.finalize().unwrap();
        ota.begin().unwrap();
        assert!(
            ota.firmware_data.is_empty(),
            "firmware_data should be cleared on begin()"
        );

        // Write different firmware
        let fw2: &[u8] = &[0xE9, 0xAA, 0xBB, 0xCC];
        ota.write(fw2).unwrap();

        // Old signature should NOT match new firmware
        let result = ota.verify_signature(sig1);
        assert!(
            matches!(result, Err(OtaError::SignatureMismatch { .. })),
            "Old signature should not match new firmware, got {:?}",
            result
        );

        // New correct signature should work
        let hmac2 = hmac_sha256(&key, fw2);
        let sig2 = truncate_signature(&hmac2);
        assert!(ota.verify_signature(sig2).is_ok());
    }

    #[test]
    fn test_full_ota_with_signature_verification() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // Build firmware
        let mut fw = alloc::vec![0xABu8; 1024];
        fw[0] = 0xE9; // Valid ESP32 image header magic

        // Compute expected signature
        let key = SigningKey::new(EspOtaFlash::<MockFlash>::default_signing_key());
        let hmac = hmac_sha256(&key, &fw);
        let expected_sig = truncate_signature(&hmac);

        // OTA flow with signature verification
        ota.begin().unwrap();
        ota.write(&fw).unwrap();

        // Verify signature before finalizing
        assert!(ota.verify_signature(expected_sig).is_ok());

        // Verify CRC also
        let expected_crc = crc32(&fw);
        assert!(ota.verify_hash(expected_crc).is_ok());

        ota.finalize().unwrap();

        // Simulate boot from new partition and mark valid
        let mut ota2 = EspOtaFlash::new(ota.flash, Partition::Ota1);
        ota2.mark_valid().unwrap();
    }
}
