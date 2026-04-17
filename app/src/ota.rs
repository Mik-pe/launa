//! OTA firmware update support.
//!
//! Uses `launa-esp-ota` crate for real OTA operations backed by `esp-storage::FlashStorage`.
//! The `EspOtaFlash` struct implements the `OtaUpdate` trait from `launa-ota`.
//!
//! OTA HTTP download is performed over embassy-net TCP. The firmware is downloaded
//! in chunks, skipping HTTP headers, and written directly to the target OTA partition.
//!
//! # Host Resolution
//!
//! Both IPv4 dotted-quad addresses and hostnames are supported. Hostnames are
//! resolved via DNS using the embassy-net DNS client. DHCP-provided DNS servers
//! are used automatically.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_time::Duration;
use embedded_io_async::Write as _;
use launa_esp_ota::{EspOtaFlash, Partition};
use launa_ota::OtaUpdate;
use log::{error, info};

use crate::mk_static;
use crate::net_util;

pub type EspOta = EspOtaFlash<esp_storage::FlashStorage<'static>>;

/// Create a new OTA updater from an existing FlashStorage.
/// Detects the actual running partition from otadata instead of hardcoding.
pub fn create_ota(flash: esp_storage::FlashStorage<'static>) -> EspOta {
    let mut temp = EspOtaFlash::new(flash, Partition::Ota0);
    let running = temp.detect_running_partition().unwrap_or(Partition::Ota0);
    let storage = temp.into_flash();
    EspOtaFlash::new(storage, running)
}

/// Maximum size for HTTP response headers. Prevents OOM from malicious servers.
const MAX_HEADER_SIZE: usize = 4096;

/// TCP socket buffers for OTA, allocated once and reused across attempts.
///
/// Without this, every `perform_ota_update` call would allocate 5 KiB via
/// `mk_static!` that is never reclaimed on failure (the device doesn't reboot),
/// permanently shrinking the 32 KiB heap.
pub struct OtaBuffers {
    rx_buf: &'static mut [u8; 4096],
    tx_buf: &'static mut [u8; 1024],
}

impl OtaBuffers {
    /// Allocate the OTA TCP socket buffers. Call once at startup.
    pub fn new() -> Self {
        let rx_buf = mk_static!([u8; 4096], [0u8; 4096]);
        let tx_buf = mk_static!([u8; 1024], [0u8; 1024]);
        Self { rx_buf, tx_buf }
    }
}

