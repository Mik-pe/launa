//! Flash the sniffer firmware to an ESP32.
//!
//! Usage: cargo xtask sniffer-flash [--port <device> | --port-index <N>] [--monitor]

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

    let config = crate::config::load().ok();
    let port = crate::util::resolve_port(port.as_deref(), serial.as_deref(), port_index, config.as_ref())?;
    let port = Some(port);

    let sniffer_dir = crate::util::project_root().join("app-sniffer");

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

    cmd.current_dir(&sniffer_dir);

    println!("Flashing sniffer firmware...");
    println!("  Working dir: {}", sniffer_dir.display());
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
        println!("Sniffer flash successful.");
        Ok(())
    } else {
        bail!("Sniffer flash failed with exit code {:?}", status.code());
    }
}
