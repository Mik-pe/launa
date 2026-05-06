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
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_time::Duration;
use embedded_io_async::Write as _;
use launa_esp_ota::{EspOtaFlash, Partition};
use launa_ota::http::{
    extract_status_line, find_header_end, parse_content_length, parse_crc_from_url, parse_http_url,
    validate_http_status,
};
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

/// TCP socket buffer sizes for OTA connections.
const OTA_SOCKET_RX_BUF_SIZE: usize = 4096;
const OTA_SOCKET_TX_BUF_SIZE: usize = 1024;

/// HTTP response body read buffer size.
const HTTP_READ_BUF_SIZE: usize = 1024;

/// OTA partition size in bytes (matches partitions.csv).
const APP_PARTITION_SIZE: u32 = 0x140000;

/// TCP socket timeout for OTA firmware downloads.
const OTA_DOWNLOAD_TIMEOUT_SECS: u32 = 30;

/// TCP socket timeout for the OTA connectivity test.
const OTA_TCP_TEST_TIMEOUT_SECS: u32 = 10;

/// TCP socket buffers for OTA, allocated once and reused across attempts.
///
/// Without this, every `perform_ota_update` call would allocate 5 KiB via
/// `mk_static!` that is never reclaimed on failure (the device doesn't reboot),
/// permanently shrinking the 32 KiB heap.
pub struct OtaBuffers {
    rx_buf: &'static mut [u8; OTA_SOCKET_RX_BUF_SIZE],
    tx_buf: &'static mut [u8; OTA_SOCKET_TX_BUF_SIZE],
}

impl OtaBuffers {
    /// Allocate the OTA TCP socket buffers. Call once at startup.
    pub fn new() -> Self {
        let rx_buf = mk_static!([u8; OTA_SOCKET_RX_BUF_SIZE], [0u8; OTA_SOCKET_RX_BUF_SIZE]);
        let tx_buf = mk_static!([u8; OTA_SOCKET_TX_BUF_SIZE], [0u8; OTA_SOCKET_TX_BUF_SIZE]);
        Self { rx_buf, tx_buf }
    }

