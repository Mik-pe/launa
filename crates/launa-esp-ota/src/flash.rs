//! Flash partition operations and otadata management.
//!
//! Handles partition layout constants, sector-aligned reads/writes,
//! and otadata read-modify-write for safe dual-slot boot selection.
//!
//! # ESP-IDF otadata format
//!
//! Each otadata entry is 32 bytes (`esp_ota_select_entry_t`):
//! ```text
//! bytes  0..3 : ota_seq    (uint32_t, little-endian)
//! bytes  4..23: seq_label  (20 bytes, unused/padding)
//! bytes 24..27: ota_state  (uint32_t, little-endian)
//! bytes 28..31: crc        (uint32_t, CRC-32/ISO-HDLP of ota_seq only)
//! ```
//!
//! The bootloader validates: `entry.crc == esp_rom_crc32_le(0xFFFFFFFF, &entry.ota_seq, 4)`
//! Two copies reside in separate 4 KiB sectors within the otadata partition.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use launa_ota::OtaError;
use log::debug;

use crate::crypto::crc32_ota;

pub(crate) const OTADATA_OFFSET: u32 = 0x10000;
pub(crate) const OTA_0_OFFSET: u32 = 0x160000;
pub(crate) const OTA_0_SIZE: u32 = 0x140000;
pub(crate) const OTA_1_OFFSET: u32 = 0x2A0000;
pub(crate) const OTA_1_SIZE: u32 = 0x140000;

pub(crate) const SECTOR_SIZE: u32 = 4096;
pub(crate) const WORD_SIZE: u32 = 4;

// OTA data entry: 32 bytes (esp_ota_select_entry_t)
pub(crate) const OTA_ENTRY_SIZE: usize = 32;

// Offsets within esp_ota_select_entry_t
pub(crate) const OTA_SEQ_OFFSET: usize = 0;
pub(crate) const OTA_SEQ_SIZE: usize = 4;
pub(crate) const OTA_LABEL_OFFSET: usize = 4;
pub(crate) const OTA_LABEL_SIZE: usize = 20;
pub(crate) const OTA_STATE_OFFSET: usize = 24;
pub(crate) const OTA_STATE_SIZE: usize = 4;
pub(crate) const OTA_CRC_OFFSET: usize = 28;
pub(crate) const OTA_CRC_SIZE: usize = 4;

/// Boot partition identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Partition {
    Ota0,
    Ota1,
}

impl Partition {
    pub(crate) fn offset(&self) -> u32 {
        match self {
            Partition::Ota0 => OTA_0_OFFSET,
            Partition::Ota1 => OTA_1_OFFSET,
        }
    }

    pub(crate) fn size(&self) -> u32 {
        match self {
            Partition::Ota0 => OTA_0_SIZE,
            Partition::Ota1 => OTA_1_SIZE,
        }
    }

    pub(crate) fn index(&self) -> usize {
        match self {
            Partition::Ota0 => 0,
            Partition::Ota1 => 1,
        }
    }
}

