//! Shared test utilities for launa-protocol integration tests.

/// Xorshift32 PRNG — simple deterministic random number generator.
pub fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Generate `len` random bytes using the xorshift32 PRNG.
///
/// If `avoid_frame_chars` is true, bytes 0x7E (frame marker) and 0x7D (escape char)
/// are replaced with 0x7F. Use this for property tests that need "clean" payloads.
/// For fuzz tests that exercise edge cases, pass false to allow all byte values.
pub fn random_bytes(state: &mut u32, len: usize, avoid_frame_chars: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        let mut b = xorshift32(state) as u8;
        if avoid_frame_chars && (b == 0x7E || b == 0x7D) {
            b = 0x7F;
        }
        out.push(b);
    }
    out
}