    /// Create a TcpSocket from these pre-allocated buffers.
    ///
    /// SAFETY: The caller must ensure no other TcpSocket is currently using
    /// these buffers (the previous socket must have been dropped).
    pub fn create_socket(&mut self, stack: &'static Stack<'static>) -> TcpSocket<'static> {
        // SAFETY: We are the only task accessing these buffers. The previous
        // TcpSocket (if any) was dropped at the end of the last call.
        let rx: &'static mut [u8] =
            unsafe { &mut *(self.rx_buf as *mut [u8; OTA_SOCKET_RX_BUF_SIZE] as *mut [u8]) };
        let tx: &'static mut [u8] =
            unsafe { &mut *(self.tx_buf as *mut [u8; OTA_SOCKET_TX_BUF_SIZE] as *mut [u8]) };
        TcpSocket::new(*stack, rx, tx)
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
    let mut socket = buffers.create_socket(stack);
    socket.set_timeout(Some(Duration::from_secs(OTA_DOWNLOAD_TIMEOUT_SECS as u64)));

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
    let mut buf = [0u8; HTTP_READ_BUF_SIZE];
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
            error!("OTA: headers exceed {} bytes, aborting", MAX_HEADER_SIZE);
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
                if content_length > APP_PARTITION_SIZE {
                    error!(
                        "OTA: Content-Length {} exceeds partition size {}",
                        content_length, APP_PARTITION_SIZE
                    );
                    ota_rollback(ota);
                    return Err(());
                }
                info!(
                    "OTA: Content-Length {} bytes (partition size {})",
                    content_length, APP_PARTITION_SIZE
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

/// TCP connectivity test for verifying OTA server reachability.
///
/// Connects to the OTA HTTP server, sends a GET request for `/firmware.bin`,
/// and logs the response without performing any OTA operation. Useful for
/// diagnosing network issues before attempting a real firmware update.
///
/// Does NOT reset the device on completion — returns Ok(()) or Err(()).
pub async fn tcp_test(
    stack: &'static Stack<'static>,
    firmware_url: &str,
    buffers: &mut OtaBuffers,
) -> Result<(), ()> {
    let (host, port, path) = match parse_http_url(firmware_url) {
        Some(v) => v,
        None => {
            error!("TCP_TEST: invalid URL: {}", firmware_url);
            return Err(());
        }
    };

    info!(
        "TCP_TEST: parsed URL -> host={} port={} path={}",
        host, port, path
    );

    // Reuse pre-allocated TCP socket buffers (same pattern as perform_ota_update)
    let mut socket = buffers.create_socket(stack);
    socket.set_timeout(Some(Duration::from_secs(OTA_TCP_TEST_TIMEOUT_SECS as u64)));

    info!("TCP_TEST: resolving host '{}'", host);
    let addr = match net_util::resolve_host(stack, &host).await {
        Some(a) => {
            info!(
                "TCP_TEST: resolved {} -> {}.{}.{}.{}",
                host, a[0], a[1], a[2], a[3]
            );
            a
        }
        None => {
            error!("TCP_TEST: failed to resolve host '{}'", host);
            return Err(());
        }
    };

    info!(
        "TCP_TEST: connecting to {}.{}.{}.{}:{} ...",
        addr[0], addr[1], addr[2], addr[3], port
    );
    if let Err(e) = socket
        .connect(IpEndpoint {
            addr: IpAddress::Ipv4(Ipv4Address::from_octets(addr)),
            port,
        })
        .await
    {
        error!("TCP_TEST: TCP connect failed: {:?}", e);
        return Err(());
    }
    info!("TCP_TEST: connected to {}:{}", host, port);

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    );
    info!("TCP_TEST: sending HTTP request ({} bytes)", request.len());
    if let Err(e) = socket.write_all(request.as_bytes()).await {
        error!("TCP_TEST: failed to send HTTP request: {:?}", e);
        return Err(());
    }
    info!("TCP_TEST: request sent, waiting for response");

    let mut buf = [0u8; HTTP_READ_BUF_SIZE];
    let mut total_read: usize = 0;

    loop {
        let n = match socket.read(&mut buf[total_read..]).await {
            Ok(0) => {
                info!(
                    "TCP_TEST: server closed connection (total read: {} bytes)",
                    total_read
                );
                break;
            }
            Ok(n) => {
                info!("TCP_TEST: read {} bytes (total: {})", n, total_read + n);
                n
            }
            Err(e) => {
                error!("TCP_TEST: read error: {:?}", e);
                return Err(());
            }
        };

        total_read += n;

        // Stop after filling the buffer or reading ~512 bytes
        if total_read >= HTTP_READ_BUF_SIZE {
            info!(
                "TCP_TEST: buffer full ({} bytes), stopping read",
                total_read
            );
            break;
        }
    }

    if total_read == 0 {
        error!("TCP_TEST: no data received from server");
        return Err(());
    }

    // Log HTTP status line (first line up to \r\n)
    let response = &buf[..total_read];
    if let Some(line_end) = response.iter().position(|&b| b == b'\r') {
        let status_line = core::str::from_utf8(&response[..line_end]).unwrap_or("<non-utf8>");
        info!("TCP_TEST: HTTP status: {}", status_line);
    } else {
        info!(
            "TCP_TEST: raw response (no status line found): {:02x?}",
            &response[..total_read.min(64)]
        );
    }

    // Log first 32 bytes of response as hex for debugging
    let hex_len = total_read.min(32);
    info!(
        "TCP_TEST: first {} bytes: {:02x?}",
        hex_len,
        &response[..hex_len]
    );

    // Try to find and log the start of the body
    if let Some(pos) = response.windows(4).position(|w| w == b"\r\n\r\n") {
        let body_start = pos + 4;
        let body_len = total_read.saturating_sub(body_start).min(32);
        if body_len > 0 {
            info!(
                "TCP_TEST: body starts at offset {}, first {} bytes: {:02x?}",
                body_start,
                body_len,
                &response[body_start..body_start + body_len]
            );
        } else {
            info!(
                "TCP_TEST: headers end at offset {} but no body data in this chunk",
                body_start
            );
        }
    }

    info!("TCP_TEST: SUCCESS ({} bytes read)", total_read);
    Ok(())
}

/// Roll back OTA and immediately reboot. Used on download/write failures
/// to ensure the device doesn't continue running with a wiped partition.
fn ota_rollback(ota: &mut EspOta) {
    error!("OTA: rolling back and rebooting");
    if let Err(e) = ota.rollback_and_reboot() {
        error!("OTA: rollback failed: {:?}", e);
    }
    esp_hal::system::software_reset()
}
