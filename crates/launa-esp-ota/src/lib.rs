//! Custom ESP32 OTA implementation using `embedded-storage` traits.
//!
//! Uses `esp-storage::FlashStorage` (which implements `embedded-storage`
//! traits) for direct flash access.
//!
//! # Partition Layout (must match `app/partitions.csv`)
//!
//! ```text
//! otadata:  offset 0x10000, size 0x2000 (8 KiB)
//! factory:  offset 0x20000, size 0x140000 (1.25 MiB)
//! ota_0:    offset 0x160000, size 0x140000 (1.25 MiB)
//! ota_1:    offset 0x2A0000, size 0x140000 (1.25 MiB)
//! ```
//!
//! # OTA Data Format
//!
//! The `otadata` partition holds two 32-byte OTA slots. Each slot contains:
//! - Bytes 0..3: CRC32 of the remaining 28 bytes
//! - Bytes 4..7: Sequence number (big-endian u32)
//! - Bytes 8..32: Reserved (zeroed)
//!
//! The bootloader picks the slot with the higher sequence number. After
//! a successful boot the app calls `mark_valid()` which writes a valid
//! entry. If the app crashes before `mark_valid()`, the bootloader rolls
//! back to the previous slot.

#![no_std]

extern crate alloc;

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use launa_ota::{OtaError, OtaUpdate};
use log::{debug, info, warn};

// ── Partition table constants (must match app/partitions.csv) ──────────

const OTADATA_OFFSET: u32 = 0x10000;
const OTA_0_OFFSET: u32 = 0x160000;
const OTA_0_SIZE: u32 = 0x140000;
const OTA_1_OFFSET: u32 = 0x2A0000;
const OTA_1_SIZE: u32 = 0x140000;

const SECTOR_SIZE: u32 = 4096;
const WORD_SIZE: u32 = 4;

// OTA data entry: 32 bytes, two slots in otadata partition
const OTA_ENTRY_SIZE: usize = 32;

const OTA_SEQ_OFFSET: usize = 4;
const OTA_SEQ_SIZE: usize = 4;

/// Boot partition identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Partition {
    Ota0,
    Ota1,
}

impl Partition {
    fn offset(&self) -> u32 {
        match self {
            Partition::Ota0 => OTA_0_OFFSET,
            Partition::Ota1 => OTA_1_OFFSET,
        }
    }

    fn size(&self) -> u32 {
        match self {
            Partition::Ota0 => OTA_0_SIZE,
            Partition::Ota1 => OTA_1_SIZE,
        }
    }

    fn index(&self) -> usize {
        match self {
            Partition::Ota0 => 0,
            Partition::Ota1 => 1,
        }
    }
}

/// ESP32 OTA updater backed by any `embedded-storage` NorFlash implementation.
///
/// Uses `esp_storage::FlashStorage` on target, but accepts any `NorFlash`
/// for testability.
pub struct EspOtaFlash<S> {
    flash: S,
    running: Partition,
    target: Partition,
    write_offset: u32,
    bytes_written: u32,
    in_progress: bool,
    /// Running CRC32 (CRC-32/MPEG-2) of all firmware data written so far.
    firmware_crc: u32,
    /// Whether the first chunk's ESP32 image header magic has been validated.
    first_chunk_validated: bool,
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

