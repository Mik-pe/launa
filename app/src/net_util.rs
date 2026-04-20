//! Shared network utilities.

extern crate alloc;

use embassy_net::{IpAddress, Stack, dns::DnsQueryType};
use log::warn;

/// Parse an IPv4 dotted-quad address string into `[u8; 4]`.
/// Rejects malformed input: wrong number of octets, out-of-range values,
/// or extra trailing data.
pub fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut parts = s.split('.');
    let a = parts.next()?.parse::<u8>().ok()?;
    let b = parts.next()?.parse::<u8>().ok()?;
    let c = parts.next()?.parse::<u8>().ok()?;
    let d = parts.next()?.parse::<u8>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some([a, b, c, d])
}

/// Compute exponential backoff in seconds for connection retries.
///
/// Returns 5s, 10s, 20s, 40s, 60s, 60s, ... capped at 60s.
/// `attempt` is 1-based (first attempt = 1).
pub fn backoff_secs(attempt: u32) -> u64 {
    (5u64 << attempt.saturating_sub(1).min(4)).min(60)
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
