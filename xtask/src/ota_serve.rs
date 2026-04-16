use anyhow::{bail, Context};
use std::path::PathBuf;
use std::time::Duration;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut firmware_path = None;
    let mut port = 8080u16;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--firmware" => {
                i += 1;
                if i >= args.len() {
                    bail!("--firmware requires a value");
                }
                firmware_path = Some(PathBuf::from(&args[i]));
            }
            "--port" => {
                i += 1;
                if i >= args.len() {
                    bail!("--port requires a value");
                }
                port = args[i].parse().context("Invalid port")?;
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let firmware_path = firmware_path.context("--firmware <path> is required")?;
    if !firmware_path.exists() {
        bail!("Firmware file not found: {}", firmware_path.display());
    }

    let firmware_data = std::fs::read(&firmware_path)
        .with_context(|| format!("Failed to read {}", firmware_path.display()))?;

    let firmware_data = std::sync::Arc::new(firmware_data);
    let addr = format!("0.0.0.0:{}", port);
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("Failed to start HTTP server on {}: {}", addr, e))?;

    println!("OTA server running on http://{}/firmware.bin", addr);
    println!(
        "Serving: {} ({} bytes)",
        firmware_path.display(),
        firmware_data.len()
    );
    println!("Press Ctrl+C to stop.");

    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    let _ = ctrlc::set_handler(move || {
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    let timeout = Duration::from_secs(1);
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        match server.recv_timeout(timeout) {
            Ok(Some(request)) => {
                let remote = request
                    .remote_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let path = request.url().to_string();
                let data = firmware_data.clone();
                let len = data.len();

                let response = tiny_http::Response::new(
                    tiny_http::StatusCode::from(200),
                    vec![
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/octet-stream"[..],
                        )
                        .unwrap(),
                        tiny_http::Header::from_bytes(
                            &b"Content-Length"[..],
                            format!("{}", len).as_bytes(),
                        )
                        .unwrap(),
                    ],
                    std::io::Cursor::new((*data).clone()),
                    Some(len),
                    None,
                );

                if let Err(e) = request.respond(response) {
                    eprintln!("Error responding to {}: {}", remote, e);
                } else {
                    println!("[{}] GET {} -> 200 ({} bytes)", remote, path, len);
                }
            }
            Ok(None) => {} // timeout, check running flag
            Err(e) => {
                eprintln!("Server error: {}", e);
                break;
            }
        }
    }

    println!("OTA server stopped.");
    Ok(())
}
