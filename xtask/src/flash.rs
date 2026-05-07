use anyhow::{bail, Context};
use std::process::Command;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut feature = None;
    let mut port = None;
    let mut serial = None;
    let mut port_index = None;
    let mut monitor = false;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--feature" => feature = Some(parser.value("--feature")?.to_string()),
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
    let port = crate::util::resolve_port(
        port.as_deref(),
        serial.as_deref(),
        port_index,
        config.as_ref(),
    )?;

    let app_dir = crate::util::project_root().join("app");
    let partitions_csv = app_dir.join("partitions.csv");
    let entries = crate::util::parse_partitions_csv(&partitions_csv)?;

    // Erase otadata + both OTA slots so the bootloader falls back to factory.
    // Without this, a device that previously did an OTA update keeps booting
    // from the OTA slot even after a USB flash overwrites factory.
    // ESP-IDF bootloader with invalid otadata tries ota_0 first, so we must
    // also erase ota_0 and ota_1 to guarantee factory boot.
    let erase_names = ["otadata", "ota_0", "ota_1"];

    for name in &erase_names {
        let part = crate::util::find_partition(&entries, name)?;
        println!(
            "Erasing {} partition (0x{:X}, {} bytes)...",
            name, part.offset, part.size
        );
        let mut erase_cmd = Command::new("cargo");
        erase_cmd
            .arg("+esp")
            .arg("espflash")
            .arg("erase-region")
            .arg(format!("0x{:X}", part.offset))
            .arg(format!("0x{:X}", part.size))
            .arg("--chip")
            .arg("esp32")
            .arg("-p")
            .arg(&port);
        erase_cmd.current_dir(&app_dir);

        let erase_status = erase_cmd
            .status()
            .with_context(|| format!("Failed to erase {} partition", name))?;

        if !erase_status.success() {
            bail!(
                "{} erase failed with exit code {:?}",
                name,
                erase_status.code()
            );
        }
    }
    println!("All OTA partitions erased.");

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

    if let Some(ref f) = feature {
        cmd.arg("--features").arg(f);
    }
    cmd.arg("-p").arg(&port);

    cmd.current_dir(&app_dir);

    println!("Flashing firmware...");
    println!("  Working dir: {}", app_dir.display());
    if let Some(ref f) = feature {
        println!("  Feature: {}", f);
    }
    println!("  Port: {}", port);
    if monitor {
        println!("  Monitor: enabled (serial log after flash)");
    }

    let status = cmd
        .status()
        .context("Failed to run cargo espflash. Is cargo-espflash installed?")?;

    if status.success() {
        println!("Flash successful.");
        Ok(())
    } else {
        bail!("Flash failed with exit code {:?}", status.code());
    }
}
