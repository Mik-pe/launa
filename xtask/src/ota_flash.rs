use anyhow::{bail, Context};
use serde::Deserialize;
use std::io::{Read as _, Write as _};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Offset of the factory/app partition in the merged flash image (from partitions.csv).
const APP_PARTITION_OFFSET: usize = 0x20000;

/// Read the firmware version from app/Cargo.toml.
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
    Ok(cargo.package.version)
}

/// Extract the firmware_version field from a state JSON payload.
/// Returns None if the field is missing or the payload is not valid JSON.
pub fn extract_firmware_version(state_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(state_json).ok()?;
    value
        .get("firmware_version")?
        .as_str()
        .map(|s| s.to_string())
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut feature = "default".to_string();
    let mut device_id_override = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--feature" => {
                i += 1;
                if i >= args.len() {
                    bail!("--feature requires a value");
                }
                feature = args[i].clone();
            }
            "--device-id" => {
                i += 1;
                if i >= args.len() {
                    bail!("--device-id requires a value");
                }
                device_id_override = Some(args[i].clone());
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let config = crate::config::load()?;
    let device_id = device_id_override.unwrap_or(config.device.id.clone());
    let ota_port = config.ota.serve_port;

    // Read expected firmware version
    let expected_version = read_firmware_version().ok();
    if let Some(ref v) = expected_version {
        println!("Expected firmware version: {}", v);
    } else {
        println!("Warning: Could not read firmware version from app/Cargo.toml");
    }

    println!(
        "OTA flash: device={}, feature={}, ota_port={}",
        device_id, feature, ota_port
    );

    // Step 1: Run cargo test
    println!("\n[1/7] Running cargo test...");
    let test_status = Command::new("cargo")
        .arg("test")
        .current_dir(crate::util::project_root())
        .status()
        .context("Failed to run cargo test")?;
    if !test_status.success() {
        bail!("cargo test failed. Aborting OTA flash.");
    }
    println!("Tests passed.");

    // Step 2: Build firmware
    println!("\n[2/7] Building firmware...");
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

    // Step 3: Start OTA server in background
    println!("\n[3/7] Starting OTA server...");
    let xtask_bin = std::env::current_exe().context("Failed to get current exe path")?;
    let mut ota_serve_cmd = Command::new(&xtask_bin);
    ota_serve_cmd
        .arg("ota-serve")
        .arg("--firmware")
        .arg(&ota_bin_path)
        .arg("--port")
        .arg(ota_port.to_string());

    let mut ota_serve_child = ota_serve_cmd
        .spawn()
        .context("Failed to start OTA server")?;
    println!("OTA server started (PID {}).", ota_serve_child.id());

    // Wait for server to be ready (verify with TCP connect)
    let server_addr = format!("127.0.0.1:{}", ota_port);
    for attempt in 1..=10 {
        std::thread::sleep(Duration::from_millis(500));
        if std::net::TcpStream::connect(&server_addr).is_ok() {
            println!("OTA server ready on port {}", ota_port);
            break;
        }
        if attempt == 10 {
            let _ = ota_serve_child.kill();
            let _ = ota_serve_child.wait();
            bail!("OTA server did not become ready on port {}", ota_port);
        }
        println!("Waiting for OTA server... (attempt {}/10)", attempt);
    }

    // Step 4: Publish OTA command via MQTT
    println!("\n[4/7] Publishing OTA command via MQTT...");
    let ota_host = if config.ota.host.is_empty() {
        &config.mqtt.host
    } else {
        &config.ota.host
    };
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
    client.subscribe(&status_topic, rumqttc::QoS::AtLeastOnce)?;
    client.subscribe(&avail_topic, rumqttc::QoS::AtLeastOnce)?;

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

    // Now publish the OTA command — any subsequent availability "online"
    // will be from the *new* boot after OTA completes.
    let payload = serde_json::json!({
        "url": firmware_url,
    });
    let topic = format!("launa/{}/ota", device_id);
    client
        .publish(
            &topic,
            rumqttc::QoS::AtLeastOnce,
            false,
            payload.to_string().as_bytes(),
        )
        .context("Failed to publish OTA command")?;
    println!("OTA command published to {}", topic);

    // Step 5: Wait for device to come back online (with serial monitor)
    println!("\n[5/7] Waiting for device to come back online (timeout 120s)...");

    // Start serial monitor thread to show device output during OTA
    let monitor_running = Arc::new(AtomicBool::new(true));
    let monitor_port = config.device.serial_port.clone();
    let ota_partition_info: Arc<std::sync::Mutex<String>> = Arc::new(std::sync::Mutex::new(String::new()));
    let ota_partition_info_clone = ota_partition_info.clone();
    let monitor_handle = {
        let running = monitor_running.clone();
        std::thread::spawn(move || {
            let Ok(mut port) = serialport::new(&monitor_port, 115200)
                .timeout(Duration::from_millis(100))
                .open()
            else {
                return;
            };
            let mut buf = [0u8; 256];
            let mut line_buf = String::new();
            while running.load(Ordering::SeqCst) {
                match port.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        print!("{}", text);
                        // Capture OTA partition info from serial log
                        line_buf.push_str(&text);
                        for pattern in &["OTA: beginning update to", "Loaded app from partition at offset"] {
                            if let Some(pos) = line_buf.find(pattern) {
                                let line_end = line_buf[pos..].find('\n').unwrap_or(line_buf[pos..].len());
                                let line = line_buf[pos..pos + line_end].trim().to_string();
                                if let Ok(mut info) = ota_partition_info_clone.lock() {
                                    *info = line;
                                }
                            }
                        }
                        // Keep line_buf bounded
                        if line_buf.len() > 4096 {
                            line_buf = line_buf[line_buf.len() - 2048..].to_string();
                        }
                    }
                    Ok(_) => {}
                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => break,
                }
            }
        })
    };
    println!("Serial monitor active on {}", config.device.serial_port);

    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut came_online = false;
    let mut state_payload: Option<String> = None;

    for notification in connection.iter() {
        if std::time::Instant::now() > deadline {
            break;
        }
        match notification {
            Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(publish))) => {
                if publish.topic == avail_topic {
                    let payload = String::from_utf8_lossy(&publish.payload);
                    if payload == "online" {
                        came_online = true;
                        println!("\nDevice {} is back online!", device_id);
                    }
                }
                if publish.topic == status_topic {
                    came_online = true;
                    state_payload = Some(String::from_utf8_lossy(&publish.payload).to_string());
                    println!("\nDevice {} published state!", device_id);
                }
                if came_online {
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("MQTT error: {}", e);
            }
        }
    }

    monitor_running.store(false, Ordering::SeqCst);
    let _ = monitor_handle.join();

    // Report OTA partition info captured from serial output
    if let Ok(info) = ota_partition_info.lock() {
        if !info.is_empty() {
            println!("\nOTA target: {}", *info);
        }
    }

    // Step 6: Verify firmware version
    println!("\n[6/7] Verifying firmware version...");
    if came_online {
        if let Some(ref expected) = expected_version {
            if let Some(ref payload) = state_payload {
                match extract_firmware_version(payload) {
                    Some(reported) => {
                        if reported == *expected {
                            println!("Firmware version verified: {} (matches expected)", reported);
                        } else {
                            bail!(
                                "Firmware version mismatch! Expected '{}', got '{}'. Possible rollback occurred.",
                                expected, reported
                            );
                        }
                    }
                    None => {
                        println!(
                            "Warning: firmware_version field not found in state payload. Cannot verify version."
                        );
                    }
                }
            } else {
                println!("Warning: no state payload captured. Cannot verify version.");
            }
        } else {
            println!("Warning: could not determine expected version. Skipping version check.");
        }
    }

    // Step 7: Cleanup
    println!("\n[7/7] Cleaning up...");
    let _ = ota_serve_child.kill();
    let _ = ota_serve_child.wait();
    println!("OTA server stopped.");

    if came_online {
        println!(
            "\nOTA flash successful! Device {} is running new firmware.",
            device_id
        );
        Ok(())
    } else {
        bail!("OTA flash timed out. Device did not come back online within 120s.");
    }
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
