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

    #[test]
    fn test_parse_http_url_standard_cases() {
        // Standard URL with default port
        let (host, port, path) = parse_http_url("http://example.com/firmware.bin").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/firmware.bin");

        // URL with explicit port
        let (host, port, path) = parse_http_url("http://example.com:8080/firmware.bin").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8080);
        assert_eq!(path, "/firmware.bin");

        // IPv4 address with port
        let (host, port, path) = parse_http_url("http://10.0.0.1:8080/firmware.bin").unwrap();
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 8080);
        assert_eq!(path, "/firmware.bin");

        // Deep path
        let (host, port, path) = parse_http_url("http://example.com/a/b/c/firmware.bin").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/a/b/c/firmware.bin");
    }

    #[test]
    fn test_parse_http_url_no_path() {
        // No path — defaults to "/"
        let (host, port, path) = parse_http_url("http://example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/");

        // No path with port
        let (host, port, path) = parse_http_url("http://example.com:3000").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 3000);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_parse_http_url_with_query_params() {
        let (host, port, path) =
            parse_http_url("http://192.168.1.100/fw.bin?crc=DEADBEEF").unwrap();
        assert_eq!(host, "192.168.1.100");
        assert_eq!(port, 80);
        assert_eq!(path, "/fw.bin?crc=DEADBEEF");
    }

    #[test]
    fn test_parse_http_url_edge_cases() {
        // Port zero (valid u16)
        let (_, port, _) = parse_http_url("http://example.com:0/fw.bin").unwrap();
        assert_eq!(port, 0);

        // Port max (65535)
        let (_, port, _) = parse_http_url("http://example.com:65535/fw.bin").unwrap();
        assert_eq!(port, 65535);

        // Just scheme — empty host
        let (host, port, path) = parse_http_url("http://").unwrap();
        assert_eq!(host, "");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_parse_http_url_rejections() {
        assert!(parse_http_url("ftp://example.com/fw.bin").is_none());
        assert!(parse_http_url("https://example.com/fw.bin").is_none());
        assert!(parse_http_url("").is_none());
        assert!(parse_http_url("http://example.com:abc/fw.bin").is_none());
        assert!(parse_http_url("http://example.com:65536/fw.bin").is_none());
    }

    #[test]
    fn test_validate_http_status_success_cases() {
        assert!(validate_http_status(
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n"
        ));
        assert!(validate_http_status(b"HTTP/1.0 200 OK\r\n\r\n"));
        assert!(validate_http_status(b"HTTP/1.1 200")); // exactly 12 bytes
    }

    #[test]
    fn test_validate_http_status_failure_cases() {
        // Non-200 status codes
        assert!(!validate_http_status(b"HTTP/1.1 404 Not Found\r\n\r\n"));
        assert!(!validate_http_status(
            b"HTTP/1.1 500 Internal Server Error\r\n\r\n"
        ));
        assert!(!validate_http_status(
            b"HTTP/1.1 301 Moved Permanently\r\n\r\n"
        ));
        assert!(!validate_http_status(b"HTTP/1.1 201 Created\r\n\r\n"));
        assert!(!validate_http_status(b"HTTP/1.1 204 No Content\r\n\r\n"));

        // Edge cases
        assert!(!validate_http_status(b"HTTP/1.1")); // too short (8 bytes)
        assert!(!validate_http_status(b"HTTP/1.1 20")); // 11 bytes
        assert!(!validate_http_status(b"FOOBAR/1.1 200 OK\r\n\r\n")); // wrong prefix
        assert!(!validate_http_status(b""));
    }

    #[test]
    fn test_find_header_end_found() {
        // Headers with body after
        let data = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nbody";
        let pos = find_header_end(data).unwrap();
        assert_eq!(&data[pos..pos + 4], b"\r\n\r\n");
        assert_eq!(&data[pos + 4..], b"body");

        // At end of data
        let data = b"HTTP/1.1 200 OK\r\n\r\n";
        let pos = find_header_end(data).unwrap();
        assert_eq!(&data[pos..pos + 4], b"\r\n\r\n");

        // Body after
        let data = b"HTTP/1.1 200\r\n\r\nbody data here";
        let pos = find_header_end(data).unwrap();
        assert_eq!(&data[pos + 4..], b"body data here");

        // Just the terminator
        assert_eq!(find_header_end(b"\r\n\r\n"), Some(0));

        // Multiple terminators — find first
        let data = b"HTTP/1. 200\r\n\r\nExtra\r\n\r\n";
        assert_eq!(find_header_end(data), Some(11));
    }

    #[test]
    fn test_find_header_end_not_found() {
        assert_eq!(
            find_header_end(b"HTTP/1.1 200 OK\r\nContent-Length: 100"),
            None
        );
        assert_eq!(find_header_end(b""), None);
        assert_eq!(
            find_header_end(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n"),
            None
        );
        assert_eq!(find_header_end(b"\r\n"), None);
        assert_eq!(find_header_end(b"\r\n\r"), None);
        assert_eq!(find_header_end(b"\r\n\r"), None);
        assert_eq!(find_header_end(b"abc"), None);
    }

    #[test]
    fn test_parse_crc_from_url_valid() {
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=DEADBEEF"),
            Some(0xDEADBEEF)
        );
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=deadbeef"),
            Some(0xDEADBEEF)
        );
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?version=2&crc=CAFEBABE"),
            Some(0xCAFEBABE)
        );
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=1234ABCD&version=2"),
            Some(0x1234ABCD)
        );
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=0"),
            Some(0)
        );
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=BEEF"),
            Some(0xBEEF)
        );
    }

    #[test]
    fn test_parse_crc_from_url_invalid() {
        assert_eq!(parse_crc_from_url("http://example.com/fw.bin"), None);
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=ZZZZ"),
            None
        );
        assert_eq!(parse_crc_from_url("http://example.com/fw.bin?"), None);
        assert_eq!(parse_crc_from_url("http://example.com/fw.bin?crc="), None);
        assert_eq!(
            parse_crc_from_url("http://example.com/fw.bin?crc=DEADBEEF00"),
            None
        );
    }

    #[test]
    fn test_parse_content_length_valid() {
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length: 12345\r\n\r\n"),
            Some(12345)
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\ncontent-length: 9999\r\n\r\n"),
            Some(9999)
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-length: 42\r\n\r\n"),
            Some(42)
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length: 4294967295\r\n\r\n"),
            Some(4294967295)
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"),
            Some(0)
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length:   512\r\n\r\n"),
            Some(512)
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length: 100"),
            Some(100)
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 2048\r\nServer: nginx\r\n\r\n"),
            Some(2048)
        );
    }

    #[test]
    fn test_parse_content_length_invalid() {
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n"),
            None
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n"),
            None
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n"),
            None
        );
        assert_eq!(
            parse_content_length(b"HTTP/1.1 200 OK\r\nContent-Length: 4294967296\r\n\r\n"),
            None
        );
        assert_eq!(parse_content_length(b""), None);
    }

    #[test]
    fn test_find_header_value_start_cases() {
        // Basic
        let headers = b"Content-Length: 1234\r\n";
        assert_eq!(
            find_header_value_start(headers, b"Content-Length: "),
            Some(16)
        );
        assert_eq!(&headers[16..20], b"1234");

        // Not found
        assert_eq!(
            find_header_value_start(b"Content-Type: text/html\r\n", b"Content-Length: "),
            None
        );

        // Empty headers
        assert_eq!(find_header_value_start(b"", b"Content-Length: "), None);

        // Empty name
        assert_eq!(find_header_value_start(b"value", b""), None);

        // Name longer than headers
        assert_eq!(
            find_header_value_start(b"short", b"Very-Long-Header-Name: "),
            None
        );

        // Skips whitespace
        let headers = b"Content-Length:   5678\r\n";
        assert_eq!(
            find_header_value_start(headers, b"Content-Length: "),
            Some(18)
        );
        assert_eq!(&headers[18..22], b"5678");

        // No trailing CRLF
        assert_eq!(find_header_value_start(b"X-Value:42", b"X-Value:"), Some(8));
        assert_eq!(&headers[18..22], b"5678");
    }

    #[test]
    fn test_extract_status_line_cases() {
        // Normal with CRLF
        assert_eq!(
            extract_status_line(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"),
            "HTTP/1.1 200 OK"
        );

        // With LF only
        assert_eq!(
            extract_status_line(b"HTTP/1.1 404 Not Found\nContent-Length: 0\r\n\r\n"),
            "HTTP/1.1 404 Not Found"
        );

        // No line ending, short
        assert_eq!(extract_status_line(b"HTTP/1.1 200 OK"), "HTTP/1.1 200 OK");

        // No line ending, long (>40 bytes) — truncated
        let mut headers = vec![b'X'; 50];
        headers[0..8].copy_from_slice(b"HTTP/1.1");
        let result = extract_status_line(&headers);
        assert_eq!(result.len(), 43); // 40 + "..."
        assert!(result.ends_with("..."));

        // Empty
        assert_eq!(extract_status_line(b""), "");

        // Exactly 40 bytes — no truncation
        let headers = vec![b'A'; 40];
        let result = extract_status_line(&headers);
        assert_eq!(result.len(), 40);
        assert!(!result.contains("..."));

        // 41 bytes — truncated
        let headers = vec![b'A'; 41];
        let result = extract_status_line(&headers);
        assert_eq!(result.len(), 43);
        assert!(result.ends_with("..."));

        // 500 error
        assert_eq!(
            extract_status_line(b"HTTP/1.1 500 Internal Server Error\r\n\r\n"),
            "HTTP/1.1 500 Internal Server Error"
        );
    }
}
