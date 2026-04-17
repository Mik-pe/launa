//! Shared JSON string escaping utility.
//!
//! Provides `escape_json_string()` for safe embedding of arbitrary strings
//! in manually-built JSON string literals (no serde dependency).

extern crate alloc;

use alloc::format;
use alloc::string::String;

/// Escape a string value for safe embedding in a JSON string literal.
///
/// Handles the escapes required by RFC 8259:
///   `\` → `\\`
///   `"` → `\"`
///   `\n` → `\\n`
///   `\r` → `\\r`
///   `\t` → `\\t`
///   Other control chars (U+0000..=U+001F) → `\uXXXX`
pub fn escape_json_string(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) <= 0x1F => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_empty() {
        assert_eq!(escape_json_string(""), "");
    }

    #[test]
    fn test_escape_plain() {
        assert_eq!(escape_json_string("hello world"), "hello world");
    }

    #[test]
    fn test_escape_backslash() {
        assert_eq!(escape_json_string(r"\"), "\\\\");
    }

    #[test]
    fn test_escape_quote() {
        assert_eq!(escape_json_string("\""), "\\\"");
    }

    #[test]
    fn test_escape_newline() {
        assert_eq!(escape_json_string("\n"), "\\n");
    }

    #[test]
    fn test_escape_carriage_return() {
        assert_eq!(escape_json_string("\r"), "\\r");
    }

    #[test]
    fn test_escape_tab() {
        assert_eq!(escape_json_string("\t"), "\\t");
    }

    #[test]
    fn test_escape_control_char() {
        assert_eq!(escape_json_string("\x07"), "\\u0007");
    }

    #[test]
    fn test_escape_null() {
        assert_eq!(escape_json_string("\x00"), "\\u0000");
    }

    #[test]
    fn test_escape_all_combined() {
        let input = alloc::format!("a\\b\"c\nd\re\tf\x01g");
        let escaped = escape_json_string(&input);
        // Should produce a string that round-trips through JSON parsing
        let json = alloc::format!("\"{}\"", escaped);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_str().unwrap(), &input);
    }
}
