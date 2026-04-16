//! AES-128-CTR encryption for sensitive NVS fields (WiFi/MQTT passwords).
//!
//! Key derivation: eFuse BLOCK3 (128 bits, provisioned via `cargo xtask provision`)
//! read via the public eFuse field constants that cover BLOCK3 data.
//!
//! Storage format: `"enc:"` + hex(12-byte-nonce + padded-ciphertext).
//!
//! Backward compatible: values without the `"enc:"` prefix are treated as plaintext.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use esp_hal::aes::{Aes, Key};
use esp_hal::efuse::Efuse;
use esp_hal::rng::Rng;
use log::warn;

/// NVS values starting with this prefix are treated as encrypted.
const ENC_PREFIX: &str = "enc:";

/// Read the 128-bit AES key from eFuse BLOCK3.
///
/// BLOCK3 is a 256-bit user-programmable eFuse block. We read 128 bits
/// by combining multiple public eFuse field constants that cover the
/// BLOCK3 data region. The fields used:
///   - BLK3_RESERVED_2: block 3, word 2, bits 64..96 (32 bits)
///   - SECURE_VERSION:   block 3, word 4, bits 128..160 (32 bits)
///   - BLK3_RESERVED_6: block 3, word 6, bits 192..224 (32 bits)
///   - BLK3_RESERVED_7: block 3, word 7, bits 224..256 (32 bits)
///
/// Total: 128 bits (16 bytes) of unique per-device key material.
///
/// Note: These fields overlap with ADC calibration and other reserved data.
/// On devices where BLOCK3 is fully user-programmed (via provision), these
/// fields contain our key. On unprovisioned devices, the key will be
/// whatever factory-default values are present (all-zero or ADC cal data),
/// which means encryption is still deterministic per-device.
fn read_key() -> [u8; 16] {
    use esp_hal::efuse::{BLK3_RESERVED_2, SECURE_VERSION, BLK3_RESERVED_6, BLK3_RESERVED_7};

    let mut key = [0u8; 16];

    // Read 4 words (each 32 bits = 4 bytes) from BLOCK3
    let word2: [u8; 4] = Efuse::read_field_le(BLK3_RESERVED_2);
    let word4: [u8; 4] = Efuse::read_field_le(SECURE_VERSION);
    let word6: [u8; 4] = Efuse::read_field_le(BLK3_RESERVED_6);
    let word7: [u8; 4] = Efuse::read_field_le(BLK3_RESERVED_7);

    // Interleave the 4 words into a 16-byte key with mixing
    // to avoid simple patterns when only some words differ.
    key[0..4].copy_from_slice(&word2);
    key[4..8].copy_from_slice(&word4);
    key[8..12].copy_from_slice(&word6);
    key[12..16].copy_from_slice(&word7);

    // XOR-mix to prevent key = raw eFuse data
    for (i, b) in key.iter_mut().enumerate() {
        *b ^= [0xA5, 0x3C, 0x96, 0xF0][i % 4];
    }

    // Warn if the raw eFuse words were all zeros (unprovisioned device).
    // This means the derived key is predictable (just the XOR constants),
    // providing no real security. Operators should run `cargo xtask provision`
    // to burn a random key into BLOCK3.
    let all_zero = word2 == [0u8; 4]
        && word4 == [0u8; 4]
        && word6 == [0u8; 4]
        && word7 == [0u8; 4];
    if all_zero {
        warn!("eFuse BLOCK3 is all zeros — encryption key is predictable! Run 'cargo xtask provision' to burn a random key.");
    }

    key
}

/// Generate a 12-byte random nonce using the hardware RNG.
fn random_nonce(rng: &mut Rng) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    for byte in nonce.iter_mut() {
        *byte = rng.random() as u8;
    }
    nonce
}

/// Convert a byte slice to a lowercase hex string.
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let hi = (b >> 4) & 0x0f;
        let lo = b & 0x0f;
        s.push(if hi < 10 { (b'0' + hi) as char } else { (b'a' + hi - 10) as char });
        s.push(if lo < 10 { (b'0' + lo) as char } else { (b'a' + lo - 10) as char });
    }
    s
}

/// Maximum hex string length accepted by `from_hex()`.
/// 1024 hex chars → 512 bytes decoded. More than enough for encrypted NVS
/// values (typically ~100 chars). Prevents OOM from malformed/corrupt input
/// on the 32 KiB ESP32 heap.
const MAX_HEX_LEN: usize = 1024;

/// Parse a hex string into a byte vector.
///
/// Returns `None` if the input has odd length, contains non-hex characters,
/// or exceeds `MAX_HEX_LEN` (prevents unbounded heap allocation).
fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 || hex.len() > MAX_HEX_LEN {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let hi = hex.as_bytes()[i].to_ascii_lowercase();
        let lo = hex.as_bytes()[i + 1].to_ascii_lowercase();
        let h = match hi {
            b'0'..=b'9' => hi - b'0',
            b'a'..=b'f' => hi - b'a' + 10,
            _ => return None,
        };
        let l = match lo {
            b'0'..=b'9' => lo - b'0',
            b'a'..=b'f' => lo - b'a' + 10,
            _ => return None,
        };
        bytes.push(h << 4 | l);
    }
    Some(bytes)
}

