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
    /// Buffered partial word bytes not yet flushed to flash.
    /// NOR flash requires word-aligned (4-byte) writes; this buffer holds
    /// 0–3 bytes that didn't fit in the last aligned write. On the next
    /// `write()` call they are prepended to form complete words.
    pending_bytes: [u8; 3],
    /// Number of valid bytes in `pending_bytes` (0..=3).
    pending_len: usize,
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

        // Both otadata slots share the same 4 KiB sector (slot 0 at 0x10000,
        // slot 1 at 0x10020).  A naive erase-then-write would destroy the
        // companion slot on power loss.  Use a read-modify-write cycle so the
        // other slot's data is preserved across the erase.
        let sector_base = slot_offset & !(SECTOR_SIZE - 1);
        let mut sector_buf = [0xFFu8; SECTOR_SIZE as usize];
        ReadNorFlash::read(&mut self.flash, sector_base, &mut sector_buf).map_err(|_| {
            OtaError::FlashError {
                address: sector_base,
            }
        })?;

        let entry_offset = (slot_offset - sector_base) as usize;
        sector_buf[entry_offset..entry_offset + OTA_ENTRY_SIZE].copy_from_slice(&entry);

        self.flash
            .erase(sector_base, sector_base + SECTOR_SIZE)
            .map_err(|_| OtaError::FlashError {
                address: sector_base,
            })?;

        self.flash
            .write(sector_base, &sector_buf)
            .map_err(|_| OtaError::FlashError {
                address: sector_base,
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
            self.aligned_write(self.write_offset, &data[..aligned_end])?;
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
            self.aligned_write(self.write_offset, &pending[..len])?;
            self.write_offset += len as u32;
            self.pending_len = 0;
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

    // ── Shared-sector otadata tests ────────────────────────────────────

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
        ota.set_boot_partition(Partition::Ota0).unwrap();
        let slot0_entry_after_first = read_otadata_entry(&ota.flash, 0);
        let slot0_seq_after_first = seq_from_entry(&slot0_entry_after_first);
        assert_eq!(
            slot0_seq_after_first, 1,
            "slot 0 seq should be 1 after first write"
        );

        // Now write to slot 1
        ota.set_boot_partition(Partition::Ota1).unwrap();
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
        ota.set_boot_partition(Partition::Ota0).unwrap();
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 0)), 1);
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 0);

        // slot 1 → seq 1
        ota.set_boot_partition(Partition::Ota1).unwrap();
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 0)),
            1,
            "slot 0 lost after slot 1 write"
        );
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 1)), 1);

        // slot 0 → seq 2
        ota.set_boot_partition(Partition::Ota0).unwrap();
        assert_eq!(seq_from_entry(&read_otadata_entry(&ota.flash, 0)), 2);
        assert_eq!(
            seq_from_entry(&read_otadata_entry(&ota.flash, 1)),
            1,
            "slot 1 lost after slot 0 write"
        );

        // slot 1 → seq 2
        ota.set_boot_partition(Partition::Ota1).unwrap();
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
        ota.set_boot_partition(Partition::Ota1).unwrap();
        let detected = ota.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota1);

        // Switch back to ota_0 (slot 0 gets higher seq)
        ota.set_boot_partition(Partition::Ota0).unwrap();
        ota.set_boot_partition(Partition::Ota0).unwrap(); // seq increments again
        let detected = ota.detect_running_partition().unwrap();
        assert_eq!(detected, Partition::Ota0);
    }

    // ── Write offset / bytes_written tracking tests ────────────────────

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

    // ── CRC-32/MPEG-2 verification tests ───────────────────────────────

    #[test]
    fn test_crc32_mpeg2_known_vector() {
        // VAL-CORE-021: CRC-32/MPEG-2 of "123456789" must equal 0x0376E6E7.
        let data = b"123456789";
        let result = crc32(data);
        assert_eq!(
            result, 0x0376E6E7,
            "CRC-32/MPEG-2 of '123456789' should be 0x0376E6E7, got {:#010X}",
            result
        );
    }

    #[test]
    fn test_crc32_incremental_matches_oneshot() {
        // VAL-CORE-022: Incremental CRC across chunks must equal one-shot CRC
        // of concatenated data.
        let chunk1 = b"The quick brown ";
        let chunk2 = b"fox jumps over ";
        let chunk3 = b"the lazy dog";

        // One-shot
        let mut all = alloc::vec![];
        all.extend_from_slice(chunk1);
        all.extend_from_slice(chunk2);
        all.extend_from_slice(chunk3);
        let expected = crc32(&all);

        // Incremental
        let mut crc = 0xFFFFFFFFu32;
        crc = crc32_update(crc, chunk1);
        crc = crc32_update(crc, chunk2);
        crc = crc32_update(crc, chunk3);

        assert_eq!(
            crc, expected,
            "incremental CRC ({:#010X}) must match one-shot ({:#010X})",
            crc, expected
        );
    }
}
