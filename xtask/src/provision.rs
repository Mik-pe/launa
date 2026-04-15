use anyhow::{bail, Context};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    // Parse arguments
    let mut port_name = None;
    let mut skip_confirm = false;
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
            "--no-confirm" => {
                skip_confirm = true;
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    // Resolve serial port from config if not provided via CLI
    let config = crate::config::load().ok();
    let port_name = port_name
        .or_else(|| config.as_ref().map(|c| c.device.serial_port.clone()))
        .context("No serial port specified. Use --port or set device.serial_port in launa.toml")?;

    let key_path = project_root().join("launa.key");

    // Warn if launa.key already exists
    if key_path.exists() {
        if !skip_confirm {
            bail!(
                "Key file already exists: {}\nRefusing to overwrite. Delete it first or use --no-confirm to overwrite.",
                key_path.display()
            );
        }
        eprintln!(
            "Warning: Overwriting existing key file: {}",
            key_path.display()
        );
    }

    // Generate a random 16-byte AES key
    let key: [u8; 16] = rand::random();

    // Save key to launa.key (binary)
    fs::write(&key_path, &key)
        .with_context(|| format!("Failed to write key to {}", key_path.display()))?;

    println!(
        "Generated 16-byte AES key and saved to {}",
        key_path.display()
    );

    // Find espefuse.py
    let espefuse = find_espefuse().context(
        "espefuse.py not found. Install it with:\n  \
         pip install esptool\n\
         Or install ESP-IDF which includes esptool.",
    )?;

    // Burn the key to eFuse BLOCK3
    println!("Burning key to eFuse BLOCK3 on port {}...", port_name);

    let status = Command::new(&espefuse)
        .args(&[
            "--port",
            &port_name,
            "burn-block-data",
            "BLOCK3",
            key_path.to_str().unwrap(),
            "--no-confirm",
        ])
        .status()
        .with_context(|| format!("Failed to run {}", espefuse))?;

    if !status.success() {
        // Clean up the key file since burning failed
        let _ = fs::remove_file(&key_path);
        bail!(
            "espefuse burn-block-data failed. The key file has been removed.\n\
             Check the error above and try again."
        );
    }

    // Print confirmation with first 4 bytes (hex) for verification
    println!();
    println!("✓ eFuse BLOCK3 provisioned successfully!");
    println!(
        "  Key preview (first 4 bytes): {:02x}{:02x}{:02x}{:02x}",
        key[0], key[1], key[2], key[3]
    );
    println!("  Key file: {}", key_path.display());
    println!();
    println!("IMPORTANT: Back up launa.key securely. It cannot be recovered from eFuse.");

    Ok(())
}

/// Try to find espefuse.py on PATH or via common locations.
fn find_espefuse() -> Option<String> {
    // Try "espefuse.py" directly (standard pip install)
    if Command::new("espefuse.py").arg("--help").output().is_ok() {
        return Some("espefuse.py".to_string());
    }

    // Try "espefuse" (some installations omit .py)
    if Command::new("espefuse").arg("--help").output().is_ok() {
        return Some("espefuse".to_string());
    }

    None
}
