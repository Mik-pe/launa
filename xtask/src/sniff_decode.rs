use anyhow::{bail, Context};
use launa_protocol::FrameDecoder;
use std::time::Duration;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut host = "localhost".to_string();
    let mut port = 1883u16;
    let mut output_file = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => {
                i += 1;
                host = args[i].clone();
            }
            "--port" => {
                i += 1;
                port = args[i].parse().context("Invalid port")?;
            }
            "--output" | "-o" => {
                i += 1;
                output_file = Some(args[i].clone());
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let mut mqttoptions = rumqttc::MqttOptions::new("xtask-sniff-decode", &host, port);
    mqttoptions.set_keep_alive(Duration::from_secs(5));

    let (client, mut connection) = rumqttc::Client::new(mqttoptions, 10);
    client
        .subscribe("launa/+/sniff", rumqttc::QoS::AtLeastOnce)
        .context("Failed to subscribe to sniff topic")?;

    println!(
        "Connected to MQTT broker at {}:{} (Ctrl+C to stop)",
        host, port
    );
    println!("Subscribed to: launa/+/sniff");
    println!();

    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    let _ = ctrlc::set_handler(move || {
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    let mut session_log: Vec<serde_json::Value> = Vec::new();
    let mut decoder = FrameDecoder::new();

    for notification in connection.iter() {
        if !running.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }

        let notification = match notification {
            Ok(n) => n,
            Err(e) => {
                eprintln!("MQTT error: {}", e);
                continue;
            }
        };

        if let rumqttc::Event::Incoming(rumqttc::Packet::Publish(publish)) = notification {
            let topic = &publish.topic;
            let payload = &publish.payload;

            // Extract device ID from topic: launa/<device_id>/sniff
            let device_id = topic.split('/').nth(1).unwrap_or("?");

            // Try to parse as JSON
            let entry = if let Ok(json) = serde_json::from_slice::<serde_json::Value>(payload) {
                handle_json_sniff(&mut decoder, device_id, &json)
            } else {
                // Treat as raw hex string
                let text = String::from_utf8_lossy(payload);
                handle_raw_sniff(&mut decoder, device_id, &text)
            };

            if let Some(ref _file) = output_file {
                if let Ok(obj) = serde_json::to_value(&entry) {
                    session_log.push(obj);
                }
            }
        }
    }

    if let Some(ref file) = output_file {
        let json = serde_json::to_string_pretty(&session_log)?;
        std::fs::write(file, json)?;
        println!("Session log saved to {}", file);
    }

    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct SniffEntry {
    device_id: String,
    timestamp: Option<String>,
    message_type: String,
    crc_ok: bool,
    raw_hex: String,
    parsed: Option<String>,
}

fn handle_json_sniff(
    decoder: &mut FrameDecoder,
    device_id: &str,
    json: &serde_json::Value,
) -> SniffEntry {
    let timestamp = json
        .get("ts")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let raw_hex = json
        .get("raw")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let raw_bytes = hex_to_bytes(&raw_hex);
    let frames = decoder.feed_slice(&raw_bytes);

    if let Some(frame) = frames.first() {
        let msg_type = format!(
            "{:02X} {:02X}",
            frame.message_type[0], frame.message_type[1]
        );
        let crc_ok = true; // FrameDecoder already validated CRC
        let parsed = describe_frame(frame);
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            timestamp,
            message_type: msg_type,
            crc_ok,
            raw_hex: format!("{:02X?}", raw_bytes),
            parsed: Some(parsed),
        };
        print_entry(&entry);
        entry
    } else {
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            timestamp,
            message_type: "unknown".to_string(),
            crc_ok: false,
            raw_hex: raw_hex.clone(),
            parsed: None,
        };
        print_entry(&entry);
        entry
    }
}

fn handle_raw_sniff(decoder: &mut FrameDecoder, device_id: &str, text: &str) -> SniffEntry {
    let raw_bytes = hex_to_bytes(text.trim());
    let frames = decoder.feed_slice(&raw_bytes);

    if let Some(frame) = frames.first() {
        let msg_type = format!(
            "{:02X} {:02X}",
            frame.message_type[0], frame.message_type[1]
        );
        let parsed = describe_frame(frame);
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            timestamp: None,
            message_type: msg_type,
            crc_ok: true,
            raw_hex: format!("{:02X?}", raw_bytes),
            parsed: Some(parsed),
        };
        print_entry(&entry);
        entry
    } else {
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            timestamp: None,
            message_type: "unknown".to_string(),
            crc_ok: false,
            raw_hex: format!("{:02X?}", raw_bytes),
            parsed: None,
        };
        print_entry(&entry);
        entry
    }
}

fn describe_frame(frame: &launa_protocol::Frame) -> String {
    match frame.message_type {
        [0xFF, 0xAF] => {
            // Status update - try to extract key fields
            if frame.payload.len() >= 21 {
                let current_temp = frame.payload[2];
                let set_temp = frame.payload[20];
                let hour = frame.payload[3];
                let minute = frame.payload[4];
                let pump_flags = frame.payload.get(11).copied().unwrap_or(0);
                let pump1 = pump_flags & 0x03;
                let pump2 = (pump_flags >> 2) & 0x03;
                format!(
                    "Status: temp={}F set={}F time={:02}:{:02} pump1={} pump2={}",
                    current_temp, set_temp, hour, minute, pump1, pump2
                )
            } else {
                "Status update (payload too short)".to_string()
            }
        }
        [0x0A, 0xBF] => {
            if frame.payload.is_empty() {
                return "Command (empty payload)".to_string();
            }
            match frame.payload[0] {
                0x04 => "Config request".to_string(),
                0x11 if frame.payload.len() >= 2 => {
                    format!("Toggle item 0x{:02X}", frame.payload[1])
                }
                0x20 if frame.payload.len() >= 2 => format!("Set temperature {}", frame.payload[1]),
                0x22 => "Settings request".to_string(),
                0x23 => "Filter cycles request".to_string(),
                0x24 => "Information request".to_string(),
                0x27 => "Temperature scale".to_string(),
                0x28 => "Fault log request".to_string(),
                0x2E => "Control configuration".to_string(),
                0x94 => "Configuration response".to_string(),
                other => format!("0A BF sub-type 0x{:02X}", other),
            }
        }
        [0xFE, 0xBF] => {
            if frame.payload.is_empty() {
                return "Registration (empty)".to_string();
            }
            match frame.payload[0] {
                0x00 => "Registration query".to_string(),
                0x01 => "Registration request".to_string(),
                0x02 if frame.payload.len() >= 2 => {
                    format!("Client ID assigned: {}", frame.payload[1])
                }
                other => format!("Registration sub-type 0x{:02X}", other),
            }
        }
        [0x10, 0xBF] => "Ready (bus free)".to_string(),
        _ => format!(
            "Unknown type {:02X} {:02X}",
            frame.message_type[0], frame.message_type[1]
        ),
    }
}

fn print_entry(entry: &SniffEntry) {
    let ts = entry.timestamp.as_deref().unwrap_or("?");
    let crc = if entry.crc_ok { "OK" } else { "FAIL" };
    println!(
        "[{}] device={} type={} crc={}",
        ts, entry.device_id, entry.message_type, crc
    );
    if let Some(ref parsed) = entry.parsed {
        println!("  {}", parsed);
    }
    println!("  raw: {}", entry.raw_hex);
    println!();
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.trim_start_matches("0x").trim_start_matches("0X");
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| {
            let byte_str = &hex[i..i.saturating_add(2).min(hex.len())];
            u8::from_str_radix(byte_str, 16).ok()
        })
        .collect()
}