/// Perform an OTA update by downloading firmware from the given HTTP URL.
///
/// Creates a TCP connection, sends an HTTP GET request, validates the
/// HTTP status, skips headers, and writes the body to the OTA partition.
/// On success, finalizes and triggers a software reset.
///
/// The `buffers` are reused across calls to avoid leaking static memory
/// when OTA fails and the device doesn't reboot.
///
/// The `wdt_feed` closure is called after each chunk write to feed the
/// hardware watchdog, preventing WDT reset during long firmware downloads.
pub async fn perform_ota_update(
    stack: &'static Stack<'static>,
    ota: &mut EspOta,
    firmware_url: &str,
    buffers: &mut OtaBuffers,
    mut wdt_feed: impl FnMut(),
) -> Result<(), ()> {
    let (host, port, path) = match parse_http_url(firmware_url) {
        Some(v) => v,
        None => {
            error!("OTA: invalid URL: {}", firmware_url);
            return Err(());
        }
    };

    // Extract expected CRC from URL query parameter (e.g. ?crc=DEADBEEF)
    let expected_crc = parse_crc_from_url(firmware_url);
    if let Some(crc) = expected_crc {
        info!("OTA: expected firmware CRC: {:#010X}", crc);
    }

    info!("OTA: downloading from {}:{}{}", host, port, path);

    // Reuse pre-allocated TCP socket buffers.
    // SAFETY: We are the only task accessing these buffers. The previous
    // TcpSocket (if any) was dropped at the end of the last call.
    let rx: &'static mut [u8] =
        unsafe { &mut *(buffers.rx_buf as *mut [u8; 4096] as *mut [u8]) };
    let tx: &'static mut [u8] =
        unsafe { &mut *(buffers.tx_buf as *mut [u8; 1024] as *mut [u8]) };
    let mut socket = TcpSocket::new(*stack, rx, tx);
    socket.set_timeout(Some(Duration::from_secs(30)));

    // Resolve host: try IPv4 parse first, then DNS
    let addr = match net_util::resolve_host(stack, &host).await {
        Some(a) => a,
        None => {
            error!("OTA: failed to resolve host '{}'", host);
            return Err(());
        }
    };

    // Connect to HTTP server
    if let Err(e) = socket
        .connect(IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::from_octets(addr)),
            port,
        })
        .await
    {
        error!("OTA: TCP connect failed: {:?}", e);
        return Err(());
    }
    info!("OTA: connected to {}:{}", host, port);

    // Send HTTP GET request
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    );
    if let Err(e) = socket.write_all(request.as_bytes()).await {
        error!("OTA: failed to send HTTP request: {:?}", e);
        return Err(());
    }

    // Read response headers and validate HTTP status before erasing partition
    let mut buf = [0u8; 1024];
    // Fixed-size stack buffer for HTTP headers (up to MAX_HEADER_SIZE = 4096 bytes).
    // Replaces Vec::new() to avoid a 4 KiB heap allocation on the 32 KiB ESP32 heap.
    let mut header_buf = [0u8; MAX_HEADER_SIZE];
    let mut header_len: usize = 0;
    let mut total_written: u32 = 0;

    // Phase 1: Read headers and validate HTTP status
    loop {
        let n = match socket.read(&mut buf).await {
            Ok(0) => {
                error!("OTA: connection closed before headers complete");
                return Err(());
            }
            Ok(n) => n,
            Err(e) => {
                error!("OTA: read error during headers: {:?}", e);
                return Err(());
            }
        };

        // Cap header size to prevent OOM on the 32 KiB heap
        if header_len + n > MAX_HEADER_SIZE {
            error!(
                "OTA: headers exceed {} bytes, aborting",
                MAX_HEADER_SIZE
            );
            return Err(());
        }
        header_buf[header_len..header_len + n].copy_from_slice(&buf[..n]);
        header_len += n;

        if let Some(pos) = find_header_end(&header_buf[..header_len]) {
            // Validate HTTP status line before proceeding
            if !validate_http_status(&header_buf[..header_len]) {
                let status_line = extract_status_line(&header_buf[..header_len]);
                error!("OTA: HTTP status not 200: {}", status_line);
                return Err(());
            }

            // HTTP response validated — now safe to erase target partition
            info!("OTA: HTTP 200 OK, erasing target partition");
            if let Err(e) = ota.begin() {
                error!("OTA: begin failed: {:?}", e);
                return Err(());
            }

            // Validate Content-Length against partition size
            if let Some(content_length) = parse_content_length(&header_buf[..header_len]) {
                let partition_size = 0x140000u32; // OTA partition size (matches partitions.csv)
                if content_length > partition_size {
                    error!(
                        "OTA: Content-Length {} exceeds partition size {}",
                        content_length, partition_size
                    );
                    ota_rollback(ota);
                    return Err(());
                }
                info!(
                    "OTA: Content-Length {} bytes (partition size {})",
                    content_length, partition_size
                );
            } else {
                info!("OTA: no Content-Length header, skipping size validation");
            }

            // Write any body data that arrived with the headers
            let body_start = pos + 4;
            if body_start < header_len {
                if let Err(e) = ota.write(&header_buf[body_start..header_len]) {
                    error!("OTA: write failed: {:?}", e);
                    ota_rollback(ota);
                    return Err(());
                }
                total_written += (header_len - body_start) as u32;
                wdt_feed();
            }
            break;
        }
    }

    // Phase 2: Read remaining body and write to OTA partition
    loop {
        let n = match socket.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                error!("OTA: read error: {:?}", e);
                ota_rollback(ota);
                return Err(());
            }
        };

        if let Err(e) = ota.write(&buf[..n]) {
            error!("OTA: write failed: {:?}", e);
            ota_rollback(ota);
            return Err(());
        }
        total_written += n as u32;
        wdt_feed();
    }

    // Verify firmware integrity before finalizing (if expected CRC was provided)
    if let Some(crc) = expected_crc {
        if let Err(e) = ota.verify_hash(crc) {
            error!("OTA: firmware integrity check failed: {:?}", e);
            ota_rollback(ota);
            return Err(());
        }
        info!("OTA: firmware CRC verified successfully");
    }

    // Finalize the OTA update
    if let Err(e) = ota.finalize() {
        error!("OTA: finalize failed: {:?}", e);
        ota_rollback(ota);
        return Err(());
    }

    info!(
        "OTA: {} bytes written successfully, rebooting",
        total_written
    );

    esp_hal::system::software_reset()
}

/// Roll back OTA and immediately reboot. Used on download/write failures
/// to ensure the device doesn't continue running with a wiped partition.
fn ota_rollback(ota: &mut EspOta) {
    error!("OTA: rolling back and rebooting");
    let _ = ota.rollback_and_reboot();
    esp_hal::system::software_reset()
}

/// Validate that the HTTP response status line indicates success (200).
fn validate_http_status(headers: &[u8]) -> bool {
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
fn extract_status_line(headers: &[u8]) -> alloc::string::String {
    if let Some(pos) = headers.iter().position(|&b| b == b'\r' || b == b'\n') {
        alloc::string::String::from_utf8_lossy(&headers[..pos]).into_owned()
    } else if headers.len() > 40 {
        alloc::string::String::from_utf8_lossy(&headers[..40]).into_owned() + "..."
    } else {
        alloc::string::String::from_utf8_lossy(headers).into_owned()
    }
}

/// Find the end of HTTP headers (`\r\n\r\n`) in a byte buffer.
/// Returns the index of the first `\r` of the terminating `\r\n\r\n`.
fn find_header_end(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(3) {
        if data[i] == b'\r' && data[i + 1] == b'\n' && data[i + 2] == b'\r' && data[i + 3] == b'\n'
        {
            return Some(i);
        }
    }
    None
}

/// Parse `crc` query parameter from URL (e.g. `?crc=DEADBEEF`).
/// Returns `None` if not present or not a valid hex u32.
fn parse_crc_from_url(url: &str) -> Option<u32> {
    let query_start = url.find('?')?;
    let query = &url[query_start + 1..];
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("crc=") {
            return u32::from_str_radix(value, 16).ok();
        }
    }
    None
}

/// Simple HTTP URL parser. Returns (host, port, path).
fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
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
/// Returns `None` if the header is not found or the value is not a valid number.
fn parse_content_length(headers: &[u8]) -> Option<u32> {
    // Search case-insensitively for "Content-Length:"
    let header_name = b"content-length:";
    let headers_lower: alloc::vec::Vec<u8> = headers.iter().map(|&b| b.to_ascii_lowercase()).collect();

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
fn find_header_value_start(headers: &[u8], name: &[u8]) -> Option<usize> {
    let search_from = 0;
    while search_from < headers.len() {
        if let Some(pos) = headers[search_from..].windows(name.len()).position(|w| w == name) {
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
