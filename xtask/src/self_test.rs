use anyhow::{bail, Context};
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port_name = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                port_name = Some(args[i].clone());
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let config = crate::config::load().ok();
    let port_name = port_name
        .or_else(|| config.map(|c| c.device.serial_port.clone()))
        .context("No serial port specified. Use --port or set device.serial_port in launa.toml")?;

    // Step 1: Flash with hw-test feature
    println!("Building and flashing self-test firmware...");
    let app_dir = project_root().join("app");
    let status = Command::new("cargo")
        .args(&[
            "espflash",
            "flash",
            "--chip",
            "esp32",
            "--features",
            "hw-test",
            "-p",
            &port_name,
        ])
        .current_dir(&app_dir)
        .status()
        .context("Failed to run cargo espflash")?;

    if !status.success() {
        bail!("Self-test flash failed.");
    }
    println!("Firmware flashed. Reading test results...\n");

    // Step 2: Open serial and read test output
    let port = serialport::new(&port_name, 115200)
        .timeout(Duration::from_millis(500))
        .open()
        .with_context(|| format!("Failed to open serial port {}", port_name))?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut port = port;
    let mut buf = [0u8; 256];
    let mut output = String::new();

    while Instant::now() < deadline {
        match port.read(&mut buf) {
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                print!("{}", text);
                output.push_str(&text);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => bail!("Serial read error: {}", e),
        }
    }

    // Step 3: Parse test results
    let mut passed = 0;
    let mut failed: Vec<String> = Vec::new();

    for line in output.lines() {
        if line.contains("TEST_PASS") {
            passed += 1;
        } else if let Some(idx) = line.find("TEST_FAIL:") {
            let reason = &line[idx + "TEST_FAIL:".len()..];
            failed.push(reason.trim().to_string());
        }
    }

    // Step 4: Report
    println!("\n--- Self-Test Results ---");
    println!("Passed: {}", passed);
    println!("Failed: {}", failed.len());
    for (i, reason) in failed.iter().enumerate() {
        println!("  {}. {}", i + 1, reason);
    }

    if failed.is_empty() && passed > 0 {
        println!("\nAll tests PASSED.");
        Ok(())
    } else if failed.is_empty() {
        bail!("No test results found in serial output.");
    } else {
        bail!("{} test(s) failed.", failed.len());
    }
}
