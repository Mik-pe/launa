//! Flash the RS-485 debugger firmware to an ESP32.
//!
//! Usage: cargo xtask rs485-debugger-flash [--port <device> | --port-index <N>] [--monitor]
//!
//! Reads WiFi/MQTT config from launa.toml and passes it as build-time env vars
//! to the firmware so it can connect to WiFi and publish MQTT status.

use anyhow::{bail, Context};
use std::process::Command;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port = None;
    let mut serial = None;
    let mut port_index = None;
    let mut monitor = false;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--port" => port = Some(parser.value("--port")?.to_string()),
            "--serial" => serial = Some(parser.value("--serial")?.to_string()),
            "--port-index" => port_index = parser.optional_parsed("--port-index")?,
            "--monitor" => {
                monitor = true;
                parser.skip();
            }
            _ => return Err(parser.unknown_arg()),
        }
    }

    let config = crate::config::load().context(
        "RS-485 debugger requires launa.toml for WiFi/MQTT config.\n\
         Copy launa.example.toml to launa.toml and fill in your values.",
    )?;
    let port = crate::util::resolve_port(port.as_deref(), serial.as_deref(), port_index, Some(&config))?;
    let port = Some(port);

    let emu_dir = crate::util::project_root().join("app-rs485-debugger");

    let mut cmd = Command::new("cargo");
    cmd.arg("+esp")
        .arg("espflash")
        .arg("flash")
        .arg("--chip")
        .arg("esp32");
    cmd.arg("--partition-table").arg("partitions.csv");

    if monitor {
        cmd.arg("--monitor");
    }
    if let Some(ref p) = port {
        cmd.arg("-p").arg(p);
    }

    // Pass WiFi/MQTT config as build-time env vars
    cmd.env("LAUNA_WIFI_SSID", &config.wifi.ssid);
    cmd.env("LAUNA_WIFI_PASSWORD", &config.wifi.password);
    cmd.env("LAUNA_MQTT_HOST", &config.mqtt.host);
    cmd.env("LAUNA_MQTT_PORT", config.mqtt.port.to_string());

    cmd.current_dir(&emu_dir);

    println!("Flashing RS-485 debugger firmware...");
    println!("  Working dir: {}", emu_dir.display());
    if let Some(ref p) = port {
        println!("  Port: {}", p);
    }
    if monitor {
        println!("  Monitor: enabled (serial log after flash)");
    }
    println!("  WiFi SSID: {}", config.wifi.ssid);
    println!("  MQTT host: {}:{}", config.mqtt.host, config.mqtt.port);

    let status = cmd
        .status()
        .context("Failed to run cargo espflash. Is cargo-espflash installed?")?;

    if status.success() {
        println!("RS-485 debugger flash successful.");
        Ok(())
    } else {
        bail!(
            "RS-485 debugger flash failed with exit code {:?}",
            status.code()
        );
    }
}
