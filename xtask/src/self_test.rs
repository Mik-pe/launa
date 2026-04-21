use anyhow::{bail, Context};
use std::io::Read;
use std::process::Command;
use std::time::{Duration, Instant};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port_name = None;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--port" => port_name = Some(parser.value("--port")?.to_string()),
            _ => return Err(parser.unknown_arg()),
        }
    }

    let config = crate::config::load().ok();
    let port_name = crate::util::resolve_port(port_name.as_deref(), config.as_ref())?;

    // Step 1: Flash with hw-test feature
    println!("Building and flashing self-test firmware...");
    let app_dir = crate::util::project_root().join("app");
    let status = Command::new("cargo")
        .args(&[
            "+esp",
            "espflash",
            "flash",
            "--chip",
            "esp32",
            "--partition-table",
            "partitions.csv",
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
