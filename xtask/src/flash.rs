use anyhow::{bail, Context};
use std::process::Command;

/// otadata partition offset and size from partitions.csv.
/// Erasing this before USB flash ensures the bootloader doesn't
/// keep booting from a stale OTA slot after a USB flash.
const OTADATA_OFFSET: &str = "0x10000";
const OTADATA_SIZE: &str = "0x2000";

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut feature = None;
    let mut port = None;
    let mut monitor = false;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--feature" => feature = Some(parser.value("--feature")?.to_string()),
            "--port" => port = Some(parser.value("--port")?.to_string()),
            "--monitor" => {
                monitor = true;
                parser.skip();
            }
            _ => return Err(parser.unknown_arg()),
        }
    }

    let app_dir = crate::util::project_root().join("app");

    // Erase otadata partition so the bootloader falls back to the factory
    // partition. Without this, an OTA-updated device keeps booting from the
    // OTA slot even after a USB flash overwrites factory.
    println!("Erasing otadata partition to clear OTA boot selection...");
    let mut erase_cmd = Command::new("cargo");
    erase_cmd
        .arg("+esp")
        .arg("espflash")
        .arg("erase-region")
        .arg(OTADATA_OFFSET)
        .arg(OTADATA_SIZE)
        .arg("--chip")
        .arg("esp32");
    if let Some(ref p) = port {
        erase_cmd.arg("-p").arg(p);
    }
    erase_cmd.current_dir(&app_dir);

    let erase_status = erase_cmd
        .status()
        .context("Failed to run espflash erase-region")?;

    if !erase_status.success() {
        bail!(
            "otadata erase failed with exit code {:?}",
            erase_status.code()
        );
    }
    println!("otadata erased.");

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
    if let Some(ref p) = port {
        cmd.arg("-p").arg(p);
    }

    cmd.current_dir(&app_dir);

    println!("Flashing firmware...");
    println!("  Working dir: {}", app_dir.display());
    if let Some(ref f) = feature {
        println!("  Feature: {}", f);
    }
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
        println!("Flash successful.");
        Ok(())
    } else {
        bail!("Flash failed with exit code {:?}", status.code());
    }
}
