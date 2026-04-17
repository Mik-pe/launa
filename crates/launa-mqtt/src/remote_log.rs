//! Remote log entry JSON serialization.
//!
//! Provides `log_entry_to_json()` for formatting captured log entries as JSON
//! payloads suitable for MQTT publishing. Extracted from `app/src/remote_log.rs`
//! for desktop testability.

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
use std::format;
#[cfg(feature = "std")]
use std::string::String;

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
    use std::format;

    // Helper to parse JSON and verify round-trip
    #[cfg(feature = "std")]
    fn parse_json(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("output should be valid JSON")
    }

    #[test]
    fn test_log_entry_to_json_basic() {
        let entry = RemoteLogEntry {
            level: "warn",
            message: String::from("Temperature high"),
            timestamp_ms: 12345,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\"level\":\"warn\""));
        assert!(json.contains("\"message\":\"Temperature high\""));
        assert!(json.contains("\"ts\":12345"));
    }

    #[test]
    fn test_log_entry_to_json_error_level() {
        let entry = RemoteLogEntry {
            level: "error",
            message: String::from("Heap low"),
            timestamp_ms: 99999,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\"level\":\"error\""));
        assert!(json.contains("\"ts\":99999"));
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
    fn test_log_entry_to_json_escapes_quotes() {
        let entry = RemoteLogEntry {
            level: "error",
            message: String::from("Heater \"dry\" fire"),
            timestamp_ms: 100,
        };
        let json = log_entry_to_json(&entry);
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["level"], "error");
            assert_eq!(parsed["message"], "Heater \"dry\" fire");
        }
        // Basic check that the escaped quotes are present
        assert!(json.contains("\\\""));
    }

    #[test]
    fn test_log_entry_to_json_escapes_backslash() {
        let entry = RemoteLogEntry {
            level: "warn",
            message: String::from("Path: \\dev\\null"),
            timestamp_ms: 200,
        };
        let json = log_entry_to_json(&entry);
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], "Path: \\dev\\null");
        }
    }

    #[test]
    fn test_log_entry_to_json_escapes_newline() {
        let entry = RemoteLogEntry {
            level: "error",
            message: String::from("Line1\nLine2"),
            timestamp_ms: 300,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\\n"));
        assert!(!json.contains("Line1\nLine2"));
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], "Line1\nLine2");
        }
    }

    #[test]
    fn test_log_entry_to_json_escapes_tab() {
        let entry = RemoteLogEntry {
            level: "warn",
            message: String::from("Col1\tCol2"),
            timestamp_ms: 400,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\\t"));
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], "Col1\tCol2");
        }
    }

    #[test]
    fn test_log_entry_to_json_escapes_carriage_return() {
        let entry = RemoteLogEntry {
            level: "warn",
            message: String::from("Line1\rLine2"),
            timestamp_ms: 500,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\\r"));
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], "Line1\rLine2");
        }
    }

    #[test]
    fn test_log_entry_to_json_escapes_control_chars() {
        let entry = RemoteLogEntry {
            level: "error",
            message: String::from("Bad\x07char"),
            timestamp_ms: 600,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\\u0007"));
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], "Bad\u{0007}char");
        }
    }

    #[test]
    fn test_log_entry_to_json_escapes_null() {
        let entry = RemoteLogEntry {
            level: "error",
            message: String::from("before\x00after"),
            timestamp_ms: 700,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\\u0000"));
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], "before\u{0000}after");
        }
    }

    #[test]
    fn test_log_entry_to_json_escapes_all_control_range() {
        // Test all control characters from 0x01 to 0x1F
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
    fn test_log_entry_to_json_all_special_combined() {
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
        // Unicode characters should pass through unmodified
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
    fn test_log_entry_to_json_zero_timestamp() {
        let entry = RemoteLogEntry {
            level: "debug",
            message: String::from("boot"),
            timestamp_ms: 0,
        };
        let json = log_entry_to_json(&entry);
        assert!(json.contains("\"ts\":0"));
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["ts"], 0);
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

    #[test]
    fn test_log_entry_to_json_multiple_escapes() {
        // Multiple special characters in one message
        let msg = String::from("Line1\nLine2\t\"quoted\"\\path\r\nEnd\x03");
        let entry = RemoteLogEntry {
            level: "error",
            message: msg.clone(),
            timestamp_ms: 12345,
        };
        let json = log_entry_to_json(&entry);
        #[cfg(feature = "std")]
        {
            let parsed = parse_json(&json);
            assert_eq!(parsed["message"], msg);
            assert_eq!(parsed["level"], "error");
            assert_eq!(parsed["ts"], 12345);
        }
    }
}
