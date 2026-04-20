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

pub mod crypto;
pub mod flash;
pub mod ota;

// Re-export public API
pub use crypto::{crc32, crc32_ota, crc32_update};
pub use flash::Partition;
pub use ota::EspOtaFlash;