/// Read a little-endian u32 from 4 bytes.
pub(crate) fn u32_from_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Read a big-endian u32 from 4 bytes.
pub(crate) fn u32_from_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Erase sectors in the target partition from `start` to `end` (byte offsets
/// relative to partition start).
pub(crate) fn erase_range<S: NorFlash>(
    flash: &mut S,
    target: Partition,
    start: u32,
    end: u32,
) -> Result<(), OtaError> {
    let base = target.offset();
    let aligned_start = (start / SECTOR_SIZE) * SECTOR_SIZE;
    let aligned_end = ((end + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;

    let mut offset = aligned_start;
    while offset < aligned_end {
        let abs_offset = base + offset;
        debug!("Erasing sector at 0x{:08X}", abs_offset);
        let addr = abs_offset;
        flash
            .erase(abs_offset, abs_offset + SECTOR_SIZE)
            .map_err(|_| OtaError::FlashError { address: addr })?;
        offset += SECTOR_SIZE;
    }
    Ok(())
}

/// Align a buffer to word boundary (4 bytes) by padding with 0xFF.
pub(crate) fn aligned_write<S: NorFlash>(
    flash: &mut S,
    target: Partition,
    offset: u32,
    data: &[u8],
) -> Result<(), OtaError> {
    let abs_offset = target.offset() + offset;

    if data.len() % WORD_SIZE as usize == 0 {
        flash
            .write(abs_offset, data)
            .map_err(|_| OtaError::FlashError {
                address: abs_offset,
            })?;
    } else {
        let pad_len = (WORD_SIZE as usize - (data.len() % WORD_SIZE as usize)) % WORD_SIZE as usize;
        let mut padded = alloc::vec![0xFFu8; data.len() + pad_len];
        padded[..data.len()].copy_from_slice(data);
        flash
            .write(abs_offset, &padded)
            .map_err(|_| OtaError::FlashError {
                address: abs_offset,
            })?;
    }
    Ok(())
}

/// Read the sequence numbers from the otadata partition.
/// Returns (seq_0, seq_1) where each is 0 if the slot is empty/erased.
///
/// The two otadata entries reside in separate 4 KiB sectors:
///   slot 0 at OTADATA_OFFSET
///   slot 1 at OTADATA_OFFSET + SECTOR_SIZE
pub(crate) fn read_otadata_sequences<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
) -> Result<(u32, u32), OtaError> {
    // Each otadata entry is in its own 4 KiB sector
    let mut buf0 = [0u8; OTA_ENTRY_SIZE];
    ReadNorFlash::read(flash, OTADATA_OFFSET, &mut buf0).map_err(|_| OtaError::FlashError {
        address: OTADATA_OFFSET,
    })?;

    let slot1_offset = OTADATA_OFFSET + SECTOR_SIZE;
    let mut buf1 = [0u8; OTA_ENTRY_SIZE];
    ReadNorFlash::read(flash, slot1_offset, &mut buf1).map_err(|_| OtaError::FlashError {
        address: slot1_offset,
    })?;

    // Sequence number is at offset 0, little-endian
    let raw_0 = u32_from_le(&buf0[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
    let raw_1 = u32_from_le(&buf1[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);

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
///
/// Format matches ESP-IDF `esp_ota_select_entry_t`:
/// - ota_seq at bytes 0-3 (little-endian)
/// - seq_label at bytes 4-23 (zeroed)
/// - ota_state at bytes 24-27 (0xFFFFFFFF = undefined/valid)
/// - crc at bytes 28-31 (CRC-32/ISO-HDLP of ota_seq only)
pub(crate) fn set_boot_partition<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
    partition: Partition,
) -> Result<(), OtaError> {
    let (seq_0, seq_1) = read_otadata_sequences(flash)?;

    // ESP-IDF bootloader maps ota_seq to partition via:
    //   ota_slot = (ota_seq - 1) % app_count
    // So Ota0 needs odd seq (1,3,5...), Ota1 needs even seq (2,4,6...).
    // We must find the right sequence number for the target partition.
    let max_seq = seq_0.max(seq_1);
    let target_index = partition.index() as u32;

    // Find the smallest seq > max_seq that maps to target_index
    let new_seq = if max_seq == 0 {
        // No previous sequences — start with 1 (maps to Ota0) or 2 (maps to Ota1)
        target_index + 1
    } else {
        // Start searching from max_seq + 1
        let mut candidate = max_seq + 1;
        while (candidate - 1) % 2 != target_index {
            candidate += 1;
        }
        candidate
    };

    // Ensure it maps correctly (paranoia check)
    debug_assert_eq!((new_seq - 1) % 2, target_index);

    // Build esp_ota_select_entry_t (32 bytes)
    let mut entry = [0xFFu8; OTA_ENTRY_SIZE];
    // ota_seq at bytes 0-3 (little-endian)
    entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE].copy_from_slice(&new_seq.to_le_bytes());
    // seq_label at bytes 4-23: leave as 0xFF (erased)
    // ota_state at bytes 24-27: leave as 0xFFFFFFFF (ESP_OTA_IMG_UNDEFINED)
    // crc at bytes 28-31: CRC-32/ISO-HDLP of ota_seq (4 bytes)
    let crc = crc32_ota(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
    entry[OTA_CRC_OFFSET..OTA_CRC_OFFSET + OTA_CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

    // Each otadata slot is in its own 4 KiB sector
    let slot_offset = OTADATA_OFFSET + (partition.index() as u32 * SECTOR_SIZE);

    // Read-modify-write the sector to preserve any other data
    let sector_base = slot_offset; // slot is at the start of its sector
    let mut sector_buf = [0xFFu8; SECTOR_SIZE as usize];
    ReadNorFlash::read(flash, sector_base, &mut sector_buf).map_err(|_| OtaError::FlashError {
        address: sector_base,
    })?;

    let entry_offset = (slot_offset - sector_base) as usize;
    sector_buf[entry_offset..entry_offset + OTA_ENTRY_SIZE].copy_from_slice(&entry);

    flash
        .erase(sector_base, sector_base + SECTOR_SIZE)
        .map_err(|_| OtaError::FlashError {
            address: sector_base,
        })?;

    flash
        .write(sector_base, &sector_buf)
        .map_err(|_| OtaError::FlashError {
            address: sector_base,
        })?;

    Ok(())
}

/// Determine which partition is currently booted by reading otadata.
///
/// The bootloader picks the otadata slot with the higher valid sequence number,
/// then maps it to a partition via `(ota_seq - 1) % 2`.
pub(crate) fn detect_running_partition<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
) -> Result<Partition, OtaError> {
    let (seq_0, seq_1) = read_otadata_sequences(flash)?;
    // The bootloader picks the slot with the higher sequence.
    // We assume the CRC is valid (otherwise the device wouldn't have booted).
    let running_seq = if seq_1 > seq_0 { seq_1 } else { seq_0 };
    if running_seq == 0 {
        // No valid otadata — default to Ota0
        return Ok(Partition::Ota0);
    }
    let slot = (running_seq - 1) % 2;
    Ok(if slot == 0 {
        Partition::Ota0
    } else {
        Partition::Ota1
    })
}
