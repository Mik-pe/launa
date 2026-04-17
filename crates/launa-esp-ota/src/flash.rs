//! Flash partition operations and otadata management.
//!
//! Handles partition layout constants, sector-aligned reads/writes,
//! and otadata read-modify-write for safe dual-slot boot selection.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use launa_ota::OtaError;
use log::debug;

use crate::crypto::crc32;

// ── Partition table constants (must match app/partitions.csv) ──────────

pub(crate) const OTADATA_OFFSET: u32 = 0x10000;
pub(crate) const OTA_0_OFFSET: u32 = 0x160000;
pub(crate) const OTA_0_SIZE: u32 = 0x140000;
pub(crate) const OTA_1_OFFSET: u32 = 0x2A0000;
pub(crate) const OTA_1_SIZE: u32 = 0x140000;

pub(crate) const SECTOR_SIZE: u32 = 4096;
pub(crate) const WORD_SIZE: u32 = 4;

// OTA data entry: 32 bytes, two slots in otadata partition
pub(crate) const OTA_ENTRY_SIZE: usize = 32;

pub(crate) const OTA_SEQ_OFFSET: usize = 4;
pub(crate) const OTA_SEQ_SIZE: usize = 4;

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
pub(crate) fn read_otadata_sequences<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
) -> Result<(u32, u32), OtaError> {
    let mut buf = [0u8; OTA_ENTRY_SIZE * 2];
    ReadNorFlash::read(flash, OTADATA_OFFSET, &mut buf).map_err(|_| OtaError::FlashError {
        address: OTADATA_OFFSET,
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
pub(crate) fn set_boot_partition<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
    partition: Partition,
) -> Result<(), OtaError> {
    let (seq_0, seq_1) = read_otadata_sequences(flash)?;

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
    entry[OTA_SEQ_OFFSET..OTA_SEQ_OFFSET + OTA_SEQ_SIZE].copy_from_slice(&new_seq.to_be_bytes());

    let crc = crc32(&entry[4..]);
    entry[..4].copy_from_slice(&crc.to_le_bytes());

    let slot_offset = OTADATA_OFFSET + (partition.index() as u32 * OTA_ENTRY_SIZE as u32);

    // Both otadata slots share the same 4 KiB sector (slot 0 at 0x10000,
    // slot 1 at 0x10020).  A naive erase-then-write would destroy the
    // companion slot on power loss.  Use a read-modify-write cycle so the
    // other slot's data is preserved across the erase.
    let sector_base = slot_offset & !(SECTOR_SIZE - 1);
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
/// Returns the partition with the higher sequence number.
pub(crate) fn detect_running_partition<S: NorFlash + ReadNorFlash>(
    flash: &mut S,
) -> Result<Partition, OtaError> {
    let (seq_0, seq_1) = read_otadata_sequences(flash)?;
    Ok(if seq_1 > seq_0 {
        Partition::Ota1
    } else {
        Partition::Ota0
    })
}
