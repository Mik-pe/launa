//! OTA firmware update support.
//!
//! Uses `launa-esp-ota` crate for real OTA operations backed by `esp-storage::FlashStorage`.
//! The `EspOtaFlash` struct implements the `OtaUpdate` trait from `launa-ota`.
//!
//! OTA HTTP download is performed over embassy-net TCP. The firmware is downloaded
//! in chunks, skipping HTTP headers, and written directly to the target OTA partition.

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

pub type EspOta = EspOtaFlash<esp_storage::FlashStorage<'static>>;

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

/// Create a new OTA updater from an existing FlashStorage.
/// Detects the actual running partition from otadata instead of hardcoding.
pub fn create_ota(flash: esp_storage::FlashStorage<'static>) -> EspOta {
    let mut temp = EspOtaFlash::new(flash, Partition::Ota0);
    let running = temp.detect_running_partition().unwrap_or(Partition::Ota0);
    let storage = temp.into_flash();
    EspOtaFlash::new(storage, running)
}

/// Perform an OTA update by downloading firmware from the given HTTP URL.
///
/// Creates a TCP connection, sends an HTTP GET request, skips headers,
/// and writes the body to the OTA partition. On success, finalizes and
/// triggers a software reset to apply the new firmware.
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

    // Resolve host IP
    let addr = match parse_ip(&host) {
        Some(a) => a,
        None => {
            error!("OTA: cannot resolve host IP: {}", host);
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

    // Begin OTA update (erase target partition)
    if let Err(e) = ota.begin() {
        error!("OTA: begin failed: {:?}", e);
        return Err(());
    }

    // Read response, skip HTTP headers, write body to OTA
    let mut buf = [0u8; 1024];
    let mut header_end = false;
    let mut total_written: u32 = 0;
    let mut header_buf = Vec::new();

    loop {
        let n = match socket.read(&mut buf).await {
            Ok(0) => break, // Connection closed by server
            Ok(n) => n,
            Err(e) => {
                error!("OTA: read error: {:?}", e);
                let _ = ota.rollback_and_reboot();
                return Err(());
            }
        };

        if !header_end {
            // Accumulate data until we find the header/body boundary
            header_buf.extend_from_slice(&buf[..n]);

            if let Some(pos) = find_header_end(&header_buf) {
                header_end = true;
                let body_start = pos + 4;
                if body_start < header_buf.len() {
                    if let Err(e) = ota.write(&header_buf[body_start..]) {
                        error!("OTA: write failed: {:?}", e);
                        let _ = ota.rollback_and_reboot();
                        return Err(());
                    }
                    total_written += (header_buf.len() - body_start) as u32;
                }
                header_buf.clear(); // Free the memory
            }
            // else: still reading headers, continue accumulating
        } else {
            // Past headers — write body directly to OTA
            if let Err(e) = ota.write(&buf[..n]) {
                error!("OTA: write failed: {:?}", e);
                let _ = ota.rollback_and_reboot();
                return Err(());
            }
            total_written += n as u32;
        }
    }

    // Finalize the OTA update
    if let Err(e) = ota.finalize() {
        error!("OTA: finalize failed: {:?}", e);
        let _ = ota.rollback_and_reboot();
        return Err(());
    }

    info!(
        "OTA: {} bytes written successfully, rebooting",
        total_written
    );

    // Software reset to apply new firmware
    esp_hal::system::software_reset()
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

/// Parse an IPv4 address string into [u8; 4].
fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<u8> = s
        .split('.')
        .filter_map(|p| p.parse::<u8>().ok())
        .collect();
    if parts.len() == 4 {
        Some([parts[0], parts[1], parts[2], parts[3]])
    } else {
        None
    }
}
