use std::path::Path;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use rusqlite::{params, Connection, Result as SqlResult};

const MAX_LOG_ENTRIES: u64 = 1000;
const STATUS_RETENTION_DAYS: i64 = 7;
const GRAPH_RETENTION_DAYS: i64 = 14;
const TEMP_SAMPLE_MIN_INTERVAL_SECS: i64 = 30;
const TEMP_MIN_DELTA: f64 = 0.4;

/// Component boolean fields tracked for state-change events.
const COMPONENT_FIELDS: &[&str] = &[
    "is_heating",
    "pump1_on",
    "pump2_on",
    "pump3_on",
    "pump4_on",
    "pump5_on",
    "pump6_on",
    "circ_pump",
    "blower",
    "light1",
    "light2",
    "light3",
    "light4",
    "mister",
];

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS devices (
                device_id   TEXT PRIMARY KEY,
                status      TEXT NOT NULL DEFAULT 'offline',
                boot_id     INTEGER,
                updated_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS device_logs (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                level       TEXT NOT NULL,
                message     TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_logs_device ON device_logs(device_id);
            CREATE INDEX IF NOT EXISTS idx_logs_received ON device_logs(received_at);

            CREATE TABLE IF NOT EXISTS status_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                payload     TEXT NOT NULL,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_status_device ON status_history(device_id);
            CREATE INDEX IF NOT EXISTS idx_status_received ON status_history(received_at);

            CREATE TABLE IF NOT EXISTS temperature_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                current_temp REAL,
                set_temp     REAL,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_temp_device ON temperature_history(device_id);
            CREATE INDEX IF NOT EXISTS idx_temp_received ON temperature_history(received_at);

            CREATE TABLE IF NOT EXISTS component_events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                component   TEXT NOT NULL,
                state       INTEGER NOT NULL,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_comp_device ON component_events(device_id);
            CREATE INDEX IF NOT EXISTS idx_comp_received ON component_events(received_at);

            CREATE TABLE IF NOT EXISTS diagnostics (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                payload     TEXT NOT NULL,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_diag_device ON diagnostics(device_id);

            CREATE TABLE IF NOT EXISTS alerts (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                payload     TEXT NOT NULL,
                received_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sniff_frames (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                payload     TEXT NOT NULL,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_sniff_device ON sniff_frames(device_id);

            CREATE TABLE IF NOT EXISTS availability_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT NOT NULL,
                status      TEXT NOT NULL,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_avail_device ON availability_history(device_id);
            ",
        )?;
        Ok(())
    }

    pub fn insert_log(&self, device_id: &str, level: &str, message: &str, timestamp_ms: u64) {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let _ = conn.execute(
            "INSERT INTO device_logs (device_id, level, message, timestamp_ms, received_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![device_id, level, message, timestamp_ms, now],
        );
        self.trim_logs(&conn);
    }

    fn trim_logs(&self, conn: &Connection) {
        let _ = conn.execute(
            "DELETE FROM device_logs WHERE id NOT IN (SELECT id FROM device_logs ORDER BY id DESC LIMIT ?1)",
            params![MAX_LOG_ENTRIES],
        );
    }

    pub fn insert_status(&self, device_id: &str, payload: &str) {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        // Always store in raw status_history (trimmed to STATUS_RETENTION_DAYS)
        let _ = conn.execute(
            "INSERT INTO status_history (device_id, payload, received_at) VALUES (?1, ?2, ?3)",
            params![device_id, payload, now],
        );
        self.trim_status(&conn);

        // Parse payload for deduplicated graph tables
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) {
            self.insert_temperature_sample(&conn, device_id, &val, &now);
            self.insert_component_changes(&conn, device_id, &val, &now);
        }
    }

    fn insert_temperature_sample(
        &self,
        conn: &Connection,
        device_id: &str,
        val: &serde_json::Value,
        now: &str,
    ) {
        let current_temp = val.get("current_temp").and_then(|v| v.as_f64());
        let set_temp = val.get("set_temp").and_then(|v| v.as_f64());

        // Check if we should store a new sample
        let should_insert = match self.get_last_temp_sample(conn, device_id) {
            Some((last_temp, last_set, last_time)) => {
                let elapsed = now.parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|t| t.signed_duration_since(last_time).num_seconds())
                    .unwrap_or(i64::MAX);

                let temp_changed = match (current_temp, last_temp) {
                    (Some(cur), Some(last)) => (cur - last).abs() >= TEMP_MIN_DELTA,
                    (Some(_), None) => true,
                    _ => false,
                };
                let set_changed = match (set_temp, last_set) {
                    (Some(cur), Some(last)) => cur != last,
                    (Some(_), None) => true,
                    _ => false,
                };

                elapsed >= TEMP_SAMPLE_MIN_INTERVAL_SECS || temp_changed || set_changed
            }
            None => true,
        };

        if should_insert {
            let _ = conn.execute(
                "INSERT INTO temperature_history (device_id, current_temp, set_temp, received_at) VALUES (?1, ?2, ?3, ?4)",
                params![device_id, current_temp, set_temp, now],
            );
            self.trim_temperature(&conn);
        }
    }

    fn get_last_temp_sample(
        &self,
        conn: &Connection,
        device_id: &str,
    ) -> Option<(Option<f64>, Option<f64>, chrono::DateTime<chrono::Utc>)> {
        let mut stmt = conn.prepare(
            "SELECT current_temp, set_temp, received_at FROM temperature_history WHERE device_id = ?1 ORDER BY id DESC LIMIT 1",
        ).ok()?;
        let mut rows = stmt.query_map(params![device_id], |row| {
            let ct: Option<f64> = row.get(0)?;
            let st: Option<f64> = row.get(1)?;
            let ra: String = row.get(2)?;
            Ok((ct, st, ra))
        }).ok()?;
        rows.next().and_then(|r| r.ok()).and_then(|(ct, st, ra)| {
            ra.parse::<chrono::DateTime<chrono::Utc>>().ok().map(|t| (ct, st, t))
        })
    }

    fn insert_component_changes(
        &self,
        conn: &Connection,
        device_id: &str,
        val: &serde_json::Value,
        now: &str,
    ) {
        for field in COMPONENT_FIELDS {
            let new_state = val.get(*field).and_then(|v| v.as_bool()).unwrap_or(false);
            let last_state = self.get_last_component_state(conn, device_id, field);
            if last_state != Some(new_state) {
                let _ = conn.execute(
                    "INSERT INTO component_events (device_id, component, state, received_at) VALUES (?1, ?2, ?3, ?4)",
                    params![device_id, field, new_state as i32, now],
                );
            }
        }
        self.trim_components(&conn);
    }

    fn get_last_component_state(
        &self,
        conn: &Connection,
        device_id: &str,
        component: &str,
    ) -> Option<bool> {
        let mut stmt = conn.prepare(
            "SELECT state FROM component_events WHERE device_id = ?1 AND component = ?2 ORDER BY id DESC LIMIT 1",
        ).ok()?;
        let mut rows = stmt.query_map(params![device_id, component], |row| {
            let s: i32 = row.get(0)?;
            Ok(s != 0)
        }).ok()?;
        rows.next().and_then(|r| r.ok())
    }

    fn trim_temperature(&self, conn: &Connection) {
        let cutoff = (Utc::now() - Duration::days(GRAPH_RETENTION_DAYS)).to_rfc3339();
        let _ = conn.execute(
            "DELETE FROM temperature_history WHERE received_at < ?1",
            params![cutoff],
        );
    }

    fn trim_components(&self, conn: &Connection) {
        let cutoff = (Utc::now() - Duration::days(GRAPH_RETENTION_DAYS)).to_rfc3339();
        let _ = conn.execute(
            "DELETE FROM component_events WHERE received_at < ?1",
            params![cutoff],
        );
    }

    fn trim_status(&self, conn: &Connection) {
        let cutoff = (Utc::now() - Duration::days(STATUS_RETENTION_DAYS)).to_rfc3339();
        let _ = conn.execute(
            "DELETE FROM status_history WHERE received_at < ?1",
            params![cutoff],
        );
    }

    fn insert_timestamped(&self, table: &str, device_id: &str, payload: &str) {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let sql = format!(
            "INSERT INTO {} (device_id, payload, received_at) VALUES (?1, ?2, ?3)",
            table
        );
        let _ = conn.execute(&sql, params![device_id, payload, now]);
    }

    pub fn insert_diagnostics(&self, device_id: &str, payload: &str) {
        self.insert_timestamped("diagnostics", device_id, payload);
    }

    pub fn insert_alert(&self, device_id: &str, payload: &str) {
        self.insert_timestamped("alerts", device_id, payload);
    }

    pub fn insert_sniff_frame(&self, device_id: &str, payload: &str) {
        self.insert_timestamped("sniff_frames", device_id, payload);
    }

    pub fn get_logs(&self, device_id: &str, limit: u64) -> Vec<LogEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT level, message, timestamp_ms, received_at FROM device_logs WHERE device_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![device_id, limit], |row| {
                Ok(LogEntry {
                    level: row.get(0)?,
                    message: row.get(1)?,
                    timestamp_ms: row.get(2)?,
                    received_at: row.get(3)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get_status_history(&self, device_id: &str, limit: u64) -> Vec<StatusEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT payload, received_at FROM status_history WHERE device_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![device_id, limit], |row| {
                Ok(StatusEntry {
                    payload: row.get(0)?,
                    received_at: row.get(1)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get_status_history_since(&self, device_id: &str, since: &str) -> Vec<StatusEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT payload, received_at FROM status_history WHERE device_id = ?1 AND received_at >= ?2 ORDER BY id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![device_id, since], |row| {
                Ok(StatusEntry {
                    payload: row.get(0)?,
                    received_at: row.get(1)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get_temperature_history_since(&self, device_id: &str, since: &str) -> Vec<TemperatureSample> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT current_temp, set_temp, received_at FROM temperature_history WHERE device_id = ?1 AND received_at >= ?2 ORDER BY id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![device_id, since], |row| {
                Ok(TemperatureSample {
                    current_temp: row.get(0)?,
                    set_temp: row.get(1)?,
                    received_at: row.get(2)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get_component_events_since(&self, device_id: &str, since: &str) -> Vec<ComponentEvent> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT component, state, received_at FROM component_events WHERE device_id = ?1 AND received_at >= ?2 ORDER BY id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![device_id, since], |row| {
                Ok(ComponentEvent {
                    component: row.get(0)?,
                    state: row.get(1)?,
                    received_at: row.get(2)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get_latest_status(&self, device_id: &str) -> Option<StatusEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT payload, received_at FROM status_history WHERE device_id = ?1 ORDER BY id DESC LIMIT 1",
            )
            .unwrap();
        let mut rows = stmt
            .query_map(params![device_id], |row| {
                Ok(StatusEntry {
                    payload: row.get(0)?,
                    received_at: row.get(1)?,
                })
            })
            .unwrap();
        rows.next().and_then(|r| r.ok())
    }

    fn get_timestamped(&self, table: &str, device_id: &str, limit: u64) -> Vec<TimestampedEntry> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT payload, received_at FROM {} WHERE device_id = ?1 ORDER BY id DESC LIMIT ?2",
            table
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows = stmt
            .query_map(params![device_id, limit], |row| {
                Ok(TimestampedEntry {
                    payload: row.get(0)?,
                    received_at: row.get(1)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get_alerts(&self, device_id: &str, limit: u64) -> Vec<TimestampedEntry> {
        self.get_timestamped("alerts", device_id, limit)
    }

    pub fn get_diagnostics(&self, device_id: &str, limit: u64) -> Vec<TimestampedEntry> {
        self.get_timestamped("diagnostics", device_id, limit)
    }

    pub fn get_sniff_frames(&self, device_id: &str, limit: u64) -> Vec<TimestampedEntry> {
        self.get_timestamped("sniff_frames", device_id, limit)
    }

    fn clear_table(&self, table: &str, device_id: &str) {
        let conn = self.conn.lock().unwrap();
        let sql = format!("DELETE FROM {} WHERE device_id = ?1", table);
        let _ = conn.execute(&sql, params![device_id]);
    }

    pub fn clear_logs(&self, device_id: &str) {
        self.clear_table("device_logs", device_id);
    }

    pub fn clear_alerts(&self, device_id: &str) {
        self.clear_table("alerts", device_id);
    }

    pub fn clear_diagnostics(&self, device_id: &str) {
        self.clear_table("diagnostics", device_id);
    }

    pub fn clear_sniff_frames(&self, device_id: &str) {
        self.clear_table("sniff_frames", device_id);
    }

    pub fn insert_availability(&self, device_id: &str, status: &str) {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        // Only insert if the status changed from the last recorded value
        let last = {
            let mut stmt = conn
                .prepare(
                    "SELECT status FROM availability_history WHERE device_id = ?1 ORDER BY id DESC LIMIT 1",
                )
                .unwrap();
            let mut rows = stmt.query_map(params![device_id], |row| row.get::<_, String>(0)).unwrap();
            rows.next().and_then(|r| r.ok())
        };
        if last.as_deref() != Some(status) {
            let _ = conn.execute(
                "INSERT INTO availability_history (device_id, status, received_at) VALUES (?1, ?2, ?3)",
                params![device_id, status, now],
            );
            self.trim_availability(&conn);
        }
    }

    fn trim_availability(&self, conn: &Connection) {
        let cutoff = (Utc::now() - Duration::days(GRAPH_RETENTION_DAYS)).to_rfc3339();
        let _ = conn.execute(
            "DELETE FROM availability_history WHERE received_at < ?1",
            params![cutoff],
        );
    }

    pub fn get_availability_history(&self, device_id: &str, limit: u64) -> Vec<AvailabilityEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT status, received_at FROM availability_history WHERE device_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![device_id, limit], |row| {
                Ok(AvailabilityEntry {
                    status: row.get(0)?,
                    received_at: row.get(1)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get_availability_history_since(&self, device_id: &str, since: &str) -> Vec<AvailabilityEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT status, received_at FROM availability_history WHERE device_id = ?1 AND received_at >= ?2 ORDER BY id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![device_id, since], |row| {
                Ok(AvailabilityEntry {
                    status: row.get(0)?,
                    received_at: row.get(1)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Update device availability status and optional boot_id.
    ///
    /// Uses UPSERT so the first seen message for a device_id creates the row.
    /// `boot_id` is only updated when Some (i.e., on "online" messages).
    pub fn update_device_status(&self, device_id: &str, status: &str, boot_id: Option<u32>) {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        match boot_id {
            Some(bid) => {
                let _ = conn.execute(
                    "INSERT INTO devices (device_id, status, boot_id, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(device_id) DO UPDATE SET status = ?2, boot_id = ?3, updated_at = ?4",
                    params![device_id, status, bid, now],
                );
            }
            None => {
                let _ = conn.execute(
                    "INSERT INTO devices (device_id, status, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(device_id) DO UPDATE SET status = ?2, updated_at = ?3",
                    params![device_id, status, now],
                );
            }
        }
    }

    /// Get the current device status (online/offline/stale) and boot_id.
    pub fn get_device_status(&self, device_id: &str) -> Option<DeviceStatus> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT status, boot_id, updated_at FROM devices WHERE device_id = ?1")
            .unwrap();
        let mut rows = stmt
            .query_map(params![device_id], |row| {
                Ok(DeviceStatus {
                    status: row.get(0)?,
                    boot_id: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            })
            .unwrap();
        rows.next().and_then(|r| r.ok())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceStatus {
    pub status: String,
    pub boot_id: Option<u32>,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
    pub timestamp_ms: u64,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusEntry {
    pub payload: String,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TimestampedEntry {
    pub payload: String,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AvailabilityEntry {
    pub status: String,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TemperatureSample {
    pub current_temp: Option<f64>,
    pub set_temp: Option<f64>,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ComponentEvent {
    pub component: String,
    pub state: i32,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphData {
    pub temperatures: Vec<TemperatureSample>,
    pub components: Vec<ComponentEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_query_logs() {
        let db = Database::open_in_memory().unwrap();
        db.insert_log("spa_001", "warn", "Temperature high", 12345);
        db.insert_log("spa_001", "error", "Sensor fault", 12350);

        let logs = db.get_logs("spa_001", 10);
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].level, "error");
        assert_eq!(logs[1].level, "warn");
    }

    #[test]
    fn test_log_ring_buffer_trim() {
        let db = Database::open_in_memory().unwrap();
        for i in 0..1100 {
            db.insert_log("spa_001", "info", &format!("msg {}", i), i);
        }
        let logs = db.get_logs("spa_001", 2000);
        assert_eq!(logs.len(), 1000);
    }

    #[test]
    fn test_insert_and_query_status() {
        let db = Database::open_in_memory().unwrap();
        db.insert_status("spa_001", r#"{"current_temp":100}"#);

        let latest = db.get_latest_status("spa_001").unwrap();
        assert!(latest.payload.contains("100"));
    }

    #[test]
    fn test_latest_status_empty() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_latest_status("spa_001").is_none());
    }

    #[test]
    fn test_device_isolation() {
        let db = Database::open_in_memory().unwrap();
        db.insert_log("spa_001", "info", "msg1", 100);
        db.insert_log("spa_002", "info", "msg2", 200);

        let logs_1 = db.get_logs("spa_001", 10);
        let logs_2 = db.get_logs("spa_002", 10);
        assert_eq!(logs_1.len(), 1);
        assert_eq!(logs_2.len(), 1);
        assert_eq!(logs_1[0].message, "msg1");
        assert_eq!(logs_2[0].message, "msg2");
    }

    #[test]
    fn test_alerts_and_diagnostics_and_sniff() {
        let db = Database::open_in_memory().unwrap();
        db.insert_alert("spa_001", r#"{"msg":"overheat"}"#);
        db.insert_diagnostics("spa_001", r#"{"uptime":1234}"#);
        db.insert_sniff_frame("spa_001", r#"{"hex":"aabbcc"}"#);

        let alerts = db.get_alerts("spa_001", 10);
        let diags = db.get_diagnostics("spa_001", 10);
        let sniffs = db.get_sniff_frames("spa_001", 10);

        assert_eq!(alerts.len(), 1);
        assert_eq!(diags.len(), 1);
        assert_eq!(sniffs.len(), 1);
        assert!(alerts[0].payload.contains("overheat"));
        assert!(diags[0].payload.contains("uptime"));
        assert!(sniffs[0].payload.contains("aabbcc"));
    }

    #[test]
    fn test_status_history_ordering() {
        let db = Database::open_in_memory().unwrap();
        db.insert_status("spa_001", r#"{"t":1}"#);
        db.insert_status("spa_001", r#"{"t":2}"#);
        db.insert_status("spa_001", r#"{"t":3}"#);

        let history = db.get_status_history("spa_001", 10);
        assert_eq!(history.len(), 3);
        assert!(history[0].payload.contains(r#""t":3"#));
        assert!(history[2].payload.contains(r#""t":1"#));
    }

    #[test]
    fn test_temperature_dedup_by_interval() {
        let db = Database::open_in_memory().unwrap();
        // First insert should always be stored
        db.insert_status("spa_001", r#"{"current_temp":100.0,"set_temp":104.0}"#);
        let samples = db.get_temperature_history_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(samples.len(), 1);

        // Same temp, same instant (too fast) should NOT create new row
        db.insert_status("spa_001", r#"{"current_temp":100.0,"set_temp":104.0}"#);
        let samples2 = db.get_temperature_history_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(samples2.len(), 1, "Should not insert duplicate within interval");
    }

    #[test]
    fn test_temperature_insert_on_significant_change() {
        let db = Database::open_in_memory().unwrap();
        db.insert_status("spa_001", r#"{"current_temp":100.0,"set_temp":104.0}"#);
        // Change by >= 0.5 should insert
        db.insert_status("spa_001", r#"{"current_temp":100.5,"set_temp":104.0}"#);
        let samples = db.get_temperature_history_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(samples.len(), 2, "Should insert on significant temp change");
    }

    #[test]
    fn test_temperature_no_insert_on_small_change() {
        let db = Database::open_in_memory().unwrap();
        db.insert_status("spa_001", r#"{"current_temp":100.0,"set_temp":104.0}"#);
        // Change by < 0.5 should NOT insert
        db.insert_status("spa_001", r#"{"current_temp":100.2,"set_temp":104.0}"#);
        let samples = db.get_temperature_history_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(samples.len(), 1, "Should not insert on small temp change");
    }

    #[test]
    fn test_component_events_only_on_change() {
        let db = Database::open_in_memory().unwrap();
        // All 14 component fields: first insert records initial states for all
        let payload = r#"{"is_heating":false,"pump1_on":false,"pump2_on":false,"pump3_on":false,"pump4_on":false,"pump5_on":false,"pump6_on":false,"circ_pump":false,"blower":false,"light1":false,"light2":false,"light3":false,"light4":false,"mister":false}"#;
        db.insert_status("spa_001", payload);
        let events = db.get_component_events_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(events.len(), 14, "First insert should record initial states for all 14 components");

        // Same state - no new events
        db.insert_status("spa_001", payload);
        let events2 = db.get_component_events_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(events2.len(), 14, "No change should not add events");

        // Heater turns on
        let payload_on = r#"{"is_heating":true,"pump1_on":false,"pump2_on":false,"pump3_on":false,"pump4_on":false,"pump5_on":false,"pump6_on":false,"circ_pump":false,"blower":false,"light1":false,"light2":false,"light3":false,"light4":false,"mister":false}"#;
        db.insert_status("spa_001", payload_on);
        let events3 = db.get_component_events_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(events3.len(), 15, "Should add event for heating change");
        assert_eq!(events3[14].component, "is_heating");
        assert_eq!(events3[14].state, 1);
    }

    #[test]
    fn test_null_temp_always_inserts() {
        let db = Database::open_in_memory().unwrap();
        db.insert_status("spa_001", r#"{"current_temp":null,"set_temp":100}"#);
        db.insert_status("spa_001", r#"{"current_temp":100.0,"set_temp":100}"#);
        let samples = db.get_temperature_history_since(
            "spa_001",
            &(Utc::now() - Duration::hours(1)).to_rfc3339(),
        );
        assert_eq!(samples.len(), 2, "Null->value transition should insert");
    }
}
