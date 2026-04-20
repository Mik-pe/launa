use std::sync::{Arc, Mutex};

use axum_test::TestServer;
use serde_json::json;

use launa_server::db::Database;
use launa_server::web::{build_router, AppState, AccessoryConfig};

fn test_server_with_db() -> (TestServer, Arc<Database>) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let state = AppState {
        accessory_config: Arc::new(Mutex::new(AccessoryConfig::default())),
        db: db.clone(),
    };
    let server = TestServer::new(build_router(state));
    (server, db)
}

fn test_server_empty() -> TestServer {
    test_server_with_db().0
}

#[tokio::test]
async fn test_get_config_defaults() {
    let server = test_server_empty();

    let resp = server.get("/api/config").await;
    resp.assert_status_ok();
    resp.assert_json(&AccessoryConfig::default());
}

#[tokio::test]
async fn test_set_config() {
    let server = test_server_empty();

    let new_cfg = json!({
        "pumps": 4,
        "lights": 2,
        "blower": false,
        "mister": true,
    });

    let resp = server.put("/api/config").json(&new_cfg).await;
    resp.assert_status_ok();
    resp.assert_json(&json!({
        "pumps": 4,
        "lights": 2,
        "blower": false,
        "mister": true,
    }));

    // Verify persisted via GET
    let get_resp = server.get("/api/config").await;
    get_resp.assert_json(&json!({
        "pumps": 4,
        "lights": 2,
        "blower": false,
        "mister": true,
    }));
}

#[tokio::test]
async fn test_set_config_clamps_values() {
    let server = test_server_empty();

    let resp = server
        .put("/api/config")
        .json(&json!({
            "pumps": 99,
            "lights": 0,
            "blower": true,
            "mister": false,
        }))
        .await;
    resp.assert_status_ok();
    resp.assert_json(&json!({
        "pumps": 6,
        "lights": 1,
        "blower": true,
        "mister": false,
    }));
}

#[tokio::test]
async fn test_device_logs_empty() {
    let server = test_server_empty();

    let resp = server.get("/api/devices/launa_spa/logs").await;
    resp.assert_status_ok();
    resp.assert_json(&json!([]));
}

#[tokio::test]
async fn test_device_logs_returns_data() {
    let (server, db) = test_server_with_db();
    db.insert_log("launa_spa", "info", "system started", 1000);
    db.insert_log("launa_spa", "warn", "temp high", 2000);
    db.insert_log("other_device", "error", "fault", 3000);

    let resp = server.get("/api/devices/launa_spa/logs").await;
    resp.assert_status_ok();

    let logs: Vec<serde_json::Value> = resp.json();
    assert_eq!(logs.len(), 2);
    // Most recent first
    assert_eq!(logs[0]["level"], "warn");
    assert_eq!(logs[0]["message"], "temp high");
    assert_eq!(logs[1]["level"], "info");
    assert_eq!(logs[1]["message"], "system started");
}

#[tokio::test]
async fn test_device_logs_limit() {
    let (server, db) = test_server_with_db();
    for i in 0..10 {
        db.insert_log("dev1", "info", &format!("msg {i}"), i);
    }

    let resp = server.get("/api/devices/dev1/logs?limit=3").await;
    resp.assert_status_ok();

    let logs: Vec<serde_json::Value> = resp.json();
    assert_eq!(logs.len(), 3);
}

#[tokio::test]
async fn test_device_status_empty() {
    let server = test_server_empty();

    let resp = server.get("/api/devices/launa_spa/status").await;
    resp.assert_status_ok();
    resp.assert_json(&json!([]));
}

#[tokio::test]
async fn test_device_latest_status_empty() {
    let server = test_server_empty();

    let resp = server.get("/api/devices/launa_spa/status/latest").await;
    resp.assert_status_ok();
    resp.assert_json(&json!(null));
}

#[tokio::test]
async fn test_device_latest_status_returns_most_recent() {
    let (server, db) = test_server_with_db();
    db.insert_status("launa_spa", r#"{"current_temp":38}"#);
    db.insert_status("launa_spa", r#"{"current_temp":39}"#);

    let resp = server.get("/api/devices/launa_spa/status/latest").await;
    resp.assert_status_ok();

    let status: serde_json::Value = resp.json();
    assert_eq!(status["payload"], r#"{"current_temp":39}"#);
}

#[tokio::test]
async fn test_device_alerts() {
    let (server, db) = test_server_with_db();
    db.insert_alert("launa_spa", r#"{"msg":"overheat"}"#);

    let resp = server.get("/api/devices/launa_spa/alerts").await;
    resp.assert_status_ok();

    let alerts: Vec<serde_json::Value> = resp.json();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["payload"], r#"{"msg":"overheat"}"#);
}

#[tokio::test]
async fn test_device_diagnostics() {
    let (server, db) = test_server_with_db();
    db.insert_diagnostics("launa_spa", r#"{"uptime":1234}"#);

    let resp = server.get("/api/devices/launa_spa/diagnostics").await;
    resp.assert_status_ok();

    let diags: Vec<serde_json::Value> = resp.json();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["payload"], r#"{"uptime":1234}"#);
}

#[tokio::test]
async fn test_device_sniff_frames() {
    let (server, db) = test_server_with_db();
    db.insert_sniff_frame("launa_spa", r#"{"hex":"aabbcc"}"#);

    let resp = server.get("/api/devices/launa_spa/sniff").await;
    resp.assert_status_ok();

    let frames: Vec<serde_json::Value> = resp.json();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["payload"], r#"{"hex":"aabbcc"}"#);
}

#[tokio::test]
async fn test_device_isolation() {
    let (server, db) = test_server_with_db();
    db.insert_log("device_a", "info", "msg a", 100);
    db.insert_log("device_b", "info", "msg b", 200);

    let resp_a = server.get("/api/devices/device_a/logs").await;
    let logs_a: Vec<serde_json::Value> = resp_a.json();
    assert_eq!(logs_a.len(), 1);
    assert_eq!(logs_a[0]["message"], "msg a");

    let resp_b = server.get("/api/devices/device_b/logs").await;
    let logs_b: Vec<serde_json::Value> = resp_b.json();
    assert_eq!(logs_b.len(), 1);
    assert_eq!(logs_b[0]["message"], "msg b");
}
