//! Flash the spa-emulator firmware to an ESP32.
//!
//! Usage: cargo xtask spa-emulator-flash [--port <device> | --port-index <N>] [--monitor]

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

    let config = crate::config::load_without_serial_port_check()
        .map_err(|e| {
            eprintln!("Warning: could not load config for env vars: {}", e);
            e
        })
        .ok();
    let port = crate::util::resolve_port(port.as_deref(), serial.as_deref(), port_index, config.as_ref())?;
    let port = Some(port);

    let emu_dir = crate::util::project_root().join("app-spa-emulator");

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

    // Pass WiFi/MQTT config as build-time env vars if available
    if let Some(ref cfg) = config {
        cmd.env("LAUNA_WIFI_SSID", &cfg.wifi.ssid);
        cmd.env("LAUNA_WIFI_PASSWORD", &cfg.wifi.password);
        cmd.env("LAUNA_MQTT_HOST", &cfg.mqtt.host);
        cmd.env("LAUNA_MQTT_PORT", cfg.mqtt.port.to_string());
    }

    cmd.current_dir(&emu_dir);

    println!("Flashing spa-emulator firmware...");
    println!("  Working dir: {}", emu_dir.display());
    if let Some(ref p) = port {
        println!("  Port: {}", p);
    }
    if monitor {
        println!("  Monitor: enabled (serial log after flash)");
    }

    let status = cmd
        .status()
        .context("Failed to run cargo espflash. Is cargo-espflash installed?")?;

    if status.success() {
        println!("Spa-emulator flash successful.");
        Ok(())
    } else {
        bail!(
            "Spa-emulator flash failed with exit code {:?}",
            status.code()
        );
    }
}
