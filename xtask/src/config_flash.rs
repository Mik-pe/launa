use anyhow::{bail, Context};
use std::path::PathBuf;
use std::process::Command;

/// Locate the `scripts/config_flash.py` bundled alongside this crate.
fn script_path() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    PathBuf::from(manifest_dir)
        .join("scripts")
        .join("config_flash.py")
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port_name = None;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--port" => port_name = Some(parser.value("--port")?.to_string()),
            _ => return Err(parser.unknown_arg()),
        }
    }

    let config = crate::config::load()?;
    let port_name = port_name.unwrap_or(config.device.serial_port.clone());

    // Build config lines
    let mut lines = Vec::new();
    lines.push("CONFIG_START".to_string());
    lines.push(format!("wifi.ssid={}", config.wifi.ssid));
    lines.push(format!("wifi.password={}", config.wifi.password));
    lines.push(format!("mqtt.host={}", config.mqtt.host));
    lines.push(format!("mqtt.port={}", config.mqtt.port));
    if !config.mqtt.user.is_empty() {
        lines.push(format!("mqtt.user={}", config.mqtt.user));
    }
    if !config.mqtt.password.is_empty() {
        lines.push(format!("mqtt.password={}", config.mqtt.password));
    }
    lines.push(format!("device.id={}", config.device.id));
    lines.push("CONFIG_END".to_string());

    // Write config payload to a temp file so Python can read it (avoids escaping issues)
    let temp_dir = std::env::temp_dir();
    let config_file = temp_dir.join("launa_config_payload.txt");
    let payload: String = lines.join("\n");
    std::fs::write(&config_file, &payload).context("Failed to write temp config file")?;

    let script = script_path();
    println!(
        "Writing config to ESP32 via {} (using Python/pyserial)...",
        port_name
    );

    let output = Command::new("python")
        .arg(&script)
        .arg(&port_name)
        .arg(&config_file)
        .output()
        .context(
            "Failed to run Python. Is Python with pyserial installed? (pip install pyserial)",
        )?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !stderr.is_empty() {
        eprintln!("Python stderr: {}", stderr);
    }

    // Clean up temp config file
    let _ = std::fs::remove_file(&config_file);

    if !output.status.success() {
        bail!("Python config-flash script failed: {}", stderr);
    }

    if stdout.starts_with("CONFIG_OK") {
        println!("Config written successfully!");
        Ok(())
    } else if let Some(err) = stdout.strip_prefix("CONFIG_ERROR:") {
        bail!("ESP32 config error: {}", err);
    } else if let Some(msg) = stdout.strip_prefix("NO_RESPONSE:") {
        bail!("No acknowledgment from ESP32: {}", msg);
    } else {
        bail!("Unexpected response: {}", stdout);
    }
}
