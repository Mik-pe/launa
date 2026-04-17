//! HTTP parsing pure functions for OTA firmware updates.
//!
//! These functions handle HTTP URL parsing, status validation, header parsing,
//! and CRC query parameter extraction. They are pure functions with no side
//! effects, making them easy to test on desktop.

use alloc::string::String;
use alloc::vec::Vec;

/// Validate that the HTTP response status line indicates success (200).
///
/// Checks that the response starts with `HTTP/1.` followed by a space and
/// status code `200`.
///
/// # Examples
///
/// ```
/// use launa_ota::http::validate_http_status;
/// assert!(validate_http_status(b"HTTP/1.1 200 OK\r\n"));
/// assert!(!validate_http_status(b"HTTP/1.1 404 Not Found\r\n"));
/// ```
pub fn validate_http_status(headers: &[u8]) -> bool {
    // Status line format: "HTTP/1.x 200 ..."
    if headers.len() < 12 {
        return false;
    }
    if !headers.starts_with(b"HTTP/1.") {
        return false;
    }
    // Status code is at bytes 9-11 (e.g., "HTTP/1.1 200")
    headers[9] == b'2' && headers[10] == b'0' && headers[11] == b'0'
}

/// Extract the status line from HTTP headers for error logging.
///
/// Returns the text before the first `\r` or `\n`. If the header data
/// is very long (>40 bytes) and contains no line ending, truncates to
/// 40 bytes with "..." appended.
pub fn extract_status_line(headers: &[u8]) -> String {
    if let Some(pos) = headers.iter().position(|&b| b == b'\r' || b == b'\n') {
        String::from_utf8_lossy(&headers[..pos]).into_owned()
    } else if headers.len() > 40 {
        String::from_utf8_lossy(&headers[..40]).into_owned() + "..."
    } else {
        String::from_utf8_lossy(headers).into_owned()
    }
}

/// Find the end of HTTP headers (`\r\n\r\n`) in a byte buffer.
///
/// Returns the index of the first `\r` of the terminating `\r\n\r\n`,
/// or `None` if the header terminator is not found.
pub fn find_header_end(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(3) {
        if data[i] == b'\r' && data[i + 1] == b'\n' && data[i + 2] == b'\r' && data[i + 3] == b'\n'
        {
            return Some(i);
        }
    }
    None
}

/// Parse `crc` query parameter from URL (e.g. `?crc=DEADBEEF`).
///
/// Searches the query string for a `crc=<hex>` parameter and parses
/// the hex value as a `u32`. Returns `None` if the parameter is not
/// present or the hex value is not valid.
pub fn parse_crc_from_url(url: &str) -> Option<u32> {
    let query_start = url.find('?')?;
    let query = &url[query_start + 1..];
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("crc=") {
            return u32::from_str_radix(value, 16).ok();
        }
    }
    None
}

/// Simple HTTP URL parser. Returns `(host, port, path)`.
///
/// Only supports `http://` scheme. Port defaults to 80 if not specified.
/// Path defaults to `/` if not specified.
///
/// Returns `None` if the URL doesn't start with `http://` or is malformed.
pub fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let url = url.strip_prefix("http://")?;
    let (host_port, path) = match url.find('/') {
        Some(idx) => (&url[..idx], &url[idx..]),
        None => (url, "/"),
    };

    let (host, port) = match host_port.find(':') {
        Some(idx) => {
            let port: u16 = host_port[idx + 1..].parse().ok()?;
            (String::from(&host_port[..idx]), port)
        }
        None => (String::from(host_port), 80),
    };

    Some((host, port, String::from(path)))
}

/// Parse `Content-Length` header value from HTTP response headers.
///
/// Performs a case-insensitive search for the `Content-Length:` header
/// and parses its value as a `u32`. Returns `None` if the header is
/// not found or the value is not a valid number.
pub fn parse_content_length(headers: &[u8]) -> Option<u32> {
    // Search case-insensitively for "Content-Length:"
    let header_name = b"content-length:";
    let headers_lower: Vec<u8> = headers.iter().map(|&b| b.to_ascii_lowercase()).collect();

    if let Some(pos) = find_header_value_start(&headers_lower, header_name) {
        let value_start = pos;
        let value_end = headers_lower[value_start..]
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')
            .map(|i| value_start + i)
            .unwrap_or(headers_lower.len());
        let value_str = core::str::from_utf8(&headers[value_start..value_end]).ok()?;
        let trimmed = value_str.trim();
        trimmed.parse::<u32>().ok()
    } else {
        None
    }
}

