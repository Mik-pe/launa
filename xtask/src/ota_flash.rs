use anyhow::{bail, Context};
use serde::Deserialize;
use std::io::Write as _;
use std::process::Command;
use std::time::Duration;

/// Offset of the factory/app partition in the merged flash image (from partitions.csv).
const APP_PARTITION_OFFSET: usize = 0x20000;

/// Read the firmware version from app/Cargo.toml and append the git short SHA,
/// matching the format used by the ESP32 firmware: "version (sha)".
fn read_firmware_version() -> anyhow::Result<String> {
    let app_cargo_toml = crate::util::project_root().join("app").join("Cargo.toml");
    let contents =
        std::fs::read_to_string(&app_cargo_toml).context("Failed to read app/Cargo.toml")?;

    #[derive(Deserialize)]
    struct CargoToml {
        package: CargoPackage,
    }
    #[derive(Deserialize)]
    struct CargoPackage {
        version: String,
    }

    let cargo: CargoToml = toml::from_str(&contents).context("Failed to parse app/Cargo.toml")?;
    let version = cargo.package.version;

    let short_sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    Ok(format!("{} ({})", version, short_sha))
}

/// Extract the firmware_version field from a JSON payload.
/// Returns None if the field is missing or the payload is not valid JSON.
pub fn extract_firmware_version(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .get("firmware_version")?
        .as_str()
        .map(|s| s.to_string())
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut feature = "default".to_string();
    let mut device_id_override = None;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--feature" => feature = parser.value("--feature")?.to_string(),
            "--device-id" => device_id_override = Some(parser.value("--device-id")?.to_string()),
            _ => return Err(parser.unknown_arg()),
        }
    }

    let config = crate::config::load_without_serial_port_check()?;
    let device_id = device_id_override.unwrap_or(config.device.id.clone());
    let ota_port = config.ota.serve_port;

    println!(
        "OTA flash: device={}, feature={}, ota_port={}",
        device_id, feature, ota_port
    );

    // Step 1: Run cargo test (exclude launa-server — it may be running and lock the binary)
    println!("\n[1/6] Running cargo test...");
    let test_status = Command::new("cargo")
        .args(["test", "--workspace", "--exclude", "launa-server"])
        .current_dir(crate::util::project_root())
        .status()
        .context("Failed to run cargo test")?;
    if !test_status.success() {
        bail!("cargo test failed. Aborting OTA flash.");
    }
    println!("Tests passed.");

    // Step 2: Build firmware
    println!("\n[2/6] Building firmware...");
    let target_dir = crate::util::project_root().join("target");
    let merged_path = target_dir.join("launa-merged.bin");
    let ota_bin_path = target_dir.join("launa-ota.bin");
    let app_dir = crate::util::project_root().join("app");

    let mut build_cmd = Command::new("cargo");
    build_cmd
        .arg("+esp")
        .arg("espflash")
        .arg("save-image")
        .arg("--chip")
        .arg("esp32")
        .arg("--merge")
        .arg("--partition-table")
        .arg("partitions.csv")
        .arg("--skip-padding");
    if feature != "default" {
        build_cmd.arg("--features").arg(&feature);
    }
    build_cmd.arg(&merged_path);
    build_cmd.current_dir(&app_dir);

    let build_status = build_cmd
        .status()
        .context("Failed to run cargo espflash save-image")?;
    if !build_status.success() {
        bail!("Firmware build failed.");
    }

    // Extract the app partition from the merged image for OTA.
    // The merged binary contains bootloader + partition table + app.
    // OTA only needs the raw app image (starts with 0xE9 magic).
    let merged_data = std::fs::read(&merged_path)
        .with_context(|| format!("Failed to read {}", merged_path.display()))?;
    if merged_data.len() <= APP_PARTITION_OFFSET {
        bail!(
            "Merged image too small ({} bytes), expected app at offset 0x{:X}",
            merged_data.len(),
            APP_PARTITION_OFFSET
        );
    }
    let app_image = &merged_data[APP_PARTITION_OFFSET..];
    if app_image[0] != 0xE9 {
        bail!(
            "App image does not start with ESP32 magic 0xE9, got 0x{:02X}",
            app_image[0]
        );
    }
    let mut f = std::fs::File::create(&ota_bin_path)
        .with_context(|| format!("Failed to create {}", ota_bin_path.display()))?;
    f.write_all(app_image)
        .context("Failed to write OTA binary")?;

    println!(
        "Firmware built: {} ({} bytes, app partition extracted)",
        ota_bin_path.display(),
        app_image.len()
    );

    // Read expected firmware version after build so git HEAD matches what was compiled in.
    let expected_version = read_firmware_version().ok();
    if let Some(ref v) = &expected_version {
        println!("Expected firmware version: {}", v);
    } else {
        println!("Warning: Could not determine expected firmware version");
    }

    // Step 3: Start OTA server in a background thread (same process, so it dies with us)
    println!("\n[3/6] Starting OTA server...");
    let server_addr = format!("127.0.0.1:{}", ota_port);
    if std::net::TcpStream::connect(&server_addr).is_ok() {
        bail!(
            "Port {} is already in use. Stop the existing OTA server first (e.g. `kill` the process listening on that port).",
            ota_port
        );
    }

    let ota_bin_data = std::fs::read(&ota_bin_path)
        .with_context(|| format!("Failed to read {}", ota_bin_path.display()))?;
    let server_shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server_shutdown_clone = server_shutdown.clone();
    let server_port = ota_port;

    let ota_progress = std::sync::Arc::new(crate::ota_serve::OtaProgress {
        bytes_sent: std::sync::atomic::AtomicUsize::new(0),
        total_bytes: ota_bin_data.len(),
    });
    let ota_progress_clone = ota_progress.clone();

    let server_thread = std::thread::spawn(move || {
        crate::ota_serve::serve(
            server_port,
            &ota_bin_data,
            true,
            &server_shutdown_clone,
            &ota_progress_clone,
        )
    });

    // Wait for server to be ready (verify with TCP connect)
    let server_addr = format!("127.0.0.1:{}", ota_port);
    for attempt in 1..=10 {
        std::thread::sleep(Duration::from_millis(500));
        if std::net::TcpStream::connect(&server_addr).is_ok() {
            println!("OTA server ready on port {}", ota_port);
            break;
        }
        if attempt == 10 {
            server_shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = server_thread.join();
            bail!("OTA server did not become ready on port {}", ota_port);
        }
        println!("Waiting for OTA server... (attempt {}/10)", attempt);
    }

    // Step 4: Trigger OTA and wait for device to download + reboot (with retries).
    // Auto-detect local IP since we host the OTA server on this machine.
    let ota_host = match local_ip_address::local_ip() {
        Ok(ip) => ip.to_string(),
        Err(_) => bail!(
            "Failed to auto-detect local IP address. Set ota.host in launa.toml or check network."
        ),
    };
    println!("OTA server address: {}:{}", ota_host, ota_port);
    let firmware_url = format!("http://{}:{}/firmware.bin", ota_host, ota_port);

    let mut mqttoptions = rumqttc::MqttOptions::new(
        format!("xtask-ota-flash-{}", device_id),
        &config.mqtt.host,
        config.mqtt.port,
    );
    mqttoptions.set_keep_alive(Duration::from_secs(5));

    let (client, mut connection) = rumqttc::Client::new(mqttoptions, 10);

    // Subscribe early so we can drain any stale retained messages before
    // sending the OTA command.  The availability topic has retain=true,
    // so subscribing now delivers the current-boot "online" immediately.
    let status_topic = format!("launa/{}/state", device_id);
    let avail_topic = format!("launa/{}/availability", device_id);
    let diag_topic = format!("launa/{}/diagnostics", device_id);
    client.subscribe(&status_topic, rumqttc::QoS::AtLeastOnce)?;
    client.subscribe(&avail_topic, rumqttc::QoS::AtLeastOnce)?;
    client.subscribe(&diag_topic, rumqttc::QoS::AtLeastOnce)?;

    // Drain stale retained messages from the current boot
    let drain_deadline = std::time::Instant::now() + Duration::from_secs(3);
    for notification in connection.iter() {
        if std::time::Instant::now() > drain_deadline {
            break;
        }
        match notification {
            Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(_))) => {}
            Ok(rumqttc::Event::Incoming(rumqttc::Packet::SubAck(_))) => {}
            Ok(_) => {}
            Err(_) => break,
        }
    }

    // Step 4: Publish OTA command and wait for device to download + reboot.
    const MAX_TRIGGER_ATTEMPTS: usize = 3;
    const DOWNLOAD_START_TIMEOUT_SECS: u64 = 30;
    const REBOOT_TIMEOUT_SECS: u64 = 120;

    let ota_topic = format!("launa/{}/ota", device_id);
    let ota_payload = serde_json::json!({ "url": firmware_url });
    let mut came_online = false;
    let mut version_payload: Option<String> = None;
    let mut download_complete = false;

    // Publish the OTA command once. The MQTT message is QoS 1 (AtLeastOnce)
    // so the broker will deliver it even if the device is briefly disconnected.
    // Do NOT republish on retries — the device may have already received it and
    // started downloading; a second OTA command would queue up and trigger an
    // unwanted re-OTA after reboot.
    println!("\n[4/6] Publishing OTA command to device...");
    client
        .publish(
            &ota_topic,
            rumqttc::QoS::AtLeastOnce,
            false,
            ota_payload.to_string().as_bytes(),
        )
        .context("Failed to publish OTA command")?;
    println!("OTA command published. Waiting for device to start download...");

    // Phase 1: Wait for the device to connect to the OTA server and start downloading.
    // The device must: receive MQTT → forward URL to main loop → start HTTP download.
    // This can take several seconds depending on where in the event loop the
    // device is when the MQTT message arrives.
    let mut download_started = false;
    for attempt in 1..=MAX_TRIGGER_ATTEMPTS {
        // Check if download already started (from a previous attempt's MQTT message)
        let bytes = ota_progress
            .bytes_sent
            .load(std::sync::atomic::Ordering::SeqCst);
        if bytes > 0 {
            download_started = true;
            break;
        }

        if attempt > 1 {
            // Re-publish only if we haven't seen any download activity.
            // The broker may have lost the message or the device didn't process it.
            println!(
                "No download activity, re-publishing OTA command (attempt {}/{})...",
                attempt, MAX_TRIGGER_ATTEMPTS
            );
            client
                .publish(
                    &ota_topic,
                    rumqttc::QoS::AtLeastOnce,
                    false,
                    ota_payload.to_string().as_bytes(),
                )
                .context("Failed to re-publish OTA command")?;
        }

        let phase1_deadline =
            std::time::Instant::now() + Duration::from_secs(DOWNLOAD_START_TIMEOUT_SECS);
        loop {
            let bytes = ota_progress
                .bytes_sent
                .load(std::sync::atomic::Ordering::SeqCst);
            if bytes > 0 {
                println!("Device connected, firmware download started!");
                download_started = true;
                break;
            }
            if std::time::Instant::now() > phase1_deadline {
                break;
            }
            // Also drain MQTT events to keep the connection alive
            let drain_deadline = std::time::Instant::now() + Duration::from_millis(500);
            for notification in connection.iter() {
                if std::time::Instant::now() > drain_deadline {
                    break;
                }
                let _ = notification;
            }
        }

        if download_started {
            break;
        }
    }

    if !download_started {
        server_shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = server_thread.join();
        bail!(
            "OTA flash failed: device did not start downloading after {} attempts ({}s timeout each).\n\
             Check that the device is online and can reach {}:{}.",
            MAX_TRIGGER_ATTEMPTS,
            DOWNLOAD_START_TIMEOUT_SECS,
            ota_host,
            ota_port
        );
    }

    // Phase 2: Wait for download to finish, then for device to reboot and come online.
    // Total timeout covers: download completion + flash write + reboot + WiFi connect + MQTT connect.
    println!(
        "Download in progress, waiting up to {}s for device to reboot and come online...",
        REBOOT_TIMEOUT_SECS
    );
    let phase2_deadline = std::time::Instant::now() + Duration::from_secs(REBOOT_TIMEOUT_SECS);

    loop {
        if std::time::Instant::now() > phase2_deadline {
            break;
        }

        // Check download progress
        let bytes = ota_progress
            .bytes_sent
            .load(std::sync::atomic::Ordering::SeqCst);
        if !download_complete && bytes >= ota_progress.total_bytes {
            download_complete = true;
            println!("Firmware download complete! Waiting for device to flash and reboot...");
        }

        // Poll MQTT for device status with a short timeout
        let poll_deadline = std::time::Instant::now() + Duration::from_secs(1);
        for notification in connection.iter() {
            if std::time::Instant::now() > poll_deadline
                || std::time::Instant::now() > phase2_deadline
            {
                break;
            }

            match notification {
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(publish))) => {
                    if publish.topic == avail_topic {
                        let payload = String::from_utf8_lossy(&publish.payload);
                        if payload == "online" && !came_online {
                            came_online = true;
                            println!("\nDevice rebooted and came back online!");
                        }
                    }
                    if came_online && (publish.topic == status_topic || publish.topic == diag_topic)
                    {
                        let payload = String::from_utf8_lossy(&publish.payload).to_string();
                        if publish.topic == status_topic {
                            println!("\nDevice published state after OTA!");
                        } else {
                            println!("\nDevice published diagnostics after OTA!");
                        }
                        if version_payload.is_none() && extract_firmware_version(&payload).is_some()
                        {
                            version_payload = Some(payload);
                        }
                    }
                    if came_online && version_payload.is_some() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    // Don't spam — the xtask MQTT connection may flake during device reboot
                    if !download_complete {
                        eprintln!("MQTT error: {}", e);
                    }
                }
            }
        }

        if came_online && version_payload.is_some() {
            break;
        }
    }

    // Step 6: Verify firmware version
    println!("\n[5/6] Verifying firmware version...");
    if !came_online {
        if !download_complete {
            bail!("OTA flash failed: firmware download did not complete.");
        } else {
            bail!("OTA flash failed: firmware was downloaded but device did not come back online within {}s. It may have failed to flash.", REBOOT_TIMEOUT_SECS);
        }
    }
    match (&expected_version, &version_payload) {
        (Some(expected), Some(payload)) => match extract_firmware_version(payload) {
            Some(reported) => {
                if reported == *expected {
                    println!("Firmware version verified: {} (matches expected)", reported);
                } else {
                    bail!(
                            "OTA flash failed: firmware version mismatch! Expected '{}', got '{}'. OTA was rejected or rolled back.",
                            expected, reported
                        );
                }
            }
            None => {
                println!("Warning: firmware_version field not found in state payload. Cannot fully verify.");
            }
        },
        (Some(expected), None) => {
            println!("Warning: no state payload received (device may not be connected to spa).");
            println!(
                "Expected version: {}. Connect device to spa and verify manually.",
                expected
            );
        }
        (None, _) => {
            println!("Warning: could not determine expected version. Skipping version check.");
        }
    }

    // Step 7: Cleanup
    println!("\n[6/6] Cleaning up...");
    server_shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    match server_thread.join() {
        Ok(Ok(())) => println!("OTA server stopped."),
        Ok(Err(e)) => eprintln!("OTA server error: {}", e),
        Err(_) => eprintln!("OTA server thread panicked."),
    }

    println!(
        "\nOTA flash successful! Device {} is running new firmware.",
        device_id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_firmware_version_present() {
        let json = r#"{"temperature": 38.5, "firmware_version": "0.3.0", "heating": true}"#;
        assert_eq!(extract_firmware_version(json), Some("0.3.0".to_string()));
    }

    #[test]
    fn test_extract_firmware_version_missing() {
        let json = r#"{"temperature": 38.5, "heating": true}"#;
        assert_eq!(extract_firmware_version(json), None);
    }

    #[test]
    fn test_extract_firmware_version_null() {
        let json = r#"{"temperature": 38.5, "firmware_version": null, "heating": true}"#;
        assert_eq!(extract_firmware_version(json), None);
    }

    #[test]
    fn test_extract_firmware_version_invalid_json() {
        assert_eq!(extract_firmware_version("not json"), None);
    }

    #[test]
    fn test_extract_firmware_version_empty_string() {
        let json = r#"{"firmware_version": ""}"#;
        assert_eq!(extract_firmware_version(json), Some("".to_string()));
    }

    #[test]
    fn test_read_firmware_version() {
        // This should successfully parse app/Cargo.toml
        let version = read_firmware_version();
        assert!(version.is_ok(), "Should be able to read app/Cargo.toml");
        let v = version.unwrap();
        assert!(!v.is_empty(), "Version should not be empty");
        // Should be a valid semver-like string
        assert!(v.contains('.'), "Version '{}' should contain dots", v);
    }
}
