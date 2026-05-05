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
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--port" => cli_port = Some(parser.value("--port")?.to_string()),
            "--serial" => serial = Some(parser.value("--serial")?.to_string()),
            "--port-index" => port_index = parser.optional_parsed("--port-index")?,
            _ => return Err(parser.unknown_arg()),
        }
    }

    let config = crate::config::load()?;
    let port_name = crate::util::resolve_port(cli_port.as_deref(), serial.as_deref(), port_index, Some(&config))?;

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

    // Reset ESP32 via DTR/RTS (standard auto-reset circuit on NodeMCU-style boards).
    // Circuit: RTS -> NPN -> EN, DTR -> NPN -> GPIO0
    // For normal boot: GPIO0 must be HIGH when EN rises.
    //   DTR=LOW (GPIO0=HIGH), RTS=HIGH (EN=LOW=assert reset), then RTS=LOW (EN=HIGH=release)
    println!("Resetting ESP32...");
    port.write_data_terminal_ready(false)
        .context("Failed to set DTR")?; // GPIO0 = HIGH (normal boot, not download)
    port.write_request_to_send(true)
        .context("Failed to set RTS")?; // EN = LOW (assert reset)
    std::thread::sleep(Duration::from_millis(100));
    port.write_request_to_send(false)
        .context("Failed to set RTS")?; // EN = HIGH (release reset)
                                        // Give ESP32 time to boot and enter the config window.
                                        // Boot takes ~500ms, then the app waits 5s for serial config.
                                        // Wait 1.5s to be safely past the bootloader and into the app.
    println!("Waiting for ESP32 to boot...");
    std::thread::sleep(Duration::from_millis(1500));

    // Drain any boot output
    let _ = port.read(&mut drain);

    println!("Sending config...");
    port.write_all(payload.as_bytes())
        .context("Failed to write config")?;
    port.flush().context("Failed to flush")?;

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