    /// Erase sectors in the target partition from `start` to `end` (byte offsets
    /// relative to partition start).
    fn erase_range(&mut self, start: u32, end: u32) -> Result<(), OtaError> {
        let base = self.target.offset();
        let aligned_start = (start / SECTOR_SIZE) * SECTOR_SIZE;
        let aligned_end = ((end + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;

        let mut offset = aligned_start;
        while offset < aligned_end {
            let abs_offset = base + offset;
            debug!("Erasing sector at 0x{:08X}", abs_offset);
            let addr = abs_offset;
            self.flash
                .erase(abs_offset, abs_offset + SECTOR_SIZE)
                .map_err(|_| OtaError::FlashError { address: addr })?;
            offset += SECTOR_SIZE;
        }
        Ok(())
    }

    /// Align a buffer to word boundary (4 bytes) by padding with 0xFF.
    fn aligned_write(&mut self, offset: u32, data: &[u8]) -> Result<(), OtaError> {
        let abs_offset = self.target.offset() + offset;

        if data.len() % WORD_SIZE as usize == 0 {
            self.flash
                .write(abs_offset, data)
                .map_err(|_| OtaError::FlashError {
                    address: abs_offset,
                })?;
        } else {
            let pad_len =
                (WORD_SIZE as usize - (data.len() % WORD_SIZE as usize)) % WORD_SIZE as usize;
            let mut padded = alloc::vec![0xFFu8; data.len() + pad_len];
            padded[..data.len()].copy_from_slice(data);
            self.flash
                .write(abs_offset, &padded)
                .map_err(|_| OtaError::FlashError {
                    address: abs_offset,
                })?;
        }
        Ok(())
    }

    /// Read the sequence numbers from the otadata partition.
    /// Returns (seq_0, seq_1) where each is 0 if the slot is empty/erased.
    fn read_otadata_sequences(&mut self) -> Result<(u32, u32), OtaError> {
        let mut buf = [0u8; OTA_ENTRY_SIZE * 2];
        ReadNorFlash::read(&mut self.flash, OTADATA_OFFSET, &mut buf).map_err(|_| {
            OtaError::FlashError {
                address: OTADATA_OFFSET,
            }
        })?;

        let raw_0 = u32_from_be(&buf[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
        let raw_1 = u32_from_be(
            &buf[OTA_ENTRY_SIZE + OTA_SEQ_OFFSET..OTA_ENTRY_SIZE + OTA_SEQ_OFFSET + OTA_SEQ_SIZE],
        );

        // Treat 0xFFFFFFFF (erased flash) as empty/0
        fn sanitize(raw: u32) -> u32 {
            if raw == 0xFFFFFFFF {
                0
            } else {
                raw
            }
        }

        Ok((sanitize(raw_0), sanitize(raw_1)))
    }

    /// Write an otadata entry to set the boot partition.
    ///
    /// The bootloader compares sequence numbers and boots from the slot
    /// with the higher value. We increment the target slot's sequence.
    fn set_boot_partition(&mut self, partition: Partition) -> Result<(), OtaError> {
        let (seq_0, seq_1) = self.read_otadata_sequences()?;

        let new_seq = match partition {
            Partition::Ota0 => {
                if seq_0 == 0 {
                    1
                } else {
                    seq_0 + 1
                }
            }
            Partition::Ota1 => {
                if seq_1 == 0 {
                    1
                } else {
                    seq_1 + 1
                }
            }
        };

        let mut entry = [0u8; OTA_ENTRY_SIZE];
        entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]
            .copy_from_slice(&new_seq.to_be_bytes());

        let crc = crc32(&entry[4..]);
        entry[..4].copy_from_slice(&crc.to_le_bytes());

        let slot_offset = OTADATA_OFFSET + (partition.index() as u32 * OTA_ENTRY_SIZE as u32);

        self.flash
            .erase(slot_offset, slot_offset + SECTOR_SIZE)
            .map_err(|_| OtaError::FlashError {
                address: slot_offset,
            })?;

        self.flash
            .write(slot_offset, &entry)
            .map_err(|_| OtaError::FlashError {
                address: slot_offset,
            })?;

        Ok(())
    }

    /// Determine which partition is currently booted by reading otadata.
    /// Returns the partition with the higher sequence number.
    pub fn detect_running_partition(&mut self) -> Result<Partition, OtaError> {
        let (seq_0, seq_1) = self.read_otadata_sequences()?;
        Ok(if seq_1 > seq_0 {
            Partition::Ota1
        } else {
            Partition::Ota0
        })
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

        self.erase_range(0, self.target.size())?;

        self.write_offset = 0;
        self.bytes_written = 0;
        self.in_progress = true;
        self.firmware_crc = 0xFFFFFFFF;
        self.first_chunk_validated = false;

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

        self.aligned_write(self.write_offset, chunk)?;

        // Accumulate CRC of firmware data
        self.firmware_crc = crc32_update(self.firmware_crc, chunk);

        self.write_offset += chunk.len() as u32;
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

        info!(
            "OTA: finalizing {} bytes written to {:?}",
            self.bytes_written, self.target
        );

        self.set_boot_partition(self.target)?;

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
            self.set_boot_partition(self.running)?;
        }
        Ok(())
    }

    fn rollback_and_reboot(&mut self) -> Result<(), OtaError> {
        info!("OTA: rolling back to {:?}", self.running);
        self.set_boot_partition(self.running)?;
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

// ── Helpers ────────────────────────────────────────────────────────────

fn u32_from_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// CRC32 (CRC-32/MPEG-2) incremental update. Polynomial 0x04C11DB7.
fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut crc = crc;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            if crc & 0x80000000 != 0 {
                crc = (crc << 1) ^ 0x04C11DB7;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// CRC32 (CRC-32/MPEG-2) for otadata entries. Polynomial 0x04C11DB7.
fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFFFFFF, data)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_crc32() {
        // CRC-32/MPEG-2: init=0xFFFFFFFF, poly=0x04C11DB7, no final XOR
        assert_eq!(crc32(&[]), 0xFFFFFFFF);
        assert_eq!(crc32(&[0x01]), 1254728195u32);
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

    // ── Firmware integrity verification tests ──────────────────────────

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
}
