//! Shared network utilities.

extern crate alloc;

/// Parse an IPv4 dotted-quad address string into `[u8; 4]`.
/// Rejects malformed input: wrong number of octets, out-of-range values,
/// or extra trailing data.
pub fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let parts: alloc::vec::Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let a = parts[0].parse::<u8>().ok()?;
    let b = parts[1].parse::<u8>().ok()?;
    let c = parts[2].parse::<u8>().ok()?;
    let d = parts[3].parse::<u8>().ok()?;
    Some([a, b, c, d])
}
