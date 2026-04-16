use anyhow::{bail, Context};
use std::io::{Read, Write};
use std::time::Duration;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port_name = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                if i >= args.len() {
                    bail!("--port requires a value");
                }
                port_name = Some(args[i].clone());
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let config = crate::config::load()?;
    let port_name = port_name.unwrap_or(config.device.serial_port.clone());

    println!("Writing config to ESP32 via {}...", port_name);

    let mut port = serialport::new(&port_name, 115200)
        .timeout(Duration::from_secs(5))
        .open()
        .with_context(|| format!("Failed to open serial port {}", port_name))?;

    // Send config using text protocol
    writeln!(port, "CONFIG_START").context("Failed to write CONFIG_START")?;
    writeln!(port, "wifi.ssid={}", config.wifi.ssid)?;
    writeln!(port, "wifi.password={}", config.wifi.password)?;
    writeln!(port, "mqtt.host={}", config.mqtt.host)?;
    writeln!(port, "mqtt.port={}", config.mqtt.port)?;
    if !config.mqtt.user.is_empty() {
        writeln!(port, "mqtt.user={}", config.mqtt.user)?;
    }
    if !config.mqtt.password.is_empty() {
        writeln!(port, "mqtt.password={}", config.mqtt.password)?;
    }
    writeln!(port, "device.id={}", config.device.id)?;
    writeln!(port, "CONFIG_END")?;
    port.flush()?;

    println!("Config sent. Waiting for acknowledgment...");

    // Wait for response
    let mut response = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; 256];

    while std::time::Instant::now() < deadline {
        match port.read(&mut buf) {
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                response.push_str(&text);
                if response.contains("CONFIG_OK") || response.contains("CONFIG_ERROR") {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                // Continue waiting
            }
            Err(e) => bail!("Serial read error: {}", e),
        }
    }

    if response.contains("CONFIG_OK") {
        println!("Config written successfully!");
        Ok(())
    } else if let Some(idx) = response.find("CONFIG_ERROR:") {
        let error = &response[idx + "CONFIG_ERROR:".len()..];
        bail!("ESP32 config error: {}", error.trim());
    } else {
        bail!(
            "No acknowledgment received from ESP32. Response: {}",
            response.trim()
        );
    }
}
