use anyhow::{bail, Context};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Shared OTA download state observed by the caller to track firmware download progress.
pub struct OtaProgress {
    /// Bytes sent so far in the current/last download. 0 = no connection yet.
    pub bytes_sent: AtomicUsize,
    /// Total firmware size in bytes.
    pub total_bytes: usize,
}

/// Run the OTA server with an externally-controlled shutdown flag.
/// Binds to `0.0.0.0:<port>` and serves the firmware file until `shutdown` is set.
/// Returns the actual bound address (useful for detecting OS-assigned port 0).
pub fn serve(
    port: u16,
    firmware_data: &[u8],
    quiet: bool,
    shutdown: &AtomicBool,
    progress: &OtaProgress,
) -> anyhow::Result<()> {
    let firmware_data = Arc::new(firmware_data.to_vec());
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr)
        .map_err(|e| anyhow::anyhow!("Failed to bind TCP on {}: {}", addr, e))?;
    listener
        .set_nonblocking(false)
        .map_err(|e| anyhow::anyhow!("Failed to set blocking mode: {}", e))?;

    if !quiet {
        let bound_addr = listener.local_addr()?;
        println!("OTA server running on http://{}/firmware.bin", bound_addr);
        println!("Firmware: {} bytes", firmware_data.len());
    }

    let accept_timeout = Duration::from_millis(500);

    while !shutdown.load(Ordering::SeqCst) {
        listener.set_nonblocking(true).ok();
        match listener.accept() {
            Ok((mut stream, peer)) => {
                stream.set_nonblocking(false).ok();
                let peer_str = peer.to_string();
                if !quiet {
                    println!("[{}] connected", peer_str);
                }

                let mut buf = [0u8; 4096];
                stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                stream.set_write_timeout(None).ok();

                match stream.read(&mut buf) {
                    Ok(0) => {
                        if !quiet {
                            println!("[{}] empty request, closing", peer_str);
                        }
                    }
                    Ok(n) => {
                        let request = String::from_utf8_lossy(&buf[..n]);
                        let first_line = request.lines().next().unwrap_or("");
                        if !quiet {
                            println!("[{}] request: {}", peer_str, first_line);
                        }

                        if first_line.contains("HTTP") {
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                firmware_data.len()
                            );
                            match stream.write_all(header.as_bytes()) {
                                Ok(()) => {
                                    let total = firmware_data.len();
                                    let mut offset = 0;
                                    let chunk_size = 4096;
                                    let mut ok = true;
                                    let mut last_pct = 0u8;
                                    while offset < total {
                                        let end = (offset + chunk_size).min(total);
                                        if let Err(e) =
                                            stream.write_all(&firmware_data[offset..end])
                                        {
                                            eprintln!(
                                                "[{}] write error at {} bytes: {}",
                                                peer_str, offset, e
                                            );
                                            ok = false;
                                            progress.bytes_sent.store(offset, Ordering::SeqCst);
                                            break;
                                        }
                                        offset = end;
                                        progress.bytes_sent.store(offset, Ordering::SeqCst);
                                        let pct = ((offset as u64 * 100) / total as u64) as u8;
                                        if pct >= last_pct + 1 || offset == total {
                                            let bar_len = 30;
                                            let filled = (pct as usize * bar_len) / 100;
                                            let bar: String =
                                                "=".repeat(filled) + &" ".repeat(bar_len - filled);
                                            eprint!(
                                                "\rDownloading firmware [{bar}] {pct:3}% ({}/{})",
                                                offset, total
                                            );
                                            last_pct = pct;
                                        }
                                    }
                                    if ok {
                                        eprintln!();
                                        if !quiet {
                                            println!(
                                                "[{}] GET -> 200 ({} bytes sent)",
                                                peer_str, total
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[{}] header write error: {}", peer_str, e);
                                }
                            }
                        } else {
                            let msg = format!("echo: {} bytes received\n", n);
                            let _ = stream.write_all(msg.as_bytes());
                            if !quiet {
                                println!("[{}] raw TCP: sent echo", peer_str);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[{}] read error: {}", peer_str, e);
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(accept_timeout);
            }
            Err(e) => {
                eprintln!("Accept error: {}", e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    if !quiet {
        println!("OTA server stopped.");
    }
    Ok(())
}

/// CLI entry point for `cargo xtask ota-serve`.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut firmware_path = None;
    let mut port = 8081u16;
    let mut quiet = false;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--firmware" => firmware_path = Some(PathBuf::from(parser.value("--firmware")?)),
            "--port" => {
                port = parser.value("--port")?.parse()?;
            }
            "--quiet" => {
                quiet = true;
                parser.skip();
            }
            _ => return Err(parser.unknown_arg()),
        }
    }

    let firmware_path = firmware_path.context("--firmware <path> is required")?;
    if !firmware_path.exists() {
        bail!("Firmware file not found: {}", firmware_path.display());
    }

    let firmware_data = std::fs::read(&firmware_path)
        .with_context(|| format!("Failed to read {}", firmware_path.display()))?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let r = shutdown.clone();
    crate::util::ctrlc_handler(move || {
        r.store(true, Ordering::SeqCst);
    });

    let progress = OtaProgress {
        bytes_sent: AtomicUsize::new(0),
        total_bytes: firmware_data.len(),
    };

    serve(port, &firmware_data, quiet, &shutdown, &progress)
}
