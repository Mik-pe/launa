use anyhow::Context;
use launa_protocol::{dispatch_frame, Frame, FrameDecoder, IncomingMessage};
use std::time::Duration;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut host = String::new();
    let mut port = 0u16;
    let mut output_file = None;
    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--host" => host = parser.value("--host")?.to_string(),
            "--port" => {
                port = parser.value("--port")?.parse()?;
            }
            "--output" | "-o" => output_file = Some(parser.value("--output")?.to_string()),
            _ => return Err(parser.unknown_arg()),
        }
    }

    // Fall back to launa.toml config if --host not given
    crate::util::resolve_mqtt_config(
        &mut host,
        &mut port,
        crate::config::load_without_serial_port_check,
    )?;

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
    crate::util::ctrlc_handler(move || {
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
            let entries = if let Ok(json) = serde_json::from_slice::<serde_json::Value>(payload) {
                handle_json_sniff(&mut decoder, device_id, &json)
            } else {
                // Treat as raw hex string
                let text = String::from_utf8_lossy(payload);
                vec![handle_raw_sniff(&mut decoder, device_id, &text)]
            };

            if let Some(ref _file) = output_file {
                for entry in &entries {
                    if let Ok(obj) = serde_json::to_value(entry) {
                        session_log.push(obj);
                    }
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
    direction: Option<String>,
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
) -> Vec<SniffEntry> {
    // New raw chunks format: {"capture_us":..., "chunks":[["R",ts_us,"HEX"], ...]}
    if let Some(chunks_arr) = json.get("chunks").and_then(|v| v.as_array()) {
        return handle_chunks_format(decoder, device_id, json, chunks_arr);
    }

    // Legacy burst capture with entries (decoded frames + garbage)
    if let Some(entries_arr) = json.get("entries").and_then(|v| v.as_array()) {
        return handle_entries_format(device_id, json, entries_arr);
    }

    // Old burst capture format: {"capture_us":..., "frame_count":..., "frames":[[ts_us, type_hex, payload_hex], ...]}
    if let Some(frames_arr) = json.get("frames").and_then(|v| v.as_array()) {
        return handle_frames_format(device_id, json, frames_arr);
    }

    // Legacy per-frame format: {"raw":"...", "type":"...", ...}
    handle_legacy_format(decoder, device_id, json)
}

/// Handle the new raw chunks format by decoding on the host side.
fn handle_chunks_format(
    decoder: &mut FrameDecoder,
    device_id: &str,
    json: &serde_json::Value,
    chunks_arr: &[serde_json::Value],
) -> Vec<SniffEntry> {
    let capture_us = json.get("capture_us").and_then(|v| v.as_u64()).unwrap_or(0);
    let total_bytes: usize = chunks_arr
        .iter()
        .filter_map(|c| c.as_array()?.get(2)?.as_str())
        .map(|h| h.len() / 2)
        .sum();
    let rx_count = chunks_arr
        .iter()
        .filter(|c| {
            c.as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map_or(false, |s| s == "R")
        })
        .count();
    let tx_count = chunks_arr.len() - rx_count;

    println!(
        "=== Raw chunks capture: {} chunks ({} RX + {} TX, {} bytes) in {}us ({:.1}ms) ===",
        chunks_arr.len(),
        rx_count,
        tx_count,
        total_bytes,
        capture_us,
        capture_us as f64 / 1000.0,
    );
    println!();

    // Build a timeline of raw bytes: concatenate all chunks with direction/timestamp info
    let mut chunks: Vec<ChunkInfo> = Vec::new();
    for chunk_val in chunks_arr {
        let arr = match chunk_val.as_array() {
            Some(a) if a.len() >= 3 => a,
            _ => continue,
        };
        let dir = arr[0].as_str().unwrap_or("R").to_string();
        let ts_us = arr[1].as_u64().unwrap_or(0);
        let hex = arr[2].as_str().unwrap_or("");
        let data = hex_to_bytes(hex);
        if !data.is_empty() {
            chunks.push(ChunkInfo { ts_us, dir, data });
        }
    }

    // Concatenate all bytes and run through FrameDecoder
    let mut all_bytes: Vec<u8> = Vec::with_capacity(total_bytes);
    for c in &chunks {
        all_bytes.extend_from_slice(&c.data);
    }

    let prev_errors = decoder.frame_error_count();
    let _frames = decoder.feed_slice(&all_bytes);
    let new_errors = decoder.frame_error_count();
    let error_count = new_errors - prev_errors;

    // Decode frames tracking byte offsets for garbage detection
    let mut offset_decoder = OffsetFrameDecoder::new();
    let mut decoded_frames: Vec<(usize, usize, Frame)> = Vec::new(); // (start, end, frame)
    for (i, &byte) in all_bytes.iter().enumerate() {
        if let Some((start, frame)) = offset_decoder.feed(byte, i) {
            decoded_frames.push((start, i + 1, frame));
        }
    }

    let _garbage_count = if decoded_frames.is_empty() && !all_bytes.is_empty() {
        1
    } else {
        // Count gaps between frames as potential garbage
        let mut gaps = 0;
        let mut pos = 0;
        for (start, end, _) in &decoded_frames {
            if *start > pos {
                gaps += 1;
            }
            pos = *end;
        }
        if pos < all_bytes.len() {
            gaps += 1;
        }
        gaps
    };

    // Map decoded frames back to timestamps and directions
    // Find the chunk that contains each frame's start byte
    let mut chunk_offsets: Vec<(usize, usize, usize, &ChunkInfo)> = Vec::new(); // (abs_start, abs_end, chunk_idx, chunk_ref)
    let mut abs_offset = 0usize;
    for (i, c) in chunks.iter().enumerate() {
        let len = c.data.len();
        chunk_offsets.push((abs_offset, abs_offset + len, i, c));
        abs_offset += len;
    }

    let mut result = Vec::new();

    // Emit garbage entries for bytes not covered by any frame
    let mut covered = vec![false; all_bytes.len()];
    for (start, end, _) in &decoded_frames {
        for i in *start..*end {
            if i < covered.len() {
                covered[i] = true;
            }
        }
    }

    // Collect garbage ranges
    let mut garbage_ranges: Vec<(usize, usize)> = Vec::new();
    let mut garbage_start: Option<usize> = None;
    for (i, &is_covered) in covered.iter().enumerate() {
        if !is_covered {
            if garbage_start.is_none() {
                garbage_start = Some(i);
            }
        } else {
            if let Some(gs) = garbage_start.take() {
                garbage_ranges.push((gs, i));
            }
        }
    }
    if let Some(gs) = garbage_start.take() {
        garbage_ranges.push((gs, all_bytes.len()));
    }

    // Emit garbage entries
    for (start, end) in &garbage_ranges {
        let garbage_bytes = &all_bytes[*start..*end];
        if garbage_bytes.is_empty() {
            continue;
        }
        // Find the chunk containing the start byte
        let chunk_info = find_chunk_for_offset(&chunk_offsets, *start);
        let ts_us = chunk_info.map_or(0, |(_, c)| c.ts_us);
        let dir = chunk_info.map_or("?", |(_, c)| c.dir.as_str());

        let entry = SniffEntry {
            device_id: device_id.to_string(),
            direction: Some(dir.to_string()),
            timestamp: Some(format!("+{}us", ts_us)),
            message_type: "GARBAGE".to_string(),
            crc_ok: false,
            raw_hex: bytes_to_hex(garbage_bytes),
            parsed: Some(format!(
                "{} bytes of inter-frame garbage",
                garbage_bytes.len()
            )),
        };
        print_entry(&entry);
        result.push(entry);
    }

    // Emit decoded frame entries
    for (start, end, frame) in &decoded_frames {
        let chunk_info = find_chunk_for_offset(&chunk_offsets, *start);
        let ts_us = chunk_info.map_or(0, |(_, c)| c.ts_us);
        let dir = chunk_info.map_or("?", |(_, c)| c.dir.as_str());

        let msg = dispatch_frame(frame);
        let parsed = describe_message(&msg);
        let msg_type = format!(
            "{:02X} {:02X}",
            frame.message_type[0], frame.message_type[1]
        );
        let raw_hex = bytes_to_hex(&all_bytes[*start..*end]);

        let entry = SniffEntry {
            device_id: device_id.to_string(),
            direction: Some(dir.to_string()),
            timestamp: Some(format!("+{}us", ts_us)),
            message_type: msg_type,
            crc_ok: true,
            raw_hex,
            parsed: Some(parsed),
        };
        print_entry(&entry);
        result.push(entry);
    }

    if error_count > 0 {
        println!("  ({} frame decode errors)", error_count);
    }

    // Sort by timestamp for consistent output
    result.sort_by_key(|e| parse_ts_us(&e.timestamp));

    print_timing_summary(&result);

    result
}

/// Find the chunk that contains a given absolute byte offset.
fn find_chunk_for_offset<'a>(
    chunk_offsets: &[(usize, usize, usize, &'a ChunkInfo)],
    offset: usize,
) -> Option<(usize, &'a ChunkInfo)> {
    for &(start, end, idx, ref c) in chunk_offsets {
        if offset >= start && offset < end {
            return Some((idx, c));
        }
    }
    // Fallback: return the chunk with the highest start <= offset
    chunk_offsets
        .iter()
        .filter(|(start, _, _, _)| *start <= offset)
        .last()
        .map(|(_start, _, idx, c)| (*idx, *c))
}

/// Legacy entries format (decoded frames + garbage from old sniffer)
fn handle_entries_format(
    device_id: &str,
    json: &serde_json::Value,
    entries_arr: &[serde_json::Value],
) -> Vec<SniffEntry> {
    let capture_us = json.get("capture_us").and_then(|v| v.as_u64()).unwrap_or(0);
    let frame_count = json
        .get("frame_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let garbage_count = entries_arr
        .iter()
        .filter(|e| {
            e.as_array()
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str())
                .map_or(false, |s| s == "garbage")
        })
        .count();

    println!(
        "=== Burst capture: {} frames + {} garbage in {}us ({:.1}ms) ===",
        frame_count,
        garbage_count,
        capture_us,
        capture_us as f64 / 1000.0,
    );
    println!();

    let mut result = Vec::new();
    for entry_val in entries_arr {
        let arr = match entry_val.as_array() {
            Some(a) if a.len() >= 3 => a,
            _ => continue,
        };
        let ts_us = arr[0].as_u64().unwrap_or(0);
        let type_str = arr[1].as_str().unwrap_or("");
        let payload_hex = arr[2].as_str().unwrap_or("");

        if type_str == "garbage" {
            let payload_bytes = hex_to_bytes(payload_hex);
            let entry = SniffEntry {
                device_id: device_id.to_string(),
                direction: None,
                timestamp: Some(format!("+{}us", ts_us)),
                message_type: "GARBAGE".to_string(),
                crc_ok: false,
                raw_hex: payload_hex.to_string(),
                parsed: Some(format!(
                    "{} bytes of inter-frame garbage",
                    payload_bytes.len()
                )),
            };
            print_entry(&entry);
            result.push(entry);
            continue;
        }

        // Regular frame entry
        let entry = decode_burst_entry(device_id, ts_us, type_str, payload_hex);
        result.push(entry);
    }

    print_timing_summary(&result);
    result
}

/// Old frames format: {"frames":[[ts_us, type_hex, payload_hex], ...]}
fn handle_frames_format(
    device_id: &str,
    json: &serde_json::Value,
    frames_arr: &[serde_json::Value],
) -> Vec<SniffEntry> {
    let capture_us = json.get("capture_us").and_then(|v| v.as_u64()).unwrap_or(0);
    let frame_count = json
        .get("frame_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    println!(
        "=== Burst capture: {} frames in {}us ({:.1}ms) ===",
        frame_count,
        capture_us,
        capture_us as f64 / 1000.0,
    );
    println!();

    let mut entries = Vec::new();
    for frame_val in frames_arr {
        let arr = match frame_val.as_array() {
            Some(a) if a.len() >= 3 => a,
            _ => continue,
        };
        let ts_us = arr[0].as_u64().unwrap_or(0);
        let type_hex = arr[1].as_str().unwrap_or("");
        let payload_hex = arr[2].as_str().unwrap_or("");

        let entry = decode_burst_entry(device_id, ts_us, type_hex, payload_hex);
        entries.push(entry);
    }

    print_timing_summary(&entries);
    entries
}

/// Legacy per-frame format: {"raw":"...", "type":"...", ...}
fn handle_legacy_format(
    decoder: &mut FrameDecoder,
    device_id: &str,
    json: &serde_json::Value,
) -> Vec<SniffEntry> {
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
        let msg = dispatch_frame(frame);
        let parsed = describe_message(&msg);
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            direction: None,
            timestamp,
            message_type: msg_type,
            crc_ok: true,
            raw_hex: bytes_to_hex(&raw_bytes),
            parsed: Some(parsed),
        };
        print_entry(&entry);
        vec![entry]
    } else {
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            direction: None,
            timestamp,
            message_type: "unknown".to_string(),
            crc_ok: false,
            raw_hex: raw_hex.clone(),
            parsed: None,
        };
        print_entry(&entry);
        vec![entry]
    }
}

/// Parse timestamp from format "+Nus" used in burst capture entries.
fn parse_ts_us(ts: &Option<String>) -> Option<u64> {
    ts.as_ref().and_then(|s| {
        s.strip_prefix('+')
            .and_then(|s| s.strip_suffix("us"))
            .and_then(|s| s.parse().ok())
    })
}

/// Raw byte chunk with direction and timestamp.
struct ChunkInfo {
    ts_us: u64,
    dir: String,
    data: Vec<u8>,
}

fn handle_raw_sniff(decoder: &mut FrameDecoder, device_id: &str, text: &str) -> SniffEntry {
    let raw_bytes = hex_to_bytes(text.trim());
    let frames = decoder.feed_slice(&raw_bytes);

    if let Some(frame) = frames.first() {
        let msg_type = format!(
            "{:02X} {:02X}",
            frame.message_type[0], frame.message_type[1]
        );
        let msg = dispatch_frame(frame);
        let parsed = describe_message(&msg);
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            direction: None,
            timestamp: None,
            message_type: msg_type,
            crc_ok: true,
            raw_hex: bytes_to_hex(&raw_bytes),
            parsed: Some(parsed),
        };
        print_entry(&entry);
        entry
    } else {
        let entry = SniffEntry {
            device_id: device_id.to_string(),
            direction: None,
            timestamp: None,
            message_type: "unknown".to_string(),
            crc_ok: false,
            raw_hex: bytes_to_hex(&raw_bytes),
            parsed: None,
        };
        print_entry(&entry);
        entry
    }
}

