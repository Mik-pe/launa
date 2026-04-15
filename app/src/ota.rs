//! OTA firmware update support.
//!
//! Uses `launa-esp-ota` crate for real OTA operations backed by `esp-storage::FlashStorage`.
//! The `EspOtaFlash` struct implements the `OtaUpdate` trait from `launa-ota`.
//!
//! OTA HTTP download is performed over embassy-net TCP. The firmware is downloaded
//! in chunks, skipping HTTP headers, and written directly to the target OTA partition.
//!
//! # Limitation: IP-only resolution (no DNS)
//!
//! The OTA URL must contain a dotted-quad IPv4 address (e.g. `http://192.168.1.100:8080/firmware.bin`).
//! Hostnames are **not** resolved — there is no DNS lookup. If a hostname is provided,
//! OTA will fail with a clear error message. This restricts OTA to LAN IP addresses only.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
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

/// Perform an OTA update by downloading firmware from the given HTTP URL.
///
/// Creates a TCP connection, sends an HTTP GET request, validates the
/// HTTP status, skips headers, and writes the body to the OTA partition.
/// On success, finalizes and triggers a software reset.
pub async fn perform_ota_update(
    stack: &'static Stack<'static>,
    ota: &mut EspOta,
    firmware_url: &str,
) -> Result<(), ()> {
    let (host, port, path) = match parse_http_url(firmware_url) {
        Some(v) => v,
        None => {
            error!("OTA: invalid URL: {}", firmware_url);
            return Err(());
        }
    };

    info!("OTA: downloading from {}:{}{}", host, port, path);

    // Create TCP socket with OTA-sized buffers
    let rx_buf = mk_static!([u8; 4096], [0u8; 4096]);
    let tx_buf = mk_static!([u8; 1024], [0u8; 1024]);
    let mut socket = TcpSocket::new(*stack, rx_buf, tx_buf);
    socket.set_timeout(Some(Duration::from_secs(30)));

    // Resolve host IP (no DNS — only dotted-quad IPv4 is supported)
    let addr = match net_util::parse_ip(&host) {
        Some(a) => a,
        None => {
            error!(
                "OTA: hostname '{}' is not a valid IPv4 address. \
                 Only dotted-quad IPs are supported (no DNS). \
                 Use an IP address like http://192.168.1.100:8080/firmware.bin",
                host
            );
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
    let mut header_buf = Vec::new();
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
        if header_buf.len() + n > MAX_HEADER_SIZE {
            error!(
                "OTA: headers exceed {} bytes, aborting",
                MAX_HEADER_SIZE
            );
            return Err(());
        }
        header_buf.extend_from_slice(&buf[..n]);

        if let Some(pos) = find_header_end(&header_buf) {
            // Validate HTTP status line before proceeding
            if !validate_http_status(&header_buf) {
                let status_line = extract_status_line(&header_buf);
                error!("OTA: HTTP status not 200: {}", status_line);
                return Err(());
            }

            // HTTP response validated — now safe to erase target partition
            info!("OTA: HTTP 200 OK, erasing target partition");
            if let Err(e) = ota.begin() {
                error!("OTA: begin failed: {:?}", e);
                return Err(());
            }

            // Write any body data that arrived with the headers
            let body_start = pos + 4;
            if body_start < header_buf.len() {
                if let Err(e) = ota.write(&header_buf[body_start..]) {
                    error!("OTA: write failed: {:?}", e);
                    ota_rollback(ota);
                    return Err(());
                }
                total_written += (header_buf.len() - body_start) as u32;
            }
            header_buf.clear();
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
    if headers.len() < 12 {
        return false;
    }
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
