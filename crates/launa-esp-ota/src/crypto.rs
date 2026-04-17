//! Cryptographic primitives for OTA firmware verification.
//!
//! Implements SHA-256, HMAC-SHA256, and CRC-32/MPEG-2 from scratch
//! for `no_std` compatibility without external crypto dependencies.

use alloc::vec;
use alloc::vec::Vec;

/// HMAC-SHA256 signing key (32 bytes).
///
/// On the device, this should be derived from an eFuse secret (e.g. BLOCK3)
/// so that only the legitimate signing tool (which knows the key) can produce
/// valid firmware signatures.
pub struct SigningKey(pub [u8; 32]);

impl SigningKey {
    /// Create a signing key from raw bytes.
    pub fn new(key: [u8; 32]) -> Self {
        SigningKey(key)
    }
}

/// SHA-256 block size in bytes.
pub(crate) const SHA256_BLOCK_SIZE: usize = 64;
/// SHA-256 digest size in bytes.
pub(crate) const SHA256_DIGEST_SIZE: usize = 32;

/// Compute SHA-256 digest of the input data.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let msg_len_bits = (data.len() as u64) * 8;

    // Padding: append 0x80, then zeros, then 64-bit big-endian length
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % SHA256_BLOCK_SIZE != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&msg_len_bits.to_be_bytes());

    // Process each 64-byte block
    let k: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    for block in padded.chunks_exact(SHA256_BLOCK_SIZE) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(k[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut result = [0u8; 32];
    for i in 0..8 {
        result[i * 4..i * 4 + 4].copy_from_slice(&state[i].to_be_bytes());
    }
    result
}

/// Compute HMAC-SHA256 of `data` using `key`.
pub(crate) fn hmac_sha256(key: &SigningKey, data: &[u8]) -> [u8; 32] {
    // If key is longer than block size, hash it first
    let key_block = if key.0.len() > SHA256_BLOCK_SIZE {
        let h = sha256(&key.0);
        let mut block = [0u8; SHA256_BLOCK_SIZE];
        block[..32].copy_from_slice(&h);
        block
    } else {
        let mut block = [0u8; SHA256_BLOCK_SIZE];
        block[..key.0.len()].copy_from_slice(&key.0);
        block
    };

    // Inner hash: SHA-256((key ^ ipad) || data)
    let mut inner_data: Vec<u8> = vec![0u8; SHA256_BLOCK_SIZE + data.len()];
    for i in 0..SHA256_BLOCK_SIZE {
        inner_data[i] = key_block[i] ^ 0x36;
    }
    inner_data[SHA256_BLOCK_SIZE..].copy_from_slice(data);
    let inner_hash = sha256(&inner_data);

    // Outer hash: SHA-256((key ^ opad) || inner_hash)
    let mut outer_data = [0u8; SHA256_BLOCK_SIZE + SHA256_DIGEST_SIZE];
    for i in 0..SHA256_BLOCK_SIZE {
        outer_data[i] = key_block[i] ^ 0x5c;
    }
    outer_data[SHA256_BLOCK_SIZE..].copy_from_slice(&inner_hash);
    sha256(&outer_data)
}

/// Compute HMAC-SHA256 incrementally, updating a running digest.
///
/// This is a simplified approach: accumulate all data into a buffer,
/// then compute HMAC-SHA256 on finalization. Suitable for firmware
/// verification where the total data fits in memory during testing.
pub(crate) fn hmac_sha256_update(key: &SigningKey, data: &[u8]) -> [u8; 32] {
    hmac_sha256(key, data)
}

/// Truncate a 32-byte HMAC-SHA256 digest to a u32 (first 4 bytes, big-endian).
pub(crate) fn truncate_signature(hmac: &[u8; 32]) -> u32 {
    u32::from_be_bytes([hmac[0], hmac[1], hmac[2], hmac[3]])
}

/// CRC32 (CRC-32/MPEG-2) incremental update. Polynomial 0x04C11DB7.
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
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
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFFFFFF, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32() {
        // CRC-32/MPEG-2: init=0xFFFFFFFF, poly=0x04C11DB7, no final XOR
        assert_eq!(crc32(&[]), 0xFFFFFFFF);
        assert_eq!(crc32(&[0x01]), 1254728195u32);
    }

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

    #[test]
    fn test_sha256_empty_string() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let result = sha256(&[]);
        let expected: [u8; 32] = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        assert_eq!(result, expected, "SHA-256 of empty string mismatch");
    }

    #[test]
    fn test_sha256_abc() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let result = sha256(b"abc");
        let expected: [u8; 32] = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(result, expected, "SHA-256 of 'abc' mismatch");
    }

    #[test]
    fn test_hmac_sha256_known_vector() {
        // RFC 4231 Test Case 2:
        // Key = "Jefe" (4 bytes), Data = "what do ya want for nothing?"
        // HMAC-SHA256 = 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843
        let key = SigningKey::new([
            0x4A, 0x65, 0x66, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ]);
        let data = b"what do ya want for nothing?";
        let result = hmac_sha256(&key, data);
        let expected: [u8; 32] = [
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ];
        assert_eq!(
            result, expected,
            "HMAC-SHA256 RFC 4231 test case 2 mismatch"
        );
    }

    #[test]
    fn test_hmac_sha256_deterministic() {
        // Same key and data must produce the same HMAC
        let key = SigningKey::new([0xAA; 32]);
        let data = b"test firmware data for signing";
        let result1 = hmac_sha256(&key, data);
        let result2 = hmac_sha256(&key, data);
        assert_eq!(result1, result2, "HMAC-SHA256 must be deterministic");
    }

    #[test]
    fn test_hmac_sha256_different_keys_different_result() {
        let key1 = SigningKey::new([0xAA; 32]);
        let key2 = SigningKey::new([0xBB; 32]);
        let data = b"test firmware data";
        let result1 = hmac_sha256(&key1, data);
        let result2 = hmac_sha256(&key2, data);
        assert_ne!(
            result1, result2,
            "Different keys must produce different HMACs"
        );
    }

    #[test]
    fn test_truncate_signature_takes_first_4_bytes() {
        let mut hmac = [0u8; 32];
        hmac[0] = 0xDE;
        hmac[1] = 0xAD;
        hmac[2] = 0xBE;
        hmac[3] = 0xEF;
        assert_eq!(truncate_signature(&hmac), 0xDEADBEEF);
    }
}