/// Produce a human-readable description using the protocol's typed message.
fn describe_message(msg: &IncomingMessage) -> String {
    match msg {
        IncomingMessage::StatusUpdate(s) => {
            let temp_str = match s.current_temp {
                Some(t) => format!("{:.0}", t),
                None => "--".to_string(),
            };
            let scale = match s.temperature_scale {
                launa_protocol::status::TemperatureScale::Fahrenheit => "F",
                launa_protocol::status::TemperatureScale::Celsius => "C",
                _ => "?",
            };
            format!(
                "Status: temp={}{} set={:.0}{} time={:02}:{:02} heating={} pump1={:?} pump2={:?}",
                temp_str,
                scale,
                s.set_temp,
                scale,
                s.hour,
                s.minute,
                s.is_heating,
                s.pumps[0],
                s.pumps[1],
            )
        }

        IncomingMessage::Ready { channel } => {
            format!("Ready (bus free, ch=0x{:02X})", channel)
        }

        IncomingMessage::Registration(
            launa_protocol::registration::RegistrationMessage::NewClientQuery,
        ) => "Registration query".to_string(),

        IncomingMessage::Registration(
            launa_protocol::registration::RegistrationMessage::ClientIdAssignment {
                channel, ..
            },
        ) => format!("Client ID assigned: {}", channel),

        IncomingMessage::Registration(
            launa_protocol::registration::RegistrationMessage::NewClientResponse { .. },
        ) => "Registration ID request".to_string(),

        IncomingMessage::Registration(
            launa_protocol::registration::RegistrationMessage::ClientIdAck { channel },
        ) => format!("Client ID ack: {}", channel),

        IncomingMessage::Registration(
            launa_protocol::registration::RegistrationMessage::ExistingClientRequest { .. },
        ) => "Existing client request".to_string(),

        IncomingMessage::Registration(
            launa_protocol::registration::RegistrationMessage::ExistingClientResponse {
                channel,
                ..
            },
        ) => format!("Existing client response: channel={}", channel),

        IncomingMessage::Registration(
            launa_protocol::registration::RegistrationMessage::ClearToSend { channel },
        ) => format!("Clear to send: channel={}", channel),

        IncomingMessage::ConfigurationResponse(config)
        | IncomingMessage::ControlConfiguration(config) => {
            let label = match msg {
                IncomingMessage::ConfigurationResponse(_) => "Configuration response",
                IncomingMessage::ControlConfiguration(_) => "Control configuration",
                _ => unreachable!(),
            };
            let pump_desc: Vec<String> = config
                .pump_configs
                .iter()
                .enumerate()
                .filter(|(_, &p)| p != launa_protocol::config::PumpConfig::None)
                .map(|(i, p)| format!("pump{}={:?}", i + 1, p))
                .collect();
            let scale = if config.temperature_scale_celsius {
                "C"
            } else {
                "F"
            };
            if pump_desc.is_empty() {
                format!("{}: scale={}", label, scale)
            } else {
                format!("{}: {} scale={}", label, pump_desc.join(" "), scale)
            }
        }

        IncomingMessage::InformationResponse(info) => {
            format!(
                "Information: model={} sw={} setup={:#04X}",
                info.system_model, info.software_id, info.current_setup,
            )
        }

        IncomingMessage::FaultLogResponse(entry) => {
            format!(
                "Fault #{}: {:?} ({} days ago, {:02}:{:02}, set={}{})",
                entry.entry_number,
                entry.message_code,
                entry.days_ago,
                entry.hour,
                entry.minute,
                entry.set_temperature,
                if entry.flags & 0x01 != 0 { "C" } else { "F" },
            )
        }

        IncomingMessage::FilterCyclesResponse(fc) => {
            let f1 = &fc.filter1;
            let f2 = &fc.filter2;
            let f2_status = if f2.enabled { "enabled" } else { "disabled" };
            format!(
                "Filter cycles: #1 {:02}:{:02}+{}h{:02}m | #2 {:02}:{:02}+{}h{:02}m ({})",
                f1.start_hour,
                f1.start_minute,
                f1.duration_hours,
                f1.duration_minutes,
                f2.start_hour,
                f2.start_minute,
                f2.duration_hours,
                f2.duration_minutes,
                f2_status,
            )
        }

        IncomingMessage::PreferencesResponse { payload } => {
            format!("Preferences ({} bytes): {:02X?}", payload.len(), payload)
        }

        IncomingMessage::SetupParametersResponse { payload } => {
            format!(
                "Setup parameters ({} bytes): {:02X?}",
                payload.len(),
                payload
            )
        }

        IncomingMessage::Unknown {
            message_type,
            payload,
        } => {
            if payload.is_empty() {
                format!(
                    "Unknown type {:02X} {:02X}",
                    message_type[0], message_type[1]
                )
            } else {
                format!(
                    "Unknown type {:02X} {:02X} sub={:02X}",
                    message_type[0], message_type[1], payload[0],
                )
            }
        }

        // Catch-all for future variants added to the non-exhaustive enum
        _ => format!("{:?}", msg),
    }
}

