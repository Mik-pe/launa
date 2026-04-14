use anyhow::{bail, Context};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut feature = "default".to_string();
    let mut device_id_override = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--feature" => {
                i += 1;
                feature = args[i].clone();
            }
            "--device-id" => {
                i += 1;
                device_id_override = Some(args[i].clone());
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let config = crate::config::load()?;
    let device_id = device_id_override.unwrap_or(config.device.id.clone());
    let ota_port = config.ota.serve_port;

    println!("OTA flash: device={}, feature={}, ota_port={}", device_id, feature, ota_port);

    // Step 1: Run cargo test
    println!("\n[1/6] Running cargo test...");
    let test_status = Command::new("cargo")
        .arg("test")
        .current_dir(project_root())
        .status()
        .context("Failed to run cargo test")?;
    if !test_status.success() {
        bail!("cargo test failed. Aborting OTA flash.");
    }
    println!("Tests passed.");

    // Step 2: Build firmware
    println!("\n[2/6] Building firmware...");
    let bin_path = project_root().join("target").join("launa.bin");
    let app_dir = project_root().join("app");

    let mut build_cmd = Command::new("cargo");
    build_cmd.arg("espflash").arg("save-image").arg("--chip").arg("esp32");
    if feature != "default" {
        build_cmd.arg("--features").arg(&feature);
    }
    build_cmd.arg("-o").arg(&bin_path);
    build_cmd.current_dir(&app_dir);

    let build_status = build_cmd.status().context("Failed to run cargo espflash save-image")?;
    if !build_status.success() {
        bail!("Firmware build failed.");
    }
    println!("Firmware built: {}", bin_path.display());

    // Step 3: Start OTA server in background
    println!("\n[3/6] Starting OTA server...");
    let xtask_bin = std::env::current_exe().context("Failed to get current exe path")?;
    let mut ota_serve_cmd = Command::new(&xtask_bin);
    ota_serve_cmd
        .arg("ota-serve")
        .arg("--firmware").arg(&bin_path)
        .arg("--port").arg(ota_port.to_string());

    let mut ota_serve_child = ota_serve_cmd.spawn()
        .context("Failed to start OTA server")?;
    println!("OTA server started (PID {}).", ota_serve_child.id());

    // Wait for server to be ready
    std::thread::sleep(Duration::from_secs(2));

    // Step 4: Publish OTA command via MQTT
    println!("\n[4/6] Publishing OTA command via MQTT...");
    let firmware_url = format!("http://{}:{}/firmware.bin", config.mqtt.host, ota_port);

    let mut mqttoptions = rumqttc::MqttOptions::new(
        format!("xtask-ota-flash-{}", device_id),
        &config.mqtt.host,
        config.mqtt.port,
    );
    mqttoptions.set_keep_alive(Duration::from_secs(5));

    let (client, mut connection) = rumqttc::Client::new(mqttoptions, 10);

    let payload = serde_json::json!({
        "url": firmware_url,
        "feature": feature,
    });
    let topic = format!("launa/{}/ota", device_id);
    client.publish(&topic, rumqttc::QoS::AtLeastOnce, false, payload.to_string().as_bytes())
        .context("Failed to publish OTA command")?;
    println!("OTA command published to {}", topic);

    // Step 5: Wait for device to come back online
    println!("\n[5/6] Waiting for device to come back online (timeout 120s)...");
    let status_topic = format!("launa/{}/status", device_id);
    client.subscribe(&status_topic, rumqttc::QoS::AtLeastOnce)?;

    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut came_online = false;

    for notification in connection.iter() {
        if std::time::Instant::now() > deadline {
            break;
        }
        match notification {
            Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(publish))) => {
                if publish.topic.contains(&device_id) && publish.topic.contains("status") {
                    came_online = true;
                    println!("Device {} is back online!", device_id);
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("MQTT error: {}", e);
            }
        }
    }

    // Step 6: Cleanup
    println!("\n[6/6] Cleaning up...");
    let _ = ota_serve_child.kill();
    let _ = ota_serve_child.wait();
    println!("OTA server stopped.");

    if came_online {
        println!("\nOTA flash successful! Device {} is running new firmware.", device_id);
        Ok(())
    } else {
        bail!("OTA flash timed out. Device did not come back online within 120s.");
    }
}
