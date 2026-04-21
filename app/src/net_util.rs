//! Shared network utilities.

extern crate alloc;

use alloc::vec::Vec;
use embassy_net::{IpAddress, Stack, dns::DnsQueryType};
use embassy_time::{Duration, Instant, Timer};
use log::{error, info, warn};

use crate::mqtt_client::{LastState, MqttClient};

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

/// Reconnect to MQTT with exponential backoff, alerting, and post-reconnect sync.
///
/// On successful reconnect, re-publishes availability, discovery entities,
/// subscribes to command topics, and optionally re-publishes the last known state.
///
/// Retries up to 30 attempts with exponential backoff (5s–60s). Sends throttled
/// alerts after 3 failed attempts. Resets the device after 30 failures.
pub async fn reconnect_with_backoff<'a>(
    mqtt: &mut MqttClient,
    celsius: bool,
    last_state: Option<&LastState<'a>>,
    alert_message: &str,
    reset_message: &str,
) {
    let mut attempt: u32 = 0;
    let mut last_alert_time: Option<Instant> = None;
    loop {
        attempt += 1;
        match mqtt.reconnect_and_sync(celsius, last_state).await {
            Ok(()) => {
                info!("MQTT reconnected and synced (attempt {})", attempt);
                break;
            }
            Err(e) => {
                let backoff_secs = backoff_secs(attempt);
                error!(
                    "MQTT reconnect attempt {} failed: {:?}, retrying in {}s",
                    attempt, e, backoff_secs
                );
                // Publish alert after 3 attempts, throttled to once per 60s
                if attempt > 3 {
                    let now = Instant::now();
                    let should_alert = last_alert_time
                        .map(|t| t.elapsed() >= Duration::from_secs(60))
                        .unwrap_or(true);
                    if should_alert {
                        let json = alloc::format!(
                            r#"{{"level":"error","message":"{}","attempts":{},"timestamp":{}}}"#,
                            alert_message,
                            attempt,
                            crate::uptime_secs()
                        );
                        let payload = Vec::from(json.as_bytes());
                        let _ = crate::ALERT_CHANNEL.try_send(payload);
                        last_alert_time = Some(now);
                    }
                }
                if attempt >= 30 {
                    error!("{}", reset_message);
                    esp_hal::system::software_reset();
                }
                Timer::after(Duration::from_secs(backoff_secs)).await;
            }
        }
    }
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