/// Find the start of a header value after the header name.
///
/// Performs a case-sensitive search for `name` in `headers` and returns
/// the position of the first non-space byte after the header name.
/// Returns `None` if the header name is not found.
pub fn find_header_value_start(headers: &[u8], name: &[u8]) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let search_from = 0;
    while search_from < headers.len() {
        if let Some(pos) = headers[search_from..]
            .windows(name.len())
            .position(|w| w == name)
        {
            let abs_pos = search_from + pos + name.len();
            // Skip any leading whitespace
            let mut start = abs_pos;
            while start < headers.len() && headers[start] == b' ' {
                start += 1;
            }
            return Some(start);
        }
        break;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // ========== parse_http_url tests ==========

    #[test]
    fn test_parse_http_url_standard() {
        let result = parse_http_url("http://example.com/firmware.bin");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/firmware.bin");
    }

    #[test]
    fn test_parse_http_url_with_port() {
        let result = parse_http_url("http://example.com:8080/firmware.bin");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8080);
        assert_eq!(path, "/firmware.bin");
    }

    #[test]
    fn test_parse_http_url_no_path() {
        let result = parse_http_url("http://example.com");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_parse_http_url_no_path_with_port() {
        let result = parse_http_url("http://example.com:3000");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 3000);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_parse_http_url_with_query_params() {
        let result = parse_http_url("http://192.168.1.100/fw.bin?crc=DEADBEEF");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "192.168.1.100");
        assert_eq!(port, 80);
        assert_eq!(path, "/fw.bin?crc=DEADBEEF");
    }

    #[test]
    fn test_parse_http_url_invalid_prefix() {
        assert!(parse_http_url("ftp://example.com/fw.bin").is_none());
    }

    #[test]
    fn test_parse_http_url_https_rejection() {
        // HTTPS is not supported — only http:// prefix is accepted
        assert!(parse_http_url("https://example.com/fw.bin").is_none());
    }

    #[test]
    fn test_parse_http_url_empty_string() {
        assert!(parse_http_url("").is_none());
    }

    #[test]
    fn test_parse_http_url_just_scheme() {
        // "http://" with nothing after — host_port is empty
        let result = parse_http_url("http://");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_parse_http_url_ipv4_address() {
        let result = parse_http_url("http://10.0.0.1:8080/firmware.bin");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 8080);
        assert_eq!(path, "/firmware.bin");
    }

    #[test]
    fn test_parse_http_url_deep_path() {
        let result = parse_http_url("http://example.com/a/b/c/firmware.bin");
        assert!(result.is_some());
        let (host, port, path) = result.unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/a/b/c/firmware.bin");
    }

    #[test]
    fn test_parse_http_url_invalid_port() {
        // Port is not a number
        assert!(parse_http_url("http://example.com:abc/fw.bin").is_none());
    }

    #[test]
    fn test_parse_http_url_port_zero() {
        let result = parse_http_url("http://example.com:0/fw.bin");
        assert!(result.is_some());
        let (_, port, _) = result.unwrap();
        assert_eq!(port, 0);
    }

    #[test]
    fn test_parse_http_url_port_max() {
        let result = parse_http_url("http://example.com:65535/fw.bin");
        assert!(result.is_some());
        let (_, port, _) = result.unwrap();
        assert_eq!(port, 65535);
    }

    #[test]
    fn test_parse_http_url_port_overflow() {
        // Port 65536 overflows u16
        assert!(parse_http_url("http://example.com:65536/fw.bin").is_none());
    }

    // ========== validate_http_status tests ==========

    #[test]
    fn test_validate_http_status_200_ok() {
        assert!(validate_http_status(
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n"
        ));
    }

    #[test]
    fn test_validate_http_status_200_http10() {
        assert!(validate_http_status(b"HTTP/1.0 200 OK\r\n\r\n"));
    }

    #[test]
    fn test_validate_http_status_404() {
        assert!(!validate_http_status(b"HTTP/1.1 404 Not Found\r\n\r\n"));
    }

    #[test]
    fn test_validate_http_status_500() {
        assert!(!validate_http_status(
            b"HTTP/1.1 500 Internal Server Error\r\n\r\n"
        ));
    }

    #[test]
    fn test_validate_http_status_301() {
        assert!(!validate_http_status(
            b"HTTP/1.1 301 Moved Permanently\r\n\r\n"
        ));
    }

    #[test]
    fn test_validate_http_status_short_input() {
        // Less than 12 bytes
        assert!(!validate_http_status(b"HTTP/1.1"));
    }

    #[test]
    fn test_validate_http_status_exactly_12_bytes() {
        // "HTTP/1.1 200" is exactly 12 bytes
        assert!(validate_http_status(b"HTTP/1.1 200"));
    }

    #[test]
    fn test_validate_http_status_11_bytes() {
        // 11 bytes, just under the threshold
        assert!(!validate_http_status(b"HTTP/1.1 20"));
    }

    #[test]
    fn test_validate_http_status_non_http_prefix() {
        assert!(!validate_http_status(b"FOOBAR/1.1 200 OK\r\n\r\n"));
    }

    #[test]
    fn test_validate_http_status_empty() {
        assert!(!validate_http_status(b""));
    }

    #[test]
    fn test_validate_http_status_201_created() {
        // 201 is not 200
        assert!(!validate_http_status(b"HTTP/1.1 201 Created\r\n\r\n"));
    }

    #[test]
    fn test_validate_http_status_204_no_content() {
        assert!(!validate_http_status(b"HTTP/1.1 204 No Content\r\n\r\n"));
    }

    // ========== find_header_end tests ==========

    #[test]
    fn test_find_header_end_with_headers() {
        let data = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nbody";
        // HTTP/1.1 200 OK\r\n = 18 bytes, Content-Length: 100\r\n = 20 bytes → \r\n\r\n at index 36
        let pos = find_header_end(data).unwrap();
        assert_eq!(&data[pos..pos + 4], b"\r\n\r\n");
        // Body follows after
        assert_eq!(&data[pos + 4..], b"body");
    }

    #[test]
    fn test_find_header_end_without_terminator() {
        let data = b"HTTP/1.1 200 OK\r\nContent-Length: 100";
        assert_eq!(find_header_end(data), None);
    }

    #[test]
    fn test_find_header_end_empty() {
        assert_eq!(find_header_end(b""), None);
    }

    #[test]
    fn test_find_header_end_partial() {
        // Only \r\n but no \r\n\r\n
        let data = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n";
        assert_eq!(find_header_end(data), None);
    }

    #[test]
    fn test_find_header_end_only_terminator() {
        // Just the terminator
        assert_eq!(find_header_end(b"\r\n\r\n"), Some(0));
    }

    #[test]
    fn test_find_header_end_short_data() {
        // Less than 4 bytes, can't possibly contain \r\n\r\n
        assert_eq!(find_header_end(b"\r\n"), None);
        assert_eq!(find_header_end(b"\r\n\r"), None);
        assert_eq!(find_header_end(b"abc"), None);
    }

    #[test]
    fn test_find_header_end_exactly_3_bytes() {
        assert_eq!(find_header_end(b"\r\n\r"), None);
    }

    #[test]
    fn test_find_header_end_multiple_possible() {
        // Should find the FIRST \r\n\r\n
        // "HTTP/1. 200\r\n\r\n" → "HTTP/1. 200" is 11 bytes, then \r\n\r\n at 11
        let data = b"HTTP/1. 200\r\n\r\nExtra\r\n\r\n";
        assert_eq!(find_header_end(data), Some(11));
    }

    #[test]
    fn test_find_header_end_at_end_of_data() {
        // "HTTP/1.1 200 OK" = 15 bytes, then \r\n\r\n
        let data = b"HTTP/1.1 200 OK\r\n\r\n";
        let pos = find_header_end(data).unwrap();
        assert_eq!(&data[pos..pos + 4], b"\r\n\r\n");
    }

    #[test]
    fn test_find_header_end_body_after() {
        let data = b"HTTP/1.1 200\r\n\r\nbody data here";
        let pos = find_header_end(data).unwrap();
        assert_eq!(&data[pos..pos + 4], b"\r\n\r\n");
        // Body starts at pos + 4
        assert_eq!(&data[pos + 4..], b"body data here");
    }

    // ========== parse_crc_from_url tests ==========

    #[test]
    fn test_parse_crc_from_url_with_crc() {
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=DEADBEEF"),
            Some(0xDEADBEEF)
        );
    }

    #[test]
    fn test_parse_crc_from_url_lowercase_hex() {
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=deadbeef"),
            Some(0xDEADBEEF)
        );
    }

    #[test]
    fn test_parse_crc_from_url_without_crc() {
        assert_eq!(parse_crc_from_url("http://example.com/fw.bin"), None);
    }

    #[test]
    fn test_parse_crc_from_url_invalid_hex() {
        // "ZZZZ" is not valid hex
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=ZZZZ"),
            None
        );
    }

    #[test]
    fn test_parse_crc_from_url_with_other_params() {
        // crc is not the first param
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?version=2&crc=CAFEBABE"),
            Some(0xCAFEBABE)
        );
    }

    #[test]
    fn test_parse_crc_from_url_crc_before_other_params() {
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=1234ABCD&version=2"),
            Some(0x1234ABCD)
        );
    }

    #[test]
    fn test_parse_crc_from_url_no_query() {
        // No ? in URL
        assert_eq!(parse_crc_from_url("http://example.com/fw.bin"), None);
    }

    #[test]
    fn test_parse_crc_from_url_empty_query() {
        assert_eq!(parse_crc_from_url("http://example.com/fw.bin?"), None);
    }

    #[test]
    fn test_parse_crc_from_url_empty_crc_value() {
        // crc= with empty value — u32 parse of "" will fail
        assert_eq!(parse_crc_from_url("http://example.com/fw.bin?crc="), None);
    }

    #[test]
    fn test_parse_crc_from_url_zero_crc() {
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=0"),
            Some(0)
        );
    }

    #[test]
    fn test_parse_crc_from_url_truncated_crc() {
        // Only 4 hex digits (valid u32)
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=BEEF"),
            Some(0xBEEF)
        );
    }

    #[test]
    fn test_parse_crc_from_url_overflow() {
        // Value too large for u32
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=DEADBEEF00"),
            None
        );
    }

    // ========== parse_content_length tests ==========

    #[test]
    fn test_parse_content_length_valid() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 12345\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(12345));
    }

    #[test]
    fn test_parse_content_length_lowercase() {
        let headers = b"HTTP/1.1 200 OK\r\ncontent-length: 9999\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(9999));
    }

    #[test]
    fn test_parse_content_length_mixed_case() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-length: 42\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(42));
    }

    #[test]
    fn test_parse_content_length_missing() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n";
        assert_eq!(parse_content_length(headers), None);
    }

    #[test]
    fn test_parse_content_length_invalid() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n";
        assert_eq!(parse_content_length(headers), None);
    }

    #[test]
    fn test_parse_content_length_large_value() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 4294967295\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(4294967295));
    }

    #[test]
    fn test_parse_content_length_zero() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(0));
    }

    #[test]
    fn test_parse_content_length_with_extra_spaces() {
        // Some HTTP servers add extra spaces after the colon
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length:   512\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(512));
    }

    #[test]
    fn test_parse_content_length_negative_rejected() {
        // u32 can't be negative
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n";
        assert_eq!(parse_content_length(headers), None);
    }

    #[test]
    fn test_parse_content_length_overflow() {
        // u32::MAX + 1 overflows
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 4294967296\r\n\r\n";
        assert_eq!(parse_content_length(headers), None);
    }

    #[test]
    fn test_parse_content_length_empty_headers() {
        assert_eq!(parse_content_length(b""), None);
    }

    #[test]
    fn test_parse_content_length_no_terminator() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 100";
        assert_eq!(parse_content_length(headers), Some(100));
    }

    #[test]
    fn test_parse_content_length_multiple_headers() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 2048\r\nServer: nginx\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(2048));
    }

    // ========== find_header_value_start tests ==========

    #[test]
    fn test_find_header_value_start_basic() {
        let headers = b"Content-Length: 1234\r\n";
        let result = find_header_value_start(headers, b"Content-Length: ");
        assert_eq!(result, Some(16));
        assert_eq!(&headers[16..20], b"1234");
    }

    #[test]
    fn test_find_header_value_start_not_found() {
        let headers = b"Content-Type: text/html\r\n";
        assert_eq!(find_header_value_start(headers, b"Content-Length: "), None);
    }

    #[test]
    fn test_find_header_value_start_empty_headers() {
        assert_eq!(find_header_value_start(b"", b"Content-Length: "), None);
    }

    #[test]
    fn test_find_header_value_start_empty_name() {
        // Empty name is edge case - should return start of headers (position 0)
        let headers = b"value";
        // Empty name search with windows(0) would panic, so we expect None
        assert_eq!(find_header_value_start(headers, b""), None);
    }

    #[test]
    fn test_find_header_value_start_name_longer_than_headers() {
        assert_eq!(
            find_header_value_start(b"short", b"Very-Long-Header-Name: "),
            None
        );
    }

    #[test]
    fn test_find_header_value_start_skips_whitespace() {
        let headers = b"Content-Length:   5678\r\n";
        // "Content-Length: " is 16 bytes, then 2 extra spaces
        let result = find_header_value_start(headers, b"Content-Length: ");
        assert_eq!(result, Some(18));
        assert_eq!(&headers[18..22], b"5678");
    }

    #[test]
    fn test_find_header_value_start_no_trailing_data() {
        // Header at end of buffer, no CRLF after
        let headers = b"X-Value:42";
        assert_eq!(find_header_value_start(headers, b"X-Value:"), Some(8));
        assert_eq!(&headers[8..], b"42");
    }

    // ========== extract_status_line tests ==========

    #[test]
    fn test_extract_status_line_normal() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(extract_status_line(headers), "HTTP/1.1 200 OK");
    }

    #[test]
    fn test_extract_status_line_with_lf() {
        let headers = b"HTTP/1.1 404 Not Found\nContent-Length: 0\r\n\r\n";
        assert_eq!(extract_status_line(headers), "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn test_extract_status_line_no_line_ending_short() {
        let headers = b"HTTP/1.1 200 OK";
        assert_eq!(extract_status_line(headers), "HTTP/1.1 200 OK");
    }

    #[test]
    fn test_extract_status_line_no_line_ending_long() {
        // Over 40 bytes with no line ending → truncated to 40 + "..."
        let mut headers = vec![b'X'; 50];
        headers[0..8].copy_from_slice(b"HTTP/1.1");
        let result = extract_status_line(&headers);
        assert_eq!(result.len(), 43); // 40 + 3 ("...")
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_extract_status_line_empty() {
        assert_eq!(extract_status_line(b""), "");
    }

    #[test]
    fn test_extract_status_line_exactly_40_bytes_no_ending() {
        let headers = vec![b'A'; 40];
        let result = extract_status_line(&headers);
        // 40 bytes is NOT > 40, so no truncation
        assert_eq!(result.len(), 40);
        assert!(!result.contains("..."));
    }

    #[test]
    fn test_extract_status_line_41_bytes_no_ending() {
        // 41 bytes with no line ending → truncated to 40 + "..."
        let headers = vec![b'A'; 41];
        let result = extract_status_line(&headers);
        assert_eq!(result.len(), 43);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_extract_status_line_500_error() {
        let headers = b"HTTP/1.1 500 Internal Server Error\r\n\r\n";
        assert_eq!(
            extract_status_line(headers),
            "HTTP/1.1 500 Internal Server Error"
        );
    }
}