fn print_entry(entry: &SniffEntry) {
    let ts = entry.timestamp.as_deref().unwrap_or("?");
    let dir = entry.direction.as_deref().unwrap_or("?");
    let crc = if entry.crc_ok { "OK" } else { "FAIL" };
    println!(
        "[{}] {} device={} type={} crc={}",
        ts, dir, entry.device_id, entry.message_type, crc
    );
    if let Some(ref parsed) = entry.parsed {
        println!("  {}", parsed);
    }
    println!("  raw: {}", entry.raw_hex);
    println!();
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    let hex: std::borrow::Cow<str> = if !hex.len().is_multiple_of(2) {
        format!("0{}", hex).into()
    } else {
        hex.into()
    };
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect()
}

fn decode_burst_entry(
    device_id: &str,
    ts_us: u64,
    type_str: &str,
    payload_hex: &str,
) -> SniffEntry {
    let type_bytes = hex_to_bytes(type_str);
    let payload_bytes = hex_to_bytes(payload_hex);

    let msg_type = if type_bytes.len() == 2 {
        format!("{:02X} {:02X}", type_bytes[0], type_bytes[1])
    } else {
        type_str.to_string()
    };

    let frame = Frame {
        message_type: if type_bytes.len() == 2 {
            [type_bytes[0], type_bytes[1]]
        } else {
            [0x00, 0x00]
        },
        payload: payload_bytes.clone(),
    };
    let msg = dispatch_frame(&frame);
    let parsed = describe_message(&msg);

    let raw_frame = frame.encode().unwrap_or_else(|_| payload_bytes);
    let raw_hex = bytes_to_hex(&raw_frame);

    let entry = SniffEntry {
        device_id: device_id.to_string(),
        direction: None,
        timestamp: Some(format!("+{}us", ts_us)),
        message_type: msg_type,
        crc_ok: true,
        raw_hex,
        parsed: Some(parsed),
    };
    print_entry(&entry);
    entry
}

