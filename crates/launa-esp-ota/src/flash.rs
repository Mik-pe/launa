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
#[allow(dead_code)]
pub(crate) const OTA_LABEL_OFFSET: usize = 4;
#[allow(dead_code)]
pub(crate) const OTA_LABEL_SIZE: usize = 20;
#[allow(dead_code)]
pub(crate) const OTA_STATE_OFFSET: usize = 24;
#[allow(dead_code)]
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
#[allow(dead_code)]
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
    let aligned_end = end.div_ceil(SECTOR_SIZE) * SECTOR_SIZE;

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

    if data.len().is_multiple_of(WORD_SIZE as usize) {
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

/// Read a single otadata entry from flash.
fn read_otadata_entry<S: ReadNorFlash>(
    flash: &mut S,
    slot: usize,
) -> Result<[u8; OTA_ENTRY_SIZE], OtaError> {
    let offset = OTADATA_OFFSET + slot as u32 * SECTOR_SIZE;
    let mut buf = [0u8; OTA_ENTRY_SIZE];
    ReadNorFlash::read(flash, offset, &mut buf).map_err(|_| OtaError::FlashError {
        address: offset,
    })?;
    Ok(buf)
}

/// Check if an otadata entry has a valid CRC.
/// Matches ESP-IDF `bootloader_common_ota_select_valid`: CRC must match and
/// ota_seq must not be 0xFFFFFFFF (erased).
pub(crate) fn otadata_entry_valid(entry: &[u8; OTA_ENTRY_SIZE]) -> bool {
    let raw_seq = u32_from_le(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
    if raw_seq == 0xFFFFFFFF {
        return false;
    }
    let stored_crc = u32_from_le(&entry[OTA_CRC_OFFSET..OTA_CRC_OFFSET + OTA_CRC_SIZE]);
    let computed_crc = crc32_ota(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
    stored_crc == computed_crc
}

/// Extract the sequence number from an otadata entry, treating erased (0xFFFFFFFF) as 0.
pub(crate) fn seq_from_entry(entry: &[u8; OTA_ENTRY_SIZE]) -> u32 {
    let raw = u32_from_le(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
    if raw == 0xFFFFFFFF {
        0
    } else {
        raw
    }
}

/// Read both otadata entries with CRC validation.
/// Returns `[(entry, valid), (entry, valid)]` for slots 0 and 1.
pub(crate) fn read_otadata_entries<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
) -> Result<([u8; OTA_ENTRY_SIZE], bool, [u8; OTA_ENTRY_SIZE], bool), OtaError> {
    let entry0 = read_otadata_entry(flash, 0)?;
    let entry1 = read_otadata_entry(flash, 1)?;
    Ok((entry0, otadata_entry_valid(&entry0), entry1, otadata_entry_valid(&entry1)))
}

/// Read the sequence numbers from the otadata partition.
/// Returns (seq_0, seq_1) where each is 0 if the slot is empty/erased/invalid CRC.
///
/// The two otadata entries reside in separate 4 KiB sectors:
///   slot 0 at OTADATA_OFFSET
///   slot 1 at OTADATA_OFFSET + SECTOR_SIZE
pub(crate) fn read_otadata_sequences<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
) -> Result<(u32, u32), OtaError> {
    let (entry0, valid0, entry1, valid1) = read_otadata_entries(flash)?;
    let seq_0 = if valid0 { seq_from_entry(&entry0) } else { 0 };
    let seq_1 = if valid1 { seq_from_entry(&entry1) } else { 0 };
    Ok((seq_0, seq_1))
}

/// Build a 32-byte `esp_ota_select_entry_t` for the given sequence number.
fn build_otadata_entry(seq: u32) -> [u8; OTA_ENTRY_SIZE] {
    let mut entry = [0xFFu8; OTA_ENTRY_SIZE];
    entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE].copy_from_slice(&seq.to_le_bytes());
    let crc = crc32_ota(&entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE]);
    entry[OTA_CRC_OFFSET..OTA_CRC_OFFSET + OTA_CRC_SIZE].copy_from_slice(&crc.to_le_bytes());
    entry
}

/// Write an otadata entry to a specific slot via read-modify-write.
fn write_otadata_slot<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
    slot: usize,
    entry: &[u8; OTA_ENTRY_SIZE],
) -> Result<(), OtaError> {
    let slot_offset = OTADATA_OFFSET + slot as u32 * SECTOR_SIZE;
    let mut sector_buf = [0xFFu8; SECTOR_SIZE as usize];
    ReadNorFlash::read(flash, slot_offset, &mut sector_buf).map_err(|_| OtaError::FlashError {
        address: slot_offset,
    })?;
    sector_buf[..OTA_ENTRY_SIZE].copy_from_slice(entry);
    flash
        .erase(slot_offset, slot_offset + SECTOR_SIZE)
        .map_err(|_| OtaError::FlashError {
            address: slot_offset,
        })?;
    flash
        .write(slot_offset, &sector_buf)
        .map_err(|_| OtaError::FlashError {
            address: slot_offset,
        })?;
    Ok(())
}

/// Write an otadata entry to set the boot partition.
///
/// The bootloader compares sequence numbers and boots from the slot
/// with the higher value. We increment the target slot's sequence.
///
/// If the other otadata slot is empty/invalid, we also seed it with a valid
/// entry for the complementary partition. This ensures both slots are always
/// valid after an OTA, so a single-sector corruption cannot leave both invalid
/// (which would cause a silent fallback to the factory partition).
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
    let (entry0, valid0, entry1, valid1) = read_otadata_entries(flash)?;
    let seq_0 = if valid0 { seq_from_entry(&entry0) } else { 0 };
    let seq_1 = if valid1 { seq_from_entry(&entry1) } else { 0 };

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

    // Write the target partition's entry to its otadata slot
    let target_slot = partition.index();
    let target_entry = build_otadata_entry(new_seq);
    write_otadata_slot(flash, target_slot, &target_entry)?;

    // Seed the other slot if it's empty/invalid, so both slots are always valid.
    let other_slot = 1 - target_slot;
    let other_valid = if other_slot == 0 { valid0 } else { valid1 };
    if !other_valid {
        // Find a sequence for the complementary partition that is lower than new_seq
        let other_index = other_slot as u32;
        let other_seq = if max_seq == 0 {
            // First ever OTA: other partition has no prior entry.
            // Use the complementary starting seq (1 for Ota0, 2 for Ota1).
            other_index + 1
        } else {
            // Use the previous max seq (which mapped to the other partition or
            // find a seq < new_seq for the other partition).
            let mut candidate = new_seq - 1;
            while candidate > 0 && (candidate - 1) % 2 != other_index {
                candidate -= 1;
            }
            if candidate == 0 {
                other_index + 1
            } else {
                candidate
            }
        };
        let other_entry = build_otadata_entry(other_seq);
        write_otadata_slot(flash, other_slot, &other_entry)?;
    }

    Ok(())
}

/// Determine which partition is currently booted by reading otadata.
///
/// Only considers slots with valid CRCs, matching the ESP-IDF bootloader behavior.
/// Picks the slot with the higher valid sequence number, then maps it to a
/// partition via `(ota_seq - 1) % 2`.
pub(crate) fn detect_running_partition<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
) -> Result<Partition, OtaError> {
    let (seq_0, seq_1) = read_otadata_sequences(flash)?;
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
