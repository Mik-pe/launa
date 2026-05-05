use anyhow::{bail, Context};
use rand::Rng;
use std::fs;
use std::process::Command;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    // Parse arguments
    let mut port_name = None;
    let mut serial = None;
    let mut port_index = None;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--port" => port_name = Some(parser.value("--port")?.to_string()),
            "--serial" => serial = Some(parser.value("--serial")?.to_string()),
            "--port-index" => port_index = parser.optional_parsed("--port-index")?,
            "--no-confirm" => {
                // Accepted for backward compat but ignored — espefuse is always
                // called with --no-confirm since the user already confirmed via xtask.
                parser.skip();
            }
            _ => return Err(parser.unknown_arg()),
        }
    }

    // Resolve serial port from config if not provided via CLI
    let config = crate::config::load().ok();
    let port_name = crate::util::resolve_port(port_name.as_deref(), serial.as_deref(), port_index, config.as_ref())?;

    // Determine keychain username from config device ID
    let keychain_user = config
        .as_ref()
        .map(|c| c.device.id.as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or("default");

    // Generate a full 32-byte random block for eFuse BLOCK3 (256 bits).
    // The firmware reads words 2, 4, 6, 7 (bytes 8-11, 16-19, 24-27, 28-31)
    // and XOR-mixes them into a 128-bit AES key. Filling the entire block
    // ensures all 4 sourced words contain random data.
    let block_data: [u8; 32] = rand::thread_rng().gen();

    // Write key to a temporary file (cleaned up after espefuse completes)
    let temp_dir = std::env::temp_dir();
    let random_suffix: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(12)
        .map(char::from)
        .collect();
    let key_path = temp_dir.join(format!("launa-key-{}.tmp", random_suffix));

    fs::write(&key_path, block_data)
        .with_context(|| format!("Failed to write key to temp file {}", key_path.display()))?;

    println!(
        "Generated 32-byte random eFuse block in temp file: {}",
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

    // Pass the key via stdin to avoid confirmation prompts.
    // Newer espefuse (v5+) removed --no-confirm; instead we pipe "BURN" to stdin.
    let burn_result = Command::new(&espefuse)
        .args([
            "--port",
            &port_name,
            "burn-block-data",
            "BLOCK3",
            key_path.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to run {}", espefuse))
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                // Pipe confirmation responses — espefuse may prompt multiple times
                let _ = stdin.write_all(b"BURN\nBURN\nBURN\nBURN\n");
            }
            child
                .wait()
                .with_context(|| format!("Failed to wait for {}", espefuse))
        });

    // Always clean up the temp file
    if let Err(e) = fs::remove_file(&key_path) {
        eprintln!("Warning: could not remove temp key file: {}", e);
    } else {
        println!("Temp key file cleaned up.");
    }

    let status = burn_result?;
    if !status.success() {
        bail!(
            "espefuse burn-block-data failed.\n\
             Check the error above and try again."
        );
    }

    // Print confirmation with first 4 bytes (hex) for verification
    println!();
    println!("✓ eFuse BLOCK3 provisioned successfully!");
    println!(
        "  Key preview (first 4 bytes): {:02x}{:02x}{:02x}{:02x}",
        block_data[0], block_data[1], block_data[2], block_data[3]
    );

    // Store the key in the OS keychain
    let key_hex: String = block_data.iter().map(|b| format!("{:02x}", b)).collect();
    match store_key_in_keychain(keychain_user, &key_hex) {
        Ok(()) => {
            println!(
                "  Key stored in OS keychain (service: \"launa\", user: \"{}\").",
                keychain_user
            );
        }
        Err(e) => {
            eprintln!();
            eprintln!("⚠ Could not store key in OS keychain: {}", e);
            eprintln!("  Save this key securely — it cannot be recovered from eFuse:");
            eprintln!("  {}", key_hex);
        }
    }

    println!();

    Ok(())
}

/// Try to store the key in the OS keychain using the `keyring` crate.
fn store_key_in_keychain(user: &str, key_hex: &str) -> anyhow::Result<()> {
    let entry = keyring::Entry::new("launa", user)
        .map_err(|e| anyhow::anyhow!("Failed to create keyring entry: {}", e))?;
    entry
        .set_password(key_hex)
        .map_err(|e| anyhow::anyhow!("Failed to set keyring password: {}", e))?;
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