fn print_timing_summary(entries: &[SniffEntry]) {
    if entries.len() < 2 {
        return;
    }
    println!("--- Timing summary ---");
    for i in 1..entries.len() {
        let prev_ts = parse_ts_us(&entries[i - 1].timestamp);
        let cur_ts = parse_ts_us(&entries[i].timestamp);
        if let (Some(prev), Some(cur)) = (prev_ts, cur_ts) {
            let delta = cur.saturating_sub(prev);
            let prev_dir = entries[i - 1].direction.as_deref().unwrap_or("?");
            let cur_dir = entries[i].direction.as_deref().unwrap_or("?");
            println!(
                "  {} {} -> {} {} : {}us ({:.1}ms)",
                prev_dir,
                entries[i - 1].message_type,
                cur_dir,
                entries[i].message_type,
                delta,
                delta as f64 / 1000.0,
            );
        }
    }
    println!();
}

/// A FrameDecoder wrapper that tracks the byte offset where each frame starts.
/// Uses the 0x7E delimiter to track frame boundaries.
struct OffsetFrameDecoder {
    decoder: FrameDecoder,
    in_frame: bool,
    frame_start: Option<usize>,
}

impl OffsetFrameDecoder {
    fn new() -> Self {
        OffsetFrameDecoder {
            decoder: FrameDecoder::new(),
            in_frame: false,
            frame_start: None,
        }
    }

    /// Feed a byte at the given absolute offset.
    /// Returns Some((start_offset, frame)) when a complete frame is decoded.
    fn feed(&mut self, byte: u8, offset: usize) -> Option<(usize, Frame)> {
        // Track 0x7E boundaries to know where frames start
        if byte == 0x7E {
            if !self.in_frame {
                // Start of frame
                self.in_frame = true;
                self.frame_start = Some(offset);
            }
            // End of frame is handled by the decoder
        }

        let result = self.decoder.feed(byte);
        if result.is_some() {
            // Frame complete — the start was at the opening 0x7E
            self.in_frame = false;
            let start = self.frame_start.take().unwrap_or(offset);
            Some((start, result.unwrap()))
        } else {
            None
        }
    }
}
