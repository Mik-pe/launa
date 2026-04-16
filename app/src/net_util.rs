//! Shared network utilities.

extern crate alloc;

use embassy_net::{IpAddress, Stack, dns::DnsQueryType};
use log::warn;

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

/// Resolve a hostname to an IPv4 address.
///
/// First tries parsing as a dotted-quad IPv4 (fast path, no network).
/// If that fails, performs a DNS A-record query via the network stack.
/// Returns `None` if resolution fails or times out.
pub async fn resolve_host(stack: &Stack<'static>, host: &str) -> Option<[u8; 4]> {
    // Fast path: already an IPv4 address
    if let Some(addr) = parse_ip(host) {
        return Some(addr);
    }

    // DNS resolution
    match stack.dns_query(host, DnsQueryType::A).await {
        Ok(addrs) => {
            if let Some(addr) = addrs.first() {
                // DnsQueryType::A always returns Ipv4 addresses
                let IpAddress::Ipv4(v4) = *addr;
                Some(v4.octets())
            } else {
                warn!("DNS: no A record found for '{}'", host);
                None
            }
        }
        Err(e) => {
            warn!("DNS: failed to resolve '{}': {:?}", host, e);
            None
        }
    }
}
