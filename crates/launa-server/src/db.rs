use std::path::Path;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use rusqlite::{params, Connection, Result as SqlResult};

const MAX_LOG_ENTRIES: u64 = 1000;
const STATUS_RETENTION_DAYS: i64 = 7;

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
        let _ = conn.execute(
            "INSERT INTO status_history (device_id, payload, received_at) VALUES (?1, ?2, ?3)",
            params![device_id, payload, now],
        );
        self.trim_status(&conn);
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
        let _ = conn.execute(
            "INSERT INTO availability_history (device_id, status, received_at) VALUES (?1, ?2, ?3)",
            params![device_id, status, now],
        );
        self.trim_availability(&conn);
    }

    fn trim_availability(&self, conn: &Connection) {
        let cutoff = (Utc::now() - Duration::days(STATUS_RETENTION_DAYS)).to_rfc3339();
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
}
