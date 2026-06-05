use std::sync::{Arc, RwLock};

use rumqttd::local::{LinkRx, LinkTx};
use rumqttd::{Broker, Notification};
use tracing::{error, info, warn};

use crate::google_home;
use crate::memory::{MemoryStore, COMPONENT_FIELDS};

pub fn start(
    broker: &Broker,
    mem: Arc<RwLock<MemoryStore>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut link_tx, mut link_rx) = broker.link("data-store")?;
    link_tx.subscribe("launa/#")?;

    // Create a publisher link for Google Home commands
    let (mut pub_tx, _) = broker.link("google-home-pub")?;
    let cmd_rx = google_home::create_mqtt_command_channel();

    info!("MQTT data-store bridge subscribed to launa/#");

    std::thread::Builder::new()
        .name("mqtt-bridge".into())
        .spawn(move || {
            run_bridge(&mut link_tx, &mut link_rx, &mem);
        })?;

    // Spawn a thread to forward Google Home commands to MQTT
    std::thread::Builder::new()
        .name("gh-mqtt-pub".into())
        .spawn(move || {
            while let Ok((topic, payload)) = cmd_rx.recv() {
                if let Err(e) = pub_tx.publish(topic, payload) {
                    warn!("Google Home MQTT publish failed: {e:?}");
                }
            }
            warn!("Google Home MQTT publisher thread exited");
        })?;

    Ok(())
}

fn run_bridge(_link_tx: &mut LinkTx, link_rx: &mut LinkRx, mem: &RwLock<MemoryStore>) {
    loop {
        match link_rx.recv() {
            Ok(Some(Notification::Forward(forward))) => {
                handle_forward(mem, &forward.publish);
            }
            Ok(Some(_)) => {}
            Ok(None) => {}
            Err(e) => {
                error!("MQTT bridge recv error: {e:?}");
                break;
            }
        }
    }
    warn!("MQTT bridge loop exited");
}

fn handle_forward(mem: &RwLock<MemoryStore>, publish: &rumqttd::protocol::Publish) {
    let topic = match std::str::from_utf8(&publish.topic) {
        Ok(t) => t,
        Err(_) => return,
    };
    let payload = match std::str::from_utf8(&publish.payload) {
        Ok(p) => p,
        Err(_) => return,
    };

    // Topic format: launa/<device_id>/<subtopic>
    let mut parts = topic.splitn(3, '/');
    let _ = parts.next(); // "launa"
    let (device_id, subtopic) = match (parts.next(), parts.next()) {
        (Some(d), Some(s)) => (d, s),
        _ => return,
    };

    match subtopic {
        "availability" => handle_availability(mem, device_id, payload),
        "boot" => handle_boot(mem, device_id, payload),
        "state" => handle_state(mem, device_id, payload),
        "log" => handle_log(mem, device_id, payload),
        "diagnostics" => {
            mem.write().unwrap().insert_diagnostics(device_id, payload);
        }
        "alert" => {
            mem.write().unwrap().insert_alert(device_id, payload);
        }
        "sniff" => {
            mem.write().unwrap().insert_sniff_frame(device_id, payload);
        }
        _ => {}
    }
}

/// Parse state JSON outside the lock, then take a brief write lock for insertion.
fn handle_state(mem: &RwLock<MemoryStore>, device_id: &str, payload: &str) {
    let val = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Extract temperature fields outside the lock
    let current_temp = val.get("current_temp").and_then(|v| v.as_f64());
    let set_temp = val.get("set_temp").and_then(|v| v.as_f64());

    // Extract component states outside the lock
    let component_states: Vec<(&str, bool)> = COMPONENT_FIELDS
        .iter()
        .filter_map(|&field| val.get(field).and_then(|v| v.as_bool()).map(|b| (field, b)))
        .collect();

    // Brief write lock for insertion only
    let mut store = mem.write().unwrap();
    store.insert_temperature_sample(device_id, current_temp, set_temp);
    store.insert_component_changes(device_id, &component_states);
}

fn handle_availability(mem: &RwLock<MemoryStore>, device_id: &str, payload: &str) {
    let status = payload.trim();
    if status == "online" {
        info!("Device '{device_id}' connected (availability online)");
    } else {
        info!("Device '{device_id}' availability: {status}");
    }
    let mut store = mem.write().unwrap();
    store.update_device_status(device_id, status, None);
    store.insert_availability(device_id, status);
}

fn handle_boot(mem: &RwLock<MemoryStore>, device_id: &str, payload: &str) {
    let boot_id = payload.trim().parse::<u32>().ok();
    info!("Device '{device_id}' booted (boot_id={})", payload.trim());
    mem.write()
        .unwrap()
        .update_device_status(device_id, "online", boot_id);
}

