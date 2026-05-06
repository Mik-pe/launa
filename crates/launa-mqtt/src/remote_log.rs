//! Remote log entry JSON serialization.
//!
//! Provides `log_entry_to_json()` for formatting captured log entries as JSON
//! payloads suitable for MQTT publishing. Extracted from `app/src/remote_log.rs`
//! for desktop testability.

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::format;
#[cfg(not(feature = "std"))]
use alloc::string::String;

/// A captured log entry with level, message, and timestamp.
///
/// Re-exported from `launa_core::LogEntry` in production; this struct mirrors
/// the shape so the function can work independently.
pub struct RemoteLogEntry {
    pub level: &'static str,
    pub message: String,
    pub timestamp_ms: u64,
}

/// Format a log entry as a JSON string suitable for MQTT publishing.
///
/// Manual JSON construction (no serde) for `no_std` compatibility.
/// Uses `escape_json_string()` from the `escape` module for proper JSON escaping.
///
/// # Example output
///
/// ```json
/// {"level":"warn","message":"Temperature high","ts":12345}
/// ```
pub fn log_entry_to_json(entry: &RemoteLogEntry) -> String {
    let escaped = crate::escape::escape_json_string(&entry.message);
    format!(
        "{{\"level\":\"{}\",\"message\":\"{}\",\"ts\":{}}}",
        entry.level, escaped, entry.timestamp_ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "std")]
    fn parse_json(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("output should be valid JSON")
    }

    #[test]
    fn test_log_entry_to_json_levels_and_timestamps() {
        // Verify level field and timestamp formatting for various levels
        for (level, ts) in [("warn", 12345u64), ("error", 99999u64), ("info", 0u64)] {
            let entry = RemoteLogEntry {
                level,
                message: String::from("msg"),
                timestamp_ms: ts,
            };
            let json = log_entry_to_json(&entry);
            assert!(json.contains(&format!("\"level\":\"{}\"", level)));
            assert!(json.contains(&format!("\"ts\":{}", ts)));
            #[cfg(feature = "std")]
            {
                let parsed = parse_json(&json);
                assert_eq!(parsed["level"], level);
                assert_eq!(parsed["ts"], ts);
            }
        }
    }

    #[test]
    fn test_log_entry_to_json_empty_message() {
        let entry = RemoteLogEntry {
            level: "info",
            message: String::from(""),
            timestamp_ms: 0,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\"message\":\"\""));
    }

    #[test]
    fn test_log_entry_to_json_escapes_each_special_char() {
        // Verify each special character is properly escaped
        let cases: &[(&str, &str)] = &[
            ("Heater \"dry\" fire", "\\\""),
            ("Path: \\dev\\null", "\\\\"),
            ("Line1\nLine2", "\\n"),
            ("Col1\tCol2", "\\t"),
            ("Line1\rLine2", "\\r"),
            ("Bad\x07char", "\\u0007"),
            ("before\x00after", "\\u0000"),
        ];
        for (msg, expected_escape) in cases {
            let entry = RemoteLogEntry {
                level: "error",
                message: String::from(*msg),
                timestamp_ms: 100,
            };
            let json = log_entry_to_json(&entry);
            assert!(
                json.contains(expected_escape),
                "expected {} escape in JSON for message {:?}",
                expected_escape,
                msg
            );
            #[cfg(feature = "std")]
            {
                let parsed = parse_json(&json);
                assert_eq!(parsed["message"], *msg);
            }
        }
    }

    #[test]
    fn test_log_entry_to_json_all_control_range() {
        // Verify all control characters U+0001..=U+001F round-trip correctly
        for code in 1u32..=0x1F {
            let ch = char::from_u32(code).unwrap();
            let msg = format!("a{}b", ch);
            let entry = RemoteLogEntry {
                level: "error",
                message: msg.clone(),
                timestamp_ms: 800,
            };
            let json = log_entry_to_json(&entry);
            #[cfg(feature = "std")]
            {
                let parsed = parse_json(&json);
                assert_eq!(
                    parsed["message"], msg,
                    "failed for control char 0x{:02x}",
                    code
                );
            }
        }
    }

    #[test]
    fn test_log_entry_to_json_combined_special_chars() {
        // All special chars in one message — round-trip verification
        let msg = format!("a\\b\"c\nd\re\tf\x01g");
        let entry = RemoteLogEntry {
            level: "warn",
            message: msg.clone(),
            timestamp_ms: 900,
        };
        let json = log_entry_to_json(&entry);
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], msg);
        }
    }

    #[test]
    fn test_log_entry_to_json_unicode_preserved() {
        let entry = RemoteLogEntry {
            level: "info",
            message: String::from("Temperature: 38°C — spa är varm 日本語"),
            timestamp_ms: 1000,
        };
        let json = log_entry_to_json(&entry);
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], "Temperature: 38°C — spa är varm 日本語");
        }
    }

    #[test]
    fn test_log_entry_to_json_large_timestamp() {
        let entry = RemoteLogEntry {
            level: "info",
            message: String::from("uptime"),
            timestamp_ms: u64::MAX,
        };
        let json = log_entry_to_json(&entry);
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["ts"], u64::MAX);
        }
    }
}
