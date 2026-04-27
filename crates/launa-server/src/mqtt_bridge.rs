use std::sync::Arc;

use rumqttd::local::{LinkRx, LinkTx};
use rumqttd::{Broker, Notification};
use tracing::{error, info, warn};

use crate::db::Database;

pub fn start(broker: &Broker, db: Arc<Database>) -> Result<(), Box<dyn std::error::Error>> {
    let (mut link_tx, mut link_rx) = broker.link("data-store")?;
    link_tx.subscribe("launa/#")?;

    info!("MQTT data-store bridge subscribed to launa/#");

    std::thread::Builder::new()
        .name("mqtt-bridge".into())
        .spawn(move || {
            run_bridge(&mut link_tx, &mut link_rx, &db);
        })?;

    Ok(())
}

fn run_bridge(_link_tx: &mut LinkTx, link_rx: &mut LinkRx, db: &Database) {
    loop {
        match link_rx.recv() {
            Ok(Some(Notification::Forward(forward))) => {
                handle_forward(db, &forward.publish);
            }
            Ok(Some(_notification)) => {
                // Acknowledgments, disconnects, etc. - ignore
            }
            Ok(None) => {
                // Empty notification, continue
            }
            Err(e) => {
                error!("MQTT bridge recv error: {e:?}");
                break;
            }
        }
    }
    warn!("MQTT bridge loop exited");
}

fn handle_forward(db: &Database, publish: &rumqttd::protocol::Publish) {
    let topic = match std::str::from_utf8(&publish.topic) {
        Ok(t) => t,
        Err(_) => return,
    };
    let payload = match std::str::from_utf8(&publish.payload) {
        Ok(p) => p,
        Err(_) => return,
    };

    // Topic format: launa/<device_id>/<subtopic>
    let parts: Vec<&str> = topic.splitn(3, '/').collect();
    if parts.len() < 3 || parts[0] != "launa" {
        return;
    }
    let device_id = parts[1];
    let subtopic = parts[2];

    match subtopic {
        "availability" => handle_availability(db, device_id, payload),
        "boot" => handle_boot(db, device_id, payload),
        "state" => db.insert_status(device_id, payload),
        "log" => handle_log(db, device_id, payload),
        "diagnostics" => db.insert_diagnostics(device_id, payload),
        "alert" => db.insert_alert(device_id, payload),
        "sniff" => db.insert_sniff_frame(device_id, payload),
        _ => {}
    }
}

fn handle_availability(db: &Database, device_id: &str, payload: &str) {
    // Payload is a plain string: "online", "offline", or "stale"
    let status = match payload.trim() {
        "online" => "online",
        "offline" => "offline",
        "stale" => "stale",
        other => other,
    };
    db.update_device_status(device_id, status, None);
}

fn handle_boot(db: &Database, device_id: &str, payload: &str) {
    let boot_id = payload.trim().parse::<u32>().ok();
    if let Some(bid) = boot_id {
        db.update_device_status(device_id, "online", Some(bid));
    }
}

fn handle_log(db: &Database, device_id: &str, payload: &str) {
    // Log payloads are JSON: {"level":"warn","message":"...","ts":12345}
    let val: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => {
            db.insert_log(device_id, "unknown", payload, 0);
            return;
        }
    };

    let level = val["level"].as_str().unwrap_or("unknown");
    let message = val["message"].as_str().unwrap_or(payload);
    let timestamp_ms = val["ts"].as_u64().unwrap_or(0);

    db.insert_log(device_id, level, message, timestamp_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use rumqttd::protocol::Publish;

    fn make_publish(topic: &str, payload: &str) -> Publish {
        Publish::new(topic.to_string(), payload.to_string(), false)
    }

    #[test]
    fn test_handle_forward_state() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("launa/spa_001/state", r#"{"current_temp":100}"#);
        handle_forward(&db, &publish);
        let status = db.get_latest_status("spa_001").unwrap();
        assert!(status.payload.contains("100"));
    }

    #[test]
    fn test_handle_forward_log() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish(
            "launa/spa_001/log",
            r#"{"level":"warn","message":"Temperature high","ts":12345}"#,
        );
        handle_forward(&db, &publish);
        let logs = db.get_logs("spa_001", 10);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, "warn");
        assert_eq!(logs[0].message, "Temperature high");
        assert_eq!(logs[0].timestamp_ms, 12345);
    }

    #[test]
    fn test_handle_forward_diagnostics() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("launa/spa_001/diagnostics", r#"{"uptime":1234}"#);
        handle_forward(&db, &publish);
        let diags = db.get_diagnostics("spa_001", 10);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].payload.contains("uptime"));
    }

    #[test]
    fn test_handle_forward_alert() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("launa/spa_001/alert", r#"{"msg":"overheat"}"#);
        handle_forward(&db, &publish);
        let alerts = db.get_alerts("spa_001", 10);
        assert_eq!(alerts.len(), 1);
    }

    #[test]
    fn test_handle_forward_sniff() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("launa/spa_001/sniff", r#"{"hex":"aabbcc"}"#);
        handle_forward(&db, &publish);
        let sniffs = db.get_sniff_frames("spa_001", 10);
        assert_eq!(sniffs.len(), 1);
    }

    #[test]
    fn test_handle_forward_unknown_subtopic_ignored() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("launa/spa_001/command", r#"{"cmd":"set_temp"}"#);
        handle_forward(&db, &publish);
        assert!(db.get_latest_status("spa_001").is_none());
    }

    #[test]
    fn test_handle_forward_invalid_topic_ignored() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("homeassistant/sensor/spa_001/temp/config", "{}");
        handle_forward(&db, &publish);
        assert!(db.get_latest_status("spa_001").is_none());
    }

    #[test]
    fn test_handle_log_malformed_json_stored_as_unknown() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("launa/spa_001/log", "not json at all");
        handle_forward(&db, &publish);
        let logs = db.get_logs("spa_001", 10);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, "unknown");
        assert_eq!(logs[0].message, "not json at all");
    }

    #[test]
    fn test_handle_forward_availability_ignored() {
        let db = Database::open_in_memory().unwrap();
        let publish = make_publish("launa/spa_001/availability", "online");
        handle_forward(&db, &publish);
        assert!(db.get_latest_status("spa_001").is_none());
    }
}