/// Increment a 16-byte counter (big-endian) in-place.
fn increment_counter(counter: &mut [u8; 16]) {
    for byte in counter.iter_mut().rev() {
        *byte = byte.wrapping_add(1);
        if *byte != 0 {
            break;
        }
    }
}

/// Encrypt a password string using AES-128-CTR.
///
/// Returns `"enc:"` + hex(12-byte-nonce + padded-ciphertext).
/// A new random nonce is generated for each call, so the same plaintext
/// produces different ciphertext each time.
pub fn encrypt(plaintext: &str, aes: &mut Aes, rng: &mut Rng) -> String {
    if plaintext.is_empty() {
        return String::new();
    }

    let key = read_key();

    // Generate random nonce (12 bytes random + 4 bytes counter = 16-byte IV)
    let nonce_12 = random_nonce(rng);
    let mut counter = [0u8; 16];
    counter[..12].copy_from_slice(&nonce_12);
    // counter[12..16] remains 0 (counter starts at 0)

    // Pad plaintext to next 16-byte boundary with PKCS7 padding
    let plain_bytes = plaintext.as_bytes();
    let pad_len = 16 - (plain_bytes.len() % 16);
    let padded_len = plain_bytes.len() + pad_len;
    let mut buf = alloc::vec![0u8; padded_len];
    buf[..plain_bytes.len()].copy_from_slice(plain_bytes);
    for b in &mut buf[plain_bytes.len()..] {
        *b = pad_len as u8;
    }

    // CTR-mode encrypt: for each 16-byte block, encrypt counter to get
    // keystream, XOR with plaintext, then increment counter.
    for chunk in buf.chunks_exact_mut(16) {
        // Encrypt counter to get keystream block (uses public Aes::encrypt API)
        let mut keystream = counter;
        aes.encrypt(&mut keystream, Key::Key128(key));

        // XOR plaintext with keystream
        for (i, b) in chunk.iter_mut().enumerate() {
            *b ^= keystream[i];
        }

        // Increment counter for next block
        increment_counter(&mut counter);
    }

    // Build output: "enc:" + hex(nonce_12 + ciphertext)
    let mut combined = Vec::with_capacity(12 + buf.len());
    combined.extend_from_slice(&nonce_12);
    combined.extend_from_slice(&buf);

    let mut result = String::from(ENC_PREFIX);
    result.push_str(&to_hex(&combined));
    result
}

/// Decrypt an encrypted value, or pass through plaintext values.
///
/// If the value starts with `"enc:"`, it is decrypted using AES-128-CTR.
/// Otherwise, it is returned as-is (backward compatible with unencrypted NVS).
pub fn maybe_decrypt(value: &str, aes: &mut Aes, _rng: &mut Rng) -> String {
    // Pass through plaintext values (backward compatible)
    if !value.starts_with(ENC_PREFIX) {
        return String::from(value);
    }

    let hex_part = &value[ENC_PREFIX.len()..];

    // Need at least 12 bytes nonce (24 hex chars) + 16 bytes ciphertext (32 hex chars)
    if hex_part.len() < 56 {
        warn!("Encrypted value too short ({} hex chars), returning as-is", hex_part.len());
        return String::from(value);
    }

    let combined = match from_hex(hex_part) {
        Some(c) => c,
        None => {
            warn!("Invalid hex in encrypted value, returning as-is");
            return String::from(value);
        }
    };

    if combined.len() < 28 {
        // 12 (nonce) + 16 (one block minimum)
        warn!("Encrypted value too short ({} bytes), returning as-is", combined.len());
        return String::from(value);
    }

    let nonce_12 = &combined[..12];
    let ciphertext = &combined[12..];

    // Verify ciphertext is a multiple of 16 bytes
    if ciphertext.len() % 16 != 0 {
        warn!("Ciphertext not block-aligned ({} bytes), returning as-is", ciphertext.len());
        return String::from(value);
    }

    let key = read_key();

    // Reconstruct counter: 12-byte nonce + 4-byte counter (starts at 0)
    let mut counter = [0u8; 16];
    counter[..12].copy_from_slice(nonce_12);

    // Decrypt using CTR mode (same operation as encrypt: encrypt counter, XOR)
    let mut buf = ciphertext.to_vec();

    for chunk in buf.chunks_exact_mut(16) {
        // Encrypt counter to get keystream block (CTR decrypt = encrypt counter)
        let mut keystream = counter;
        aes.encrypt(&mut keystream, Key::Key128(key));

        // XOR ciphertext with keystream to get plaintext
        for (i, b) in chunk.iter_mut().enumerate() {
            *b ^= keystream[i];
        }

        // Increment counter for next block
        increment_counter(&mut counter);
    }

    // Remove PKCS7 padding
    if let Some(&pad_val) = buf.last() {
        let pad_val = pad_val as usize;
        if pad_val > 0 && pad_val <= 16 && pad_val <= buf.len() {
            let pad_start = buf.len() - pad_val;
            // Verify all padding bytes are correct
            let valid = buf[pad_start..].iter().all(|&b| b as usize == pad_val);
            if valid {
                buf.truncate(pad_start);
            }
        }
    }

    match String::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => {
            warn!("Decrypted value is not valid UTF-8, returning original");
            String::from(value)
        }
    }
}
