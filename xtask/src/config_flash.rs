use anyhow::{bail, Context};
use serialport::{DataBits, FlowControl, Parity, StopBits};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Strip ANSI escape sequences from a string for reliable matching.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip the entire escape sequence
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if nc.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut cli_port = None;
    let mut serial = None;
    let mut port_index = None;
    let mut no_reset = false;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--port" => cli_port = Some(parser.value("--port")?.to_string()),
            "--serial" => serial = Some(parser.value("--serial")?.to_string()),
            "--port-index" => port_index = parser.optional_parsed("--port-index")?,
            "--no-reset" => {
                no_reset = true;
                parser.skip();
            }
            _ => return Err(parser.unknown_arg()),
        }
    }

    let config = crate::config::load()?;
    let port_name = crate::util::resolve_port(
        cli_port.as_deref(),
        serial.as_deref(),
        port_index,
        Some(&config),
    )?;

    // Build config payload
    let mut payload = String::from("CONFIG_START\r\n");
    payload.push_str(&format!("wifi.ssid={}\r\n", config.wifi.ssid));
    payload.push_str(&format!("wifi.password={}\r\n", config.wifi.password));
    payload.push_str(&format!("mqtt.host={}\r\n", config.mqtt.host));
    payload.push_str(&format!("mqtt.port={}\r\n", config.mqtt.port));
    if !config.mqtt.user.is_empty() {
        payload.push_str(&format!("mqtt.user={}\r\n", config.mqtt.user));
    }
    if !config.mqtt.password.is_empty() {
        payload.push_str(&format!("mqtt.password={}\r\n", config.mqtt.password));
    }
    payload.push_str(&format!("device.id={}\r\n", config.device.id));
    payload.push_str("CONFIG_END\r\n");

    println!("Connecting to {}...", port_name);
    let mut port = serialport::new(&port_name, 115200)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .timeout(Duration::from_millis(100))
        .open()
        .with_context(|| format!("Failed to open {}", port_name))?;

    // Drain any stale data in the RX buffer
    let mut drain = [0u8; 4096];
    let _ = port.read(&mut drain);

    if !no_reset {
        // Reset ESP32 via DTR/RTS (standard auto-reset circuit on NodeMCU-style boards).
        println!("Resetting ESP32...");
        port.write_data_terminal_ready(false)
            .context("Failed to set DTR")?;
        port.write_request_to_send(true)
            .context("Failed to set RTS")?;
        std::thread::sleep(Duration::from_millis(100));
        port.write_request_to_send(false)
            .context("Failed to set RTS")?;
        println!("Waiting for ESP32 to boot...");
        std::thread::sleep(Duration::from_millis(5000));
        // Drain any boot output
        let _ = port.read(&mut drain);
    }

    println!("Sending config...");
    // Send line-by-line with 50ms delays to avoid overflowing the ESP32's
    // 128-byte UART RX FIFO. The total config payload (~180 bytes) exceeds
    // the FIFO size, so sending all at once causes silent data loss.
    for line in payload.lines() {
        port.write_all(line.as_bytes())
            .context("Failed to write config line")?;
        port.write_all(b"\r\n")
            .context("Failed to write line ending")?;
        port.flush().context("Failed to flush")?;
        std::thread::sleep(Duration::from_millis(150));
    }

    // Wait for CONFIG_OK or CONFIG_ERROR
    let resp_deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; 4096];
    let mut response = String::new();
    while Instant::now() < resp_deadline {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                response.push_str(&String::from_utf8_lossy(&buf[..n]));
                let clean = strip_ansi(&response);
                if clean.contains("CONFIG_OK") {
                    println!("Config written successfully!");
                    return Ok(());
                }
                if clean.contains("CONFIG_ERROR") {
                    bail!("ESP32 config error: {}", clean.trim());
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => bail!("Serial read error: {}", e),
        }
    }
    bail!(
        "No CONFIG_OK response within 10s. Output:\n{}",
        strip_ansi(&response)
    );
}
