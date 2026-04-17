//! OTA URL parsing and validation.
//!
//! Parses a JSON-like payload containing a `"url"` field and validates
//! that the URL uses the `http://` scheme. Extracted from the ESP32
//! mqtt_client module so it can be tested on desktop.

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::string::String;

/// Parse an OTA URL from a JSON-like MQTT payload.
///
/// The payload is expected to contain a `"url"` key with a string value,
/// e.g. `{"url":"http://192.168.1.100/firmware.bin"}`.
///
/// # Validation
///
/// - Only `http://` scheme is accepted.
/// - Empty payloads, missing `"url"` key, non-UTF-8, and other schemes return `None`.
///
/// # Whitespace tolerance
///
/// The parser tolerates whitespace around the colon and quotes, matching
/// real-world JSON from Home Assistant automations.
pub fn parse_ota_url(payload: &[u8]) -> Option<String> {
    let s = core::str::from_utf8(payload).ok()?;
    let mut search_from = 0;
    while let Some(pos) = s[search_from..].find("\"url\"") {
        let abs_pos = search_from + pos;
        // Reject matches inside longer keys like "callback_url" or "image_url"
        if abs_pos > 0 {
            let ch_before = s.as_bytes()[abs_pos - 1];
            if ch_before == b'_' || ch_before.is_ascii_alphanumeric() {
                search_from = abs_pos + 5;
                continue;
            }
        }
        let after_key = &s[abs_pos + 5..];
        let after_key = after_key.trim_start();
        let after_key = after_key.strip_prefix(':')?;
        let after_key = after_key.trim_start();
        let after_key = after_key.strip_prefix('"')?;
        if let Some(end) = after_key.find('"') {
            let url = &after_key[..end];
            // Validate URL scheme — only http:// is allowed for OTA.
            if let Some(scheme_end) = url.find("://") {
                let scheme = &url[..scheme_end];
                if scheme != "http" {
                    return None;
                }
            } else if url.find(':').map_or(false, |i| i < 8) {
                // Matches patterns like "data:..." without "://"
                return None;
            } else {
                // No scheme at all — reject
                return None;
            }
            return Some(String::from(url));
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ota_url_valid() {
        let payload = br#"{"url":"http://192.168.1.100/firmware.bin"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from("http://192.168.1.100/firmware.bin"))
        );
    }

    #[test]
    fn test_parse_ota_url_valid_with_port() {
        let payload = br#"{"url":"http://10.0.0.1:8080/fw.bin"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from("http://10.0.0.1:8080/fw.bin"))
        );
    }

    #[test]
    fn test_parse_ota_url_valid_with_query() {
        let payload = br#"{"url":"http://example.com/fw.bin?crc=abcd1234"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from("http://example.com/fw.bin?crc=abcd1234"))
        );
    }

    #[test]
    fn test_parse_ota_url_reject_empty() {
        assert_eq!(parse_ota_url(b""), None);
    }

    #[test]
    fn test_parse_ota_url_reject_empty_json() {
        assert_eq!(parse_ota_url(b"{}"), None);
    }

    #[test]
    fn test_parse_ota_url_reject_missing_url_key() {
        assert_eq!(parse_ota_url(br#"{"command":"pump1"}"#), None);
    }

    #[test]
    fn test_parse_ota_url_reject_https() {
        assert_eq!(
            parse_ota_url(br#"{"url":"https://example.com/fw.bin"}"#),
            None
        );
    }

    #[test]
    fn test_parse_ota_url_reject_ftp() {
        assert_eq!(
            parse_ota_url(br#"{"url":"ftp://example.com/fw.bin"}"#),
            None
        );
    }

    #[test]
    fn test_parse_ota_url_reject_file_scheme() {
        assert_eq!(parse_ota_url(br#"{"url":"file:///etc/passwd"}"#), None);
    }

    #[test]
    fn test_parse_ota_url_reject_data_scheme() {
        assert_eq!(parse_ota_url(br#"{"url":"data:text/plain,hello"}"#), None);
    }

    #[test]
    fn test_parse_ota_url_reject_no_scheme() {
        assert_eq!(parse_ota_url(br#"{"url":"example.com/fw.bin"}"#), None);
    }

    #[test]
    fn test_parse_ota_url_reject_non_utf8() {
        assert_eq!(parse_ota_url(&[0xFF, 0xFE]), None);
    }

    #[test]
    fn test_parse_ota_url_reject_callback_url() {
        // Must not match "callback_url" — it's a different key
        assert_eq!(
            parse_ota_url(br#"{"callback_url":"http://example.com/hook"}"#),
            None
        );
    }

    #[test]
    fn test_parse_ota_url_reject_image_url() {
        assert_eq!(
            parse_ota_url(br#"{"image_url":"http://example.com/img.png"}"#),
            None
        );
    }

    #[test]
    fn test_parse_ota_url_whitespace_tolerance() {
        let payload = br#"{"url" : "http://example.com/fw.bin"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from("http://example.com/fw.bin"))
        );
    }

    #[test]
    fn test_parse_ota_url_extra_keys_before() {
        let payload = br#"{"version":"1.0","url":"http://example.com/fw.bin"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from("http://example.com/fw.bin"))
        );
    }

    #[test]
    fn test_parse_ota_url_empty_url_value() {
        // URL key present but value is empty — no scheme → rejected
        assert_eq!(parse_ota_url(br#"{"url":""}"#), None);
    }

    // ── Additional OTA validation tests ─────────────────────────────

    #[test]
    fn test_parse_ota_url_with_crc_query_param() {
        let payload = br#"{"url":"http://192.168.1.100/firmware.bin?crc=DEADBEEF"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from(
                "http://192.168.1.100/firmware.bin?crc=DEADBEEF"
            ))
        );
    }

    #[test]
    fn test_parse_ota_url_with_version_query_param() {
        let payload = br#"{"url":"http://10.0.0.1/fw.bin?version=1.2.3"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from("http://10.0.0.1/fw.bin?version=1.2.3"))
        );
    }

    #[test]
    fn test_parse_ota_url_with_both_crc_and_version() {
        let payload = br#"{"url":"http://example.com/fw.bin?crc=ABCD1234&version=2.0.0"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from(
                "http://example.com/fw.bin?crc=ABCD1234&version=2.0.0"
            ))
        );
    }

    #[test]
    fn test_parse_ota_url_javascript_scheme_rejected() {
        assert_eq!(parse_ota_url(br#"{"url":"javascript:alert(1)"}"#), None);
    }

    #[test]
    fn test_parse_ota_url_deep_nested_json() {
        // URL nested inside other keys should still be found
        let payload = br#"{"config":{"ota":{"url":"http://10.0.0.1/fw.bin"}}}"#;
        // This won't match because the parser expects top-level "url" key
        // Actually, the parser does a simple string search, so it will find "url" anywhere
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from("http://10.0.0.1/fw.bin"))
        );
    }

    #[test]
    fn test_parse_ota_url_with_path_segments() {
        let payload = br#"{"url":"http://192.168.1.100/ota/firmware/v2/rc1/firmware.bin"}"#;
        assert_eq!(
            parse_ota_url(payload),
            Some(String::from(
                "http://192.168.1.100/ota/firmware/v2/rc1/firmware.bin"
            ))
        );
    }

    #[test]
    fn test_parse_ota_url_http_scheme_case_sensitive() {
        // "HTTP" (uppercase) should be rejected — only lowercase "http" is valid
        assert_eq!(
            parse_ota_url(br#"{"url":"HTTP://192.168.1.100/fw.bin"}"#),
            None
        );
    }

    #[test]
    fn test_parse_ota_url_ip_with_auth() {
        // URL with userinfo should still be accepted if http:// scheme
        assert_eq!(
            parse_ota_url(br#"{"url":"http://admin:pass@192.168.1.100/fw.bin"}"#),
            Some(String::from("http://admin:pass@192.168.1.100/fw.bin"))
        );
    }
}