/// Parse log JSON outside the lock, then take a brief write lock for insertion.
fn handle_log(mem: &RwLock<MemoryStore>, device_id: &str, payload: &str) {
    let (level, message, timestamp_ms) = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(val) => {
            let level = val["level"].as_str().unwrap_or("unknown").to_string();
            let message = val["message"].as_str().unwrap_or(payload).to_string();
            let ts = val["ts"].as_u64().unwrap_or(0);
            (level, message, ts)
        }
        Err(_) => ("unknown".to_string(), payload.to_string(), 0),
    };

    mem.write()
        .unwrap()
        .insert_log(device_id, &level, &message, timestamp_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryStore;
    use rumqttd::protocol::Publish;

    fn make_publish(topic: &str, payload: &str) -> Publish {
        Publish::new(topic.to_string(), payload.to_string(), false)
    }

    fn test_store() -> RwLock<MemoryStore> {
        RwLock::new(MemoryStore::new())
    }

    #[test]
    fn test_handle_forward_state_stores_graph_data() {
        let mem = test_store();
        let publish = make_publish(
            "launa/spa_001/state",
            r#"{"current_temp":100.0,"set_temp":104.0}"#,
        );
        handle_forward(&mem, &publish);
        let since = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let samples = mem
            .read()
            .unwrap()
            .get_temperature_history_since("spa_001", &since);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].current_temp, Some(100.0));
    }

    #[test]
    fn test_handle_forward_log() {
        let mem = test_store();
        let publish = make_publish(
            "launa/spa_001/log",
            r#"{"level":"warn","message":"Temperature high","ts":12345}"#,
        );
        handle_forward(&mem, &publish);
        let logs = mem.read().unwrap().get_logs("spa_001", 10);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, "warn");
        assert_eq!(logs[0].message, "Temperature high");
        assert_eq!(logs[0].timestamp_ms, 12345);
    }

    #[test]
    fn test_handle_forward_diagnostics() {
        let mem = test_store();
        let publish = make_publish("launa/spa_001/diagnostics", r#"{"uptime":1234}"#);
        handle_forward(&mem, &publish);
        let diags = mem.read().unwrap().get_diagnostics("spa_001", 10);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].payload.contains("uptime"));
    }

    #[test]
    fn test_handle_forward_alert() {
        let mem = test_store();
        let publish = make_publish("launa/spa_001/alert", r#"{"msg":"overheat"}"#);
        handle_forward(&mem, &publish);
        let alerts = mem.read().unwrap().get_alerts("spa_001", 10);
        assert_eq!(alerts.len(), 1);
    }

    #[test]
    fn test_handle_forward_sniff() {
        let mem = test_store();
        let publish = make_publish("launa/spa_001/sniff", r#"{"hex":"aabbcc"}"#);
        handle_forward(&mem, &publish);
        let sniffs = mem.read().unwrap().get_sniff_frames("spa_001", 10);
        assert_eq!(sniffs.len(), 1);
    }

    #[test]
    fn test_handle_forward_unknown_subtopic_ignored() {
        let mem = test_store();
        let publish = make_publish("launa/spa_001/command", r#"{"cmd":"set_temp"}"#);
        handle_forward(&mem, &publish);
        assert!(mem.read().unwrap().get_logs("spa_001", 10).is_empty());
    }

    #[test]
    fn test_handle_forward_invalid_topic_ignored() {
        let mem = test_store();
        let publish = make_publish("homeassistant/sensor/spa_001/temp/config", "{}");
        handle_forward(&mem, &publish);
        assert!(mem.read().unwrap().get_logs("spa_001", 10).is_empty());
    }

    #[test]
    fn test_handle_log_malformed_json_stored_as_unknown() {
        let mem = test_store();
        let publish = make_publish("launa/spa_001/log", "not json at all");
        handle_forward(&mem, &publish);
        let logs = mem.read().unwrap().get_logs("spa_001", 10);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, "unknown");
        assert_eq!(logs[0].message, "not json at all");
    }

    #[test]
    fn test_handle_forward_availability() {
        let mem = test_store();
        let publish = make_publish("launa/spa_001/availability", "online");
        handle_forward(&mem, &publish);
        let status = mem.read().unwrap().get_device_status("spa_001").unwrap();
        assert_eq!(status.status, "online");
        let history = mem.read().unwrap().get_availability_history("spa_001", 10);
        assert_eq!(history.len(), 1);
    }
}
