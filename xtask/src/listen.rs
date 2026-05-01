use anyhow::{bail, Context};
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut host = String::new();
    let mut port = 0u16;
    let mut topic_filter = "launa/#".to_string();
    let mut verbose = false;

    let mut parser = crate::util::Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--host" => host = parser.value("--host")?.to_string(),
            "--port" => {
                port = parser.value("--port")?.parse()?;
            }
            "--topic" | "-t" => topic_filter = parser.value("--topic")?.to_string(),
            "--verbose" | "-v" => {
                parser.skip();
                verbose = true;
            }
            _ => return Err(parser.unknown_arg()),
        }
    }

    // Fall back to launa.toml config if --host not given
    if host.is_empty() || port == 0 {
        match crate::config::load() {
            Ok(config) => {
                if host.is_empty() {
                    host = config.mqtt.host;
                }
                if port == 0 {
                    port = config.mqtt.port;
                }
            }
            Err(e) => {
                if host.is_empty() || port == 0 {
                    bail!(
                        "Cannot load launa.toml: {}\nUse --host and --port to specify the MQTT broker.",
                        e
                    );
                }
            }
        }
    }

    let client_id = format!("xtask-listen-{}", std::process::id());
    let mut mqttoptions = rumqttc::MqttOptions::new(client_id, &host, port);
    mqttoptions.set_keep_alive(Duration::from_secs(5));

    let (client, mut connection) = rumqttc::Client::new(mqttoptions, 10);
    client
        .subscribe(&topic_filter, rumqttc::QoS::AtLeastOnce)
        .context("Failed to subscribe")?;

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    crate::util::ctrlc_handler(move || {
        r.store(false, Ordering::SeqCst);
    });

    println!(
        "Listening on {}:{} — {} (Ctrl+C to stop)",
        host, port, topic_filter
    );
    println!();

    let mut counts: HashMap<String, u64> = HashMap::new();

    for notification in connection.iter() {
        if !running.load(Ordering::SeqCst) {
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
            let len = payload.len();

            *counts.entry(topic.clone()).or_insert(0) += 1;

            // Extract sub-topic after device ID for color/type hint
            let sub_topic = topic.split('/').nth(2).unwrap_or("");

            if verbose || should_print_full(sub_topic) {
                let text = String::from_utf8_lossy(payload);
                // Truncate very long payloads (e.g. HA discovery configs)
                if text.len() > 300 && !verbose {
                    println!("{}: {}... ({} bytes)", topic, &text[..300], len);
                } else {
                    println!("{}: {}", topic, text);
                }
            } else {
                // Summarize: just show topic and size
                println!("{} ({} bytes)", topic, len);
            }

            // Flush stdout for immediate display
            std::io::stdout().flush().ok();
        }
    }

    // Print summary on exit
    if !counts.is_empty() {
        println!();
        println!("=== Message summary ===");
        let mut total: u64 = 0;
        let mut sorted: Vec<(&String, &u64)> = counts.iter().collect();
        sorted.sort_by_key(|(t, _)| t.as_str());
        for (topic, count) in sorted {
            println!("  {:50} {} msgs", topic, count);
            total += count;
        }
        println!("  {:50} {} msgs", "TOTAL", total);
    }

    Ok(())
}

/// Topics that should always print their full payload.
fn should_print_full(sub_topic: &str) -> bool {
    matches!(
        sub_topic,
        "log" | "diagnostics" | "alert" | "state" | "sniff" | "ota"
    )
}
