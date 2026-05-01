//! ESP32 OTA update state machine.
//!
//! Implements the `OtaUpdate` trait with begin/write/finalize/rollback
//! operations, managing partition selection and CRC-32 integrity verification.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use launa_ota::{OtaError, OtaUpdate};
use log::{debug, info, warn};

use crate::crypto::crc32_update;
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

        // Read both otadata entries and check CRC validity.
        let (entry0, valid0, entry1, valid1) = crate::flash::read_otadata_entries(&mut self.flash)?;
        let running_slot = self.running.index();

        // Check if the running partition's otadata slot has a valid CRC
        // and points to the correct partition.
        let running_entry = if running_slot == 0 { &entry0 } else { &entry1 };
        let running_valid = if running_slot == 0 { valid0 } else { valid1 };
        let running_seq = if running_valid {
            crate::flash::seq_from_entry(running_entry)
        } else {
            0
        };
        let running_correct = running_seq > 0 && (running_seq - 1) % 2 == running_slot as u32;

        let other_valid = if running_slot == 0 { valid1 } else { valid0 };

        if running_correct && other_valid {
            debug!(
                "OTA: running partition {:?} confirmed in otadata (both slots valid)",
                self.running
            );
        } else {
            // Active slot is corrupt/wrong or the other slot is invalid.
            // Rewrite both to ensure redundancy.
            warn!(
                "OTA: otadata needs repair (running slot valid={}, correct={}, other valid={}), rewriting",
                running_valid, running_correct, other_valid
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::crc32;
    use crate::flash::{
        set_boot_partition, u32_from_le, OTADATA_OFFSET, OTA_0_SIZE, OTA_1_OFFSET, OTA_1_SIZE,
        OTA_CRC_OFFSET, OTA_ENTRY_SIZE, OTA_SEQ_OFFSET, OTA_SEQ_SIZE, SECTOR_SIZE,
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

        let slot1_start = OTADATA_OFFSET as usize + SECTOR_SIZE as usize;
        let entry = &inner.data[slot1_start..slot1_start + OTA_ENTRY_SIZE];
        let seq = u32_from_le(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
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
        let slot_start = OTADATA_OFFSET as usize + SECTOR_SIZE as usize;
        let entry = &inner.data[slot_start..slot_start + OTA_ENTRY_SIZE];
        let seq = u32_from_le(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
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
        let offset = OTADATA_OFFSET as usize + slot * SECTOR_SIZE as usize;
        let mut buf = [0u8; OTA_ENTRY_SIZE];
        buf.copy_from_slice(&flash.data[offset..offset + OTA_ENTRY_SIZE]);
        buf
    }

    /// Helper: extract the sequence number from an otadata entry.
    fn seq_from_entry(entry: &[u8; OTA_ENTRY_SIZE]) -> u32 {
        let raw = u32_from_le(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
        if raw == 0xFFFFFFFF {
            0
        } else {
            raw
        }
    }

    #[test]
    fn test_both_otadata_slots_survive_sequential_writes() {
        // Write slot 0 first, then slot 1, and verify both slots remain valid.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // First: set_boot_partition(Ota0) on clean flash seeds both slots.
        // Slot 0 gets seq=1 (Ota0), slot 1 gets seeded with seq=2 (Ota1).
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        let slot0_entry_after_first = read_otadata_entry(&ota.flash, 0);
        let slot0_seq_after_first = seq_from_entry(&slot0_entry_after_first);
        assert_eq!(
            slot0_seq_after_first, 1,
            "slot 0 seq should be 1 after first write"
        );
        // Slot 1 was seeded because it was empty
        let slot1_seq_after_first = seq_from_entry(&read_otadata_entry(&ota.flash, 1));
        assert_eq!(
            slot1_seq_after_first, 2,
            "slot 1 should be seeded with seq=2"
        );

        // Now write to slot 1 (Ota1) — slot 0 is already valid so only slot 1 is updated
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        let slot1_entry = read_otadata_entry(&ota.flash, 1);
        let slot1_seq = seq_from_entry(&slot1_entry);
        // max_seq was 2, next even seq > 2 for Ota1 is 4
        assert_eq!(
            slot1_seq, 4,
            "slot 1 seq should be 4 (Ota1 needs even seq, >2)"
        );

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
        // With seeding, the first write to a clean flash will seed both slots.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // First write on clean flash: set Ota0 → slot 0 gets seq=1, slot 1 seeded with seq=2
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 0)), 1);
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 2);

        // set Ota1: max_seq=2, next even for Ota1 is 4 → slot 1 gets seq=4, slot 0 unchanged
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 0)),
            1,
            "slot 0 lost after slot 1 write"
        );
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 4);

        // set Ota0: max_seq=4, next odd for Ota0 is 5 → slot 0 gets seq=5, slot 1 unchanged
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 0)), 5);
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 1)),
            4,
            "slot 1 lost after slot 0 write"
        );

        // set Ota1: max_seq=5, next even for Ota1 is 6 → slot 1 gets seq=6
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 0)),
            5,
            "slot 0 lost after slot 1 write"
        );
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 6);
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
    fn test_full_ota_with_crc_verification() {
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        let mut fw = alloc::vec![0xABu8; 1024];
        fw[0] = 0xE9; // Valid ESP32 image header magic

        let expected_crc = crc32(&fw);

        ota.begin().unwrap();
        ota.write(&fw).unwrap();
        assert!(ota.verify_hash(expected_crc).is_ok());
        ota.finalize().unwrap();

        let mut ota2 = EspOtaFlash::new(ota.flash, Partition::Ota1);
        ota2.mark_valid().unwrap();
    }

    #[test]
    fn test_read_otadata_sequences_validates_crc() {
        // Corrupt the CRC of slot 1 and verify read_otadata_sequences returns 0 for it.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // Write valid entries to both slots
        set_boot_partition(&mut ota.flash, Partition::Ota0).unwrap();
        // Both slots should be valid now
        let (seq_0, seq_1) = crate::flash::read_otadata_sequences(&mut ota.flash).unwrap();
        assert_eq!(seq_0, 1, "slot 0 should have seq 1");
        assert!(seq_1 > 0, "slot 1 should be seeded");

        // Corrupt slot 1's CRC
        let slot1_crc_offset = (OTADATA_OFFSET + SECTOR_SIZE) as usize + OTA_CRC_OFFSET;
        ota.flash.data[slot1_crc_offset] ^= 0xFF; // Flip CRC bytes

        // Slot 1 should now be reported as invalid (seq=0)
        let (seq_0_after, seq_1_after) =
            crate::flash::read_otadata_sequences(&mut ota.flash).unwrap();
        assert_eq!(seq_0_after, 1, "slot 0 still valid");
        assert_eq!(seq_1_after, 0, "slot 1 should be 0 after CRC corruption");
    }

    #[test]
    fn test_mark_valid_skips_write_when_both_slots_valid() {
        // mark_valid should NOT rewrite when both otadata slots have valid CRCs
        // and the running partition's entry is correct.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // Perform a full OTA cycle
        ota.begin().unwrap();
        let mut fw = alloc::vec![0xABu8; 256];
        fw[0] = 0xE9;
        ota.write(&fw).unwrap();
        ota.finalize().unwrap();

        // Snapshot the flash after finalize (both slots seeded)
        let flash_snapshot = ota.flash.data.clone();

        // Simulate boot from ota_1
        let mut ota2 = EspOtaFlash::new(ota.flash, Partition::Ota1);
        ota2.mark_valid().unwrap();

        // Flash should be unchanged — mark_valid detected both slots valid, no write needed
        assert_eq!(
            ota2.flash.data, flash_snapshot,
            "mark_valid should not write flash when both slots are valid"
        );
    }

    #[test]
    fn test_mark_valid_repairs_corrupted_active_slot() {
        // If the running partition's otadata slot has a corrupted CRC,
        // mark_valid should detect it and rewrite both slots.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // Full OTA cycle
        ota.begin().unwrap();
        let mut fw = alloc::vec![0xABu8; 256];
        fw[0] = 0xE9;
        ota.write(&fw).unwrap();
        ota.finalize().unwrap();

        // Corrupt slot 1's CRC (ota_1 is the target, which becomes running after reboot)
        let slot1_crc_offset = (OTADATA_OFFSET + SECTOR_SIZE) as usize + OTA_CRC_OFFSET;
        ota.flash.data[slot1_crc_offset] ^= 0xFF;

        // Verify it's actually corrupted
        let (_, valid0, _, valid1) = crate::flash::read_otadata_entries(&mut ota.flash).unwrap();
        assert!(valid0, "slot 0 should still be valid");
        assert!(!valid1, "slot 1 should be invalid after CRC corruption");

        // mark_valid should repair
        let mut ota2 = EspOtaFlash::new(ota.flash, Partition::Ota1);
        ota2.mark_valid().unwrap();

        // Both slots should be valid now
        let (_, valid0_after, _, valid1_after) =
            crate::flash::read_otadata_entries(&mut ota2.flash).unwrap();
        assert!(valid0_after, "slot 0 should be valid after repair");
        assert!(valid1_after, "slot 1 should be valid after repair");

        // And detect_running_partition should correctly find Ota1
        let detected = ota2.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota1);
    }

    #[test]
    fn test_mark_valid_repairs_missing_other_slot() {
        // If the other slot is erased/invalid, mark_valid should rewrite.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // Manually write only slot 1 (simulating old behavior where only one slot was written)
        set_boot_partition(&mut ota.flash, Partition::Ota1).unwrap();
        // Now erase slot 0 to simulate it being invalid
        let sector_base = OTADATA_OFFSET;
        for b in &mut ota.flash.data[sector_base as usize..(sector_base + SECTOR_SIZE) as usize] {
            *b = 0xFF;
        }

        // Verify: slot 0 invalid, slot 1 valid
        let (_, valid0, _, valid1) = crate::flash::read_otadata_entries(&mut ota.flash).unwrap();
        assert!(!valid0, "slot 0 should be invalid (erased)");
        assert!(valid1, "slot 1 should be valid");

        // mark_valid with running=Ota1 should repair by seeding slot 0
        let mut ota2 = EspOtaFlash::new(ota.flash, Partition::Ota1);
        ota2.mark_valid().unwrap();

        // Both slots should now be valid
        let (_, valid0_after, _, valid1_after) =
            crate::flash::read_otadata_entries(&mut ota2.flash).unwrap();
        assert!(valid0_after, "slot 0 should be valid after repair");
        assert!(valid1_after, "slot 1 should still be valid");
    }

    #[test]
    fn test_first_ota_seeds_both_otadata_slots() {
        // Simulate the exact scenario from the bug: first OTA from factory.
        // After USB flash, otadata is erased. OTA writes to ota_1.
        // Both slots should be valid after finalize.
        let flash = MockFlash::new(total_flash_size());

        // Simulate factory boot: create_ota detects both slots = 0 → defaults to Ota0
        let mut temp = EspOtaFlash::new(flash, Partition::Ota0);
        let running = temp.detect_running_partition().unwrap();
        assert_eq!(
            running,
            Partition::Ota0,
            "should default to Ota0 on clean flash"
        );
        let storage = temp.into_flash();

        // OTA update (running thinks it's Ota0, so target is Ota1)
        let mut ota = EspOtaFlash::new(storage, running);
        ota.begin().unwrap();
        let mut fw = alloc::vec![0xABu8; 256];
        fw[0] = 0xE9;
        ota.write(&fw).unwrap();
        ota.finalize().unwrap();

        // Both slots should be valid after finalize
        let (entry0, valid0, entry1, valid1) =
            crate::flash::read_otadata_entries(&mut ota.flash).unwrap();
        assert!(valid0, "slot 0 should be valid after first OTA");
        assert!(valid1, "slot 1 should be valid after first OTA");

        let seq_0 = crate::flash::seq_from_entry(&entry0);
        let seq_1 = crate::flash::seq_from_entry(&entry1);
        // Slot 1 should have higher seq (boot partition = ota_1)
        assert!(seq_1 > seq_0, "slot 1 should have higher seq than slot 0");

        // detect_running should find Ota1
        let detected = ota.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota1);
    }

    #[test]
    fn test_single_corruption_survives_after_seeding() {
        // After OTA with seeding, corrupt one slot and verify the other still works.
        let flash = MockFlash::new(total_flash_size());
        let mut ota = EspOtaFlash::new(flash, Partition::Ota0);

        // Full OTA
        ota.begin().unwrap();
        let mut fw = alloc::vec![0xABu8; 256];
        fw[0] = 0xE9;
        ota.write(&fw).unwrap();
        ota.finalize().unwrap();

        // Corrupt slot 1 entirely (simulate brownout)
        let slot1_start = (OTADATA_OFFSET + SECTOR_SIZE) as usize;
        for b in &mut ota.flash.data[slot1_start..slot1_start + OTA_ENTRY_SIZE] {
            *b = 0x00;
        }

        // Slot 0 should still be valid and detectable
        let (_, valid0, _, _) = crate::flash::read_otadata_entries(&mut ota.flash).unwrap();
        assert!(valid0, "slot 0 should survive slot 1 corruption");

        // detect_running should still find a valid partition via slot 0
        let detected = ota.detect_running_partition().unwrap();
        // Slot 0 has seq=1 which maps to Ota0
        assert_eq!(detected, Partition::Ota0);
    }
}
