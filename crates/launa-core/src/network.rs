//! Network utility functions.
//!
//! Provides pure functions for IPv4 address parsing and exponential
//! backoff calculation. No network I/O — just data transformation
//! that can be tested on desktop.

/// Parse an IPv4 dotted-quad address string into `[u8; 4]`.
///
/// Rejects malformed input: wrong number of octets, out-of-range values,
/// or extra trailing data.
///
/// # Examples
/// ```
/// use launa_core::network::parse_ip;
/// assert_eq!(parse_ip("192.168.1.1"), Some([192, 168, 1, 1]));
/// assert_eq!(parse_ip("999.1.1.1"), None);
/// assert_eq!(parse_ip("1.2.3"), None);
/// ```
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
///
/// # Examples
/// ```
/// use launa_core::network::backoff_secs;
/// assert_eq!(backoff_secs(1), 5);
/// assert_eq!(backoff_secs(2), 10);
/// assert_eq!(backoff_secs(5), 60);
/// assert_eq!(backoff_secs(100), 60);
/// ```
pub fn backoff_secs(attempt: u32) -> u64 {
    (5u64 << attempt.saturating_sub(1).min(4)).min(60)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_ip tests ---

    #[test]
    fn test_parse_ip_valid() {
        assert_eq!(parse_ip("192.168.1.1"), Some([192, 168, 1, 1]));
        assert_eq!(parse_ip("0.0.0.0"), Some([0, 0, 0, 0]));
        assert_eq!(parse_ip("255.255.255.255"), Some([255, 255, 255, 255]));
        assert_eq!(parse_ip("10.0.0.1"), Some([10, 0, 0, 1]));
    }

    #[test]
    fn test_parse_ip_localhost() {
        assert_eq!(parse_ip("127.0.0.1"), Some([127, 0, 0, 1]));
    }

    #[test]
    fn test_parse_ip_too_few_octets() {
        assert_eq!(parse_ip("1.2.3"), None);
        assert_eq!(parse_ip("1.2"), None);
        assert_eq!(parse_ip("1"), None);
        assert_eq!(parse_ip(""), None);
    }

    #[test]
    fn test_parse_ip_too_many_octets() {
        assert_eq!(parse_ip("1.2.3.4.5"), None);
    }

    #[test]
    fn test_parse_ip_out_of_range() {
        assert_eq!(parse_ip("256.1.1.1"), None);
        assert_eq!(parse_ip("1.256.1.1"), None);
        assert_eq!(parse_ip("999.999.999.999"), None);
    }

    #[test]
    fn test_parse_ip_non_numeric() {
        assert_eq!(parse_ip("abc"), None);
        assert_eq!(parse_ip("1.2.3.abc"), None);
        assert_eq!(parse_ip("a.b.c.d"), None);
    }

    #[test]
    fn test_parse_ip_negative() {
        assert_eq!(parse_ip("-1.0.0.0"), None);
    }

    #[test]
    fn test_parse_ip_trailing_dot() {
        assert_eq!(parse_ip("1.2.3.4."), None);
    }

    #[test]
    fn test_parse_ip_leading_dot() {
        assert_eq!(parse_ip(".1.2.3.4"), None);
    }

    #[test]
    fn test_parse_ip_extra_whitespace() {
        assert_eq!(parse_ip(" 1.2.3.4"), None);
        assert_eq!(parse_ip("1.2.3.4 "), None);
    }

    // --- backoff_secs tests ---

    #[test]
    fn test_backoff_attempt_1() {
        assert_eq!(backoff_secs(1), 5);
    }

    #[test]
    fn test_backoff_attempt_2() {
        assert_eq!(backoff_secs(2), 10);
    }

    #[test]
    fn test_backoff_attempt_3() {
        assert_eq!(backoff_secs(3), 20);
    }

    #[test]
    fn test_backoff_attempt_4() {
        assert_eq!(backoff_secs(4), 40);
    }

    #[test]
    fn test_backoff_attempt_5_capped() {
        assert_eq!(backoff_secs(5), 60);
    }

    #[test]
    fn test_backoff_attempt_10_capped() {
        assert_eq!(backoff_secs(10), 60);
    }

    #[test]
    fn test_backoff_attempt_100_capped() {
        assert_eq!(backoff_secs(100), 60);
    }

    #[test]
    fn test_backoff_sequence() {
        let expected = [5, 10, 20, 40, 60, 60, 60];
        for (i, &exp) in expected.iter().enumerate() {
            assert_eq!(backoff_secs((i + 1) as u32), exp, "attempt {}", i + 1);
        }
    }

    #[test]
    fn test_backoff_zero_attempt() {
        // Edge case: attempt 0 should still return a value
        assert!(backoff_secs(0) > 0);
    }
}
