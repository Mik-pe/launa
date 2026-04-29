//! In-memory store for all launa-server data.
//!
//! Uses capped VecDeque ring buffers to limit memory usage. State is persisted
//! to a JSON file on shutdown and reloaded on startup (best-effort; data loss
//! on crash is acceptable).
//!
//! Timestamp comparison uses RFC3339 lexicographic ordering, which is correct
//! as long as all timestamps are produced by this module's `now_rfc3339()`
//! (i.e. UTC with consistent sub-second precision).

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;

use chrono::Utc;
use tracing::{info, warn};

// --- Shared types ---

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceStatus {
    pub status: String,
    pub boot_id: Option<u32>,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
    pub timestamp_ms: u64,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TimestampedEntry {
    pub payload: String,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AvailabilityEntry {
    pub status: String,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemperatureSample {
    pub current_temp: Option<f64>,
    pub set_temp: Option<f64>,
    pub received_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// Summary of a known device, returned by the device-list endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceSummary {
    pub device_id: String,
    pub status: String,
    pub boot_id: Option<u32>,
    pub updated_at: String,
}

// --- Constants ---

const MAX_LOG_ENTRIES: usize = 1000;
const MAX_DIAGNOSTICS_ENTRIES: usize = 200;
const MAX_ALERT_ENTRIES: usize = 200;
const MAX_SNIFF_ENTRIES: usize = 500;
const MAX_AVAILABILITY_ENTRIES: usize = 500;
const MAX_TEMPERATURE_ENTRIES: usize = 50_000; // ~14 days at 30s intervals
const MAX_COMPONENT_ENTRIES: usize = 10_000;

const TEMP_SAMPLE_MIN_INTERVAL_SECS: i64 = 30;
const TEMP_MIN_DELTA: f64 = 0.4;

/// Component boolean fields tracked for state-change events.
pub const COMPONENT_FIELDS: &[&str] = &[
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

// --- Helpers ---

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn push_capped<T>(deque: &mut VecDeque<T>, item: T, max: usize) {
    if deque.len() >= max {
        deque.pop_front();
    }
    deque.push_back(item);
}

/// Return the last `limit` items in chronological order (oldest first).
fn recent<T: Clone>(deque: &VecDeque<T>, limit: usize) -> Vec<T> {
    let skip = deque.len().saturating_sub(limit);
    deque.iter().skip(skip).cloned().collect()
}

/// Return items whose timestamp is >= `since`, in chronological order.
///
/// Relies on RFC3339 lexicographic ordering.
fn recent_since<T: Clone>(
    deque: &VecDeque<T>,
    get_time: impl Fn(&T) -> &str,
    since: &str,
) -> Vec<T> {
    deque
        .iter()
        .filter(|item| get_time(item) >= since)
        .cloned()
        .collect()
}

// --- Serialization ---

/// Serializable representation of MemoryStore contents.
/// Uses Vec instead of VecDeque for clean JSON serialization.
#[derive(serde::Serialize, serde::Deserialize)]
struct SerializedStore {
    devices: HashMap<String, DeviceStatus>,
    accessory_config: Option<AccessoryConfigData>,
    logs: HashMap<String, Vec<LogEntry>>,
    diagnostics: HashMap<String, Vec<TimestampedEntry>>,
    alerts: HashMap<String, Vec<TimestampedEntry>>,
    sniff_frames: HashMap<String, Vec<TimestampedEntry>>,
    availability: HashMap<String, Vec<AvailabilityEntry>>,
    temperatures: HashMap<String, Vec<TemperatureSample>>,
    component_events: HashMap<String, Vec<ComponentEvent>>,
}

/// Persisted accessory configuration. Stored inside SerializedStore.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccessoryConfigData {
    pub pumps: u8,
    pub lights: u8,
    pub blower: bool,
    pub mister: bool,
}

// --- MemoryStore ---

pub struct MemoryStore {
    devices: HashMap<String, DeviceStatus>,
    accessory_config: AccessoryConfigData,
    logs: HashMap<String, VecDeque<LogEntry>>,
    diagnostics: HashMap<String, VecDeque<TimestampedEntry>>,
    alerts: HashMap<String, VecDeque<TimestampedEntry>>,
    sniff_frames: HashMap<String, VecDeque<TimestampedEntry>>,
    availability: HashMap<String, VecDeque<AvailabilityEntry>>,
    temperatures: HashMap<String, VecDeque<TemperatureSample>>,
    component_events: HashMap<String, VecDeque<ComponentEvent>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            accessory_config: AccessoryConfigData::default(),
            logs: HashMap::new(),
            diagnostics: HashMap::new(),
            alerts: HashMap::new(),
            sniff_frames: HashMap::new(),
            availability: HashMap::new(),
            temperatures: HashMap::new(),
            component_events: HashMap::new(),
        }
    }

    /// Load state from a JSON file. Returns a new empty store if the file
    /// doesn't exist or is malformed (best-effort).
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(data) => match serde_json::from_str::<SerializedStore>(&data) {
                Ok(s) => {
                    let store = Self {
                        devices: s.devices,
                        accessory_config: s.accessory_config.unwrap_or_default(),
                        logs: s.logs.into_iter().map(|(k, v)| (k, v.into())).collect(),
                        diagnostics: s
                            .diagnostics
                            .into_iter()
                            .map(|(k, v)| (k, v.into()))
                            .collect(),
                        alerts: s.alerts.into_iter().map(|(k, v)| (k, v.into())).collect(),
                        sniff_frames: s
                            .sniff_frames
                            .into_iter()
                            .map(|(k, v)| (k, v.into()))
                            .collect(),
                        availability: s
                            .availability
                            .into_iter()
                            .map(|(k, v)| (k, v.into()))
                            .collect(),
                        temperatures: s
                            .temperatures
                            .into_iter()
                            .map(|(k, v)| (k, v.into()))
                            .collect(),
                        component_events: s
                            .component_events
                            .into_iter()
                            .map(|(k, v)| (k, v.into()))
                            .collect(),
                    };
                    let log_count = store.logs.values().map(|d| d.len()).sum::<usize>();
                    let temp_count = store.temperatures.values().map(|d| d.len()).sum::<usize>();
                    info!(
                        "Loaded memory store from {:?} ({} logs, {} temp samples)",
                        path, log_count, temp_count
                    );
                    store
                }
                Err(e) => {
                    warn!("Failed to parse memory store {:?}: {e}", path);
                    Self::new()
                }
            },
            Err(_) => {
                info!("No memory store file at {:?}, starting fresh", path);
                Self::new()
            }
        }
    }

    /// Persist state to a JSON file (best-effort, errors are logged only).
    ///
    /// Writes to a `.tmp` file first, then atomically renames over the target.
    /// This prevents corruption if the process crashes mid-write.
    pub fn save(&self, path: &Path) {
        let serialized = SerializedStore {
            devices: self.devices.clone(),
            accessory_config: Some(self.accessory_config.clone()),
            logs: self
                .logs
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            diagnostics: self
                .diagnostics
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            alerts: self
                .alerts
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            sniff_frames: self
                .sniff_frames
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            availability: self
                .availability
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            temperatures: self
                .temperatures
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            component_events: self
                .component_events
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
        };
        match serde_json::to_string(&serialized) {
            Ok(json) => {
                let tmp = path.with_extension("tmp");
                if let Err(e) = std::fs::write(&tmp, &json) {
                    warn!("Failed to write temp state file {:?}: {e}", tmp);
                    return;
                }
                if let Err(e) = std::fs::rename(&tmp, path) {
                    warn!("Failed to rename temp state file to {:?}: {e}", path);
                }
            }
            Err(e) => {
                warn!("Failed to serialize memory store: {e}");
            }
        }
    }

    // --- Device status ---

    pub fn update_device_status(&mut self, device_id: &str, status: &str, boot_id: Option<u32>) {
        let now = now_rfc3339();
        self.devices.insert(
            device_id.to_string(),
            DeviceStatus {
                status: status.to_string(),
                boot_id,
                updated_at: now,
            },
        );
    }

    pub fn get_device_status(&self, device_id: &str) -> Option<DeviceStatus> {
        self.devices.get(device_id).cloned()
    }

    /// Return a summary of all known devices, sorted by device_id.
    pub fn list_devices(&self) -> Vec<DeviceSummary> {
        let mut devices: Vec<_> = self
            .devices
            .iter()
            .map(|(id, d)| DeviceSummary {
                device_id: id.clone(),
                status: d.status.clone(),
                boot_id: d.boot_id,
                updated_at: d.updated_at.clone(),
            })
            .collect();
        devices.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        devices
    }

    // --- Accessory config ---

    pub fn get_accessory_config(&self) -> &AccessoryConfigData {
        &self.accessory_config
    }

    pub fn set_accessory_config(&mut self, config: AccessoryConfigData) {
        self.accessory_config = config;
    }

    // --- Logs ---

    pub fn insert_log(&mut self, device_id: &str, level: &str, message: &str, timestamp_ms: u64) {
        let entry = LogEntry {
            level: level.to_string(),
            message: message.to_string(),
            timestamp_ms,
            received_at: now_rfc3339(),
        };
        let deque = self.logs.entry(device_id.to_string()).or_default();
        push_capped(deque, entry, MAX_LOG_ENTRIES);
    }

    /// Return the last `limit` log entries, newest first.
    pub fn get_logs(&self, device_id: &str, limit: u64) -> Vec<LogEntry> {
        match self.logs.get(device_id) {
            Some(d) => recent(d, limit as usize).into_iter().rev().collect(),
            None => Vec::new(),
        }
    }

    pub fn clear_logs(&mut self, device_id: &str) {
        if let Some(deque) = self.logs.get_mut(device_id) {
            deque.clear();
        }
    }

    // --- Diagnostics ---

    pub fn insert_diagnostics(&mut self, device_id: &str, payload: &str) {
        let entry = TimestampedEntry {
            payload: payload.to_string(),
            received_at: now_rfc3339(),
        };
        let deque = self.diagnostics.entry(device_id.to_string()).or_default();
        push_capped(deque, entry, MAX_DIAGNOSTICS_ENTRIES);
    }

    /// Return the last `limit` diagnostics entries, newest first.
    pub fn get_diagnostics(&self, device_id: &str, limit: u64) -> Vec<TimestampedEntry> {
        match self.diagnostics.get(device_id) {
            Some(d) => recent(d, limit as usize).into_iter().rev().collect(),
            None => Vec::new(),
        }
    }

    pub fn clear_diagnostics(&mut self, device_id: &str) {
        if let Some(deque) = self.diagnostics.get_mut(device_id) {
            deque.clear();
        }
    }

    // --- Alerts ---

    pub fn insert_alert(&mut self, device_id: &str, payload: &str) {
        let entry = TimestampedEntry {
            payload: payload.to_string(),
            received_at: now_rfc3339(),
        };
        let deque = self.alerts.entry(device_id.to_string()).or_default();
        push_capped(deque, entry, MAX_ALERT_ENTRIES);
    }

    /// Return the last `limit` alerts, newest first.
    pub fn get_alerts(&self, device_id: &str, limit: u64) -> Vec<TimestampedEntry> {
        match self.alerts.get(device_id) {
            Some(d) => recent(d, limit as usize).into_iter().rev().collect(),
            None => Vec::new(),
        }
    }

    pub fn clear_alerts(&mut self, device_id: &str) {
        if let Some(deque) = self.alerts.get_mut(device_id) {
            deque.clear();
        }
    }

    // --- Sniff frames ---

    pub fn insert_sniff_frame(&mut self, device_id: &str, payload: &str) {
        let entry = TimestampedEntry {
            payload: payload.to_string(),
            received_at: now_rfc3339(),
        };
        let deque = self.sniff_frames.entry(device_id.to_string()).or_default();
        push_capped(deque, entry, MAX_SNIFF_ENTRIES);
    }

    /// Return the last `limit` sniff frames, newest first.
    pub fn get_sniff_frames(&self, device_id: &str, limit: u64) -> Vec<TimestampedEntry> {
        match self.sniff_frames.get(device_id) {
            Some(d) => recent(d, limit as usize).into_iter().rev().collect(),
            None => Vec::new(),
        }
    }

    pub fn clear_sniff_frames(&mut self, device_id: &str) {
        if let Some(deque) = self.sniff_frames.get_mut(device_id) {
            deque.clear();
        }
    }

    // --- Availability ---

    pub fn insert_availability(&mut self, device_id: &str, status: &str) {
        let deque = self.availability.entry(device_id.to_string()).or_default();
        if deque.back().map_or(true, |e| e.status != status) {
            let entry = AvailabilityEntry {
                status: status.to_string(),
                received_at: now_rfc3339(),
            };
            push_capped(deque, entry, MAX_AVAILABILITY_ENTRIES);
        }
    }

    /// Return the last `limit` availability entries, newest first.
    pub fn get_availability_history(&self, device_id: &str, limit: u64) -> Vec<AvailabilityEntry> {
        match self.availability.get(device_id) {
            Some(d) => recent(d, limit as usize).into_iter().rev().collect(),
            None => Vec::new(),
        }
    }

    /// Return availability entries since `since`, chronological order (oldest first).
    pub fn get_availability_history_since(
        &self,
        device_id: &str,
        since: &str,
    ) -> Vec<AvailabilityEntry> {
        match self.availability.get(device_id) {
            Some(d) => recent_since(d, |e| &e.received_at, since),
            None => Vec::new(),
        }
    }

    // --- Graph data (temperature + components) ---

    /// Insert pre-parsed graph data into temperature and component stores.
    ///
    /// Callers should parse JSON outside the lock, then pass the extracted values here.
    /// This minimizes lock hold time.
    pub fn insert_temperature_sample(
        &mut self,
        device_id: &str,
        current_temp: Option<f64>,
        set_temp: Option<f64>,
    ) {
        let deque = self.temperatures.entry(device_id.to_string()).or_default();

        let should_insert = match deque.back() {
            Some(last) => {
                let now = now_rfc3339();
                let elapsed = now
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .and_then(|t| {
                        last.received_at
                            .parse::<chrono::DateTime<chrono::Utc>>()
                            .ok()
                            .map(|lt| t.signed_duration_since(lt).num_seconds())
                    })
                    .unwrap_or(i64::MAX);

                let temp_changed = match (current_temp, last.current_temp) {
                    (Some(cur), Some(last)) => (cur - last).abs() >= TEMP_MIN_DELTA,
                    (Some(_), None) => true,
                    _ => false,
                };
                let set_changed = match (set_temp, last.set_temp) {
                    (Some(cur), Some(last)) => cur != last,
                    (Some(_), None) => true,
                    _ => false,
                };

                elapsed >= TEMP_SAMPLE_MIN_INTERVAL_SECS || temp_changed || set_changed
            }
            None => true,
        };

        if should_insert {
            push_capped(
                deque,
                TemperatureSample {
                    current_temp,
                    set_temp,
                    received_at: now_rfc3339(),
                },
                MAX_TEMPERATURE_ENTRIES,
            );
        }
    }

    /// Insert component state-change events. `new_states` maps field names to bool values.
    ///
    /// Only emits events for fields whose state differs from the last recorded value.
    pub fn insert_component_changes(&mut self, device_id: &str, new_states: &[(&str, bool)]) {
        let deque = self
            .component_events
            .entry(device_id.to_string())
            .or_default();
        let now = now_rfc3339();

        for &(field, new_state) in new_states {
            let last_state = deque
                .iter()
                .rev()
                .find(|e| e.component == field)
                .map(|e| e.state != 0);
            if last_state != Some(new_state) {
                push_capped(
                    deque,
                    ComponentEvent {
                        component: field.to_string(),
                        state: new_state as i32,
                        received_at: now.clone(),
                    },
                    MAX_COMPONENT_ENTRIES,
                );
            }
        }
    }

    /// Return temperature samples since `since`, chronological order.
    pub fn get_temperature_history_since(
        &self,
        device_id: &str,
        since: &str,
    ) -> Vec<TemperatureSample> {
        match self.temperatures.get(device_id) {
            Some(d) => recent_since(d, |e| &e.received_at, since),
            None => Vec::new(),
        }
    }

    /// Return component events since `since`, chronological order.
    pub fn get_component_events_since(&self, device_id: &str, since: &str) -> Vec<ComponentEvent> {
        match self.component_events.get(device_id) {
            Some(d) => recent_since(d, |e| &e.received_at, since),
            None => Vec::new(),
        }
    }
}

impl Default for AccessoryConfigData {
    fn default() -> Self {
        AccessoryConfigData {
            pumps: 2,
            lights: 1,
            blower: true,
            mister: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_insert_and_query_logs() {
        let mut store = MemoryStore::new();
        store.insert_log("spa_001", "warn", "Temperature high", 12345);
        store.insert_log("spa_001", "error", "Sensor fault", 12350);

        let logs = store.get_logs("spa_001", 10);
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].level, "error");
        assert_eq!(logs[1].level, "warn");
    }

    #[test]
    fn test_log_ring_buffer_trim() {
        let mut store = MemoryStore::new();
        for i in 0..1100 {
            store.insert_log("spa_001", "info", &format!("msg {}", i), i);
        }
        let logs = store.get_logs("spa_001", 2000);
        assert_eq!(logs.len(), 1000);
        assert!(logs[0].message.contains("1099"));
    }

    #[test]
    fn test_device_isolation() {
        let mut store = MemoryStore::new();
        store.insert_log("spa_001", "info", "msg1", 100);
        store.insert_log("spa_002", "info", "msg2", 200);

        let logs_1 = store.get_logs("spa_001", 10);
        let logs_2 = store.get_logs("spa_002", 10);
        assert_eq!(logs_1.len(), 1);
        assert_eq!(logs_2.len(), 1);
        assert_eq!(logs_1[0].message, "msg1");
        assert_eq!(logs_2[0].message, "msg2");
    }

    #[test]
    fn test_alerts_and_diagnostics_and_sniff() {
        let mut store = MemoryStore::new();
        store.insert_alert("spa_001", r#"{"msg":"overheat"}"#);
        store.insert_diagnostics("spa_001", r#"{"uptime":1234}"#);
        store.insert_sniff_frame("spa_001", r#"{"hex":"aabbcc"}"#);

        let alerts = store.get_alerts("spa_001", 10);
        let diags = store.get_diagnostics("spa_001", 10);
        let sniffs = store.get_sniff_frames("spa_001", 10);

        assert_eq!(alerts.len(), 1);
        assert_eq!(diags.len(), 1);
        assert_eq!(sniffs.len(), 1);
        assert!(alerts[0].payload.contains("overheat"));
        assert!(diags[0].payload.contains("uptime"));
        assert!(sniffs[0].payload.contains("aabbcc"));
    }

    #[test]
    fn test_availability_dedup() {
        let mut store = MemoryStore::new();
        store.insert_availability("spa_001", "online");
        store.insert_availability("spa_001", "online");
        store.insert_availability("spa_001", "offline");
        store.insert_availability("spa_001", "offline");
        store.insert_availability("spa_001", "online");

        let history = store.get_availability_history("spa_001", 100);
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].status, "online");
        assert_eq!(history[1].status, "offline");
        assert_eq!(history[2].status, "online");
    }

    #[test]
    fn test_device_status_upsert() {
        let mut store = MemoryStore::new();
        store.update_device_status("spa_001", "online", Some(42));
        let status = store.get_device_status("spa_001").unwrap();
        assert_eq!(status.status, "online");
        assert_eq!(status.boot_id, Some(42));

        store.update_device_status("spa_001", "stale", None);
        let status = store.get_device_status("spa_001").unwrap();
        assert_eq!(status.status, "stale");
    }

    #[test]
    fn test_list_devices() {
        let mut store = MemoryStore::new();
        store.update_device_status("spa_002", "offline", None);
        store.update_device_status("spa_001", "online", Some(1));

        let devices = store.list_devices();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].device_id, "spa_001");
        assert_eq!(devices[0].status, "online");
        assert_eq!(devices[1].device_id, "spa_002");
        assert_eq!(devices[1].status, "offline");
    }

    #[test]
    fn test_accessory_config_persistence() {
        let mut store = MemoryStore::new();
        assert_eq!(store.get_accessory_config().pumps, 2);

        store.set_accessory_config(AccessoryConfigData {
            pumps: 4,
            lights: 2,
            blower: false,
            mister: true,
        });
        assert_eq!(store.get_accessory_config().pumps, 4);
        assert!(!store.get_accessory_config().blower);
    }

    #[test]
    fn test_clear_operations() {
        let mut store = MemoryStore::new();
        store.insert_log("spa_001", "info", "msg", 1);
        store.insert_alert("spa_001", r#"{"a":1}"#);
        store.insert_diagnostics("spa_001", r#"{"d":1}"#);
        store.insert_sniff_frame("spa_001", r#"{"s":1}"#);

        store.clear_logs("spa_001");
        store.clear_alerts("spa_001");
        store.clear_diagnostics("spa_001");
        store.clear_sniff_frames("spa_001");

        assert!(store.get_logs("spa_001", 10).is_empty());
        assert!(store.get_alerts("spa_001", 10).is_empty());
        assert!(store.get_diagnostics("spa_001", 10).is_empty());
        assert!(store.get_sniff_frames("spa_001", 10).is_empty());
    }

    #[test]
    fn test_temperature_dedup_by_interval() {
        let mut store = MemoryStore::new();
        store.insert_temperature_sample("spa_001", Some(100.0), Some(104.0));
        let since = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let samples = store.get_temperature_history_since("spa_001", &since);
        assert_eq!(samples.len(), 1);

        // Same temp too fast should NOT insert
        store.insert_temperature_sample("spa_001", Some(100.0), Some(104.0));
        let samples2 = store.get_temperature_history_since("spa_001", &since);
        assert_eq!(
            samples2.len(),
            1,
            "Should not insert duplicate within interval"
        );
    }

    #[test]
    fn test_temperature_insert_on_significant_change() {
        let mut store = MemoryStore::new();
        store.insert_temperature_sample("spa_001", Some(100.0), Some(104.0));
        store.insert_temperature_sample("spa_001", Some(100.5), Some(104.0));
        let since = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let samples = store.get_temperature_history_since("spa_001", &since);
        assert_eq!(samples.len(), 2, "Should insert on significant temp change");
    }

    #[test]
    fn test_temperature_no_insert_on_small_change() {
        let mut store = MemoryStore::new();
        store.insert_temperature_sample("spa_001", Some(100.0), Some(104.0));
        store.insert_temperature_sample("spa_001", Some(100.2), Some(104.0));
        let since = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let samples = store.get_temperature_history_since("spa_001", &since);
        assert_eq!(samples.len(), 1, "Should not insert on small temp change");
    }

    #[test]
    fn test_component_events_only_on_change() {
        let mut store = MemoryStore::new();
        let all_off: Vec<(&str, bool)> = COMPONENT_FIELDS.iter().map(|&f| (f, false)).collect();
        store.insert_component_changes("spa_001", &all_off);
        let since = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let events = store.get_component_events_since("spa_001", &since);
        assert_eq!(
            events.len(),
            14,
            "First insert records initial states for all 14 components"
        );

        store.insert_component_changes("spa_001", &all_off);
        let events2 = store.get_component_events_since("spa_001", &since);
        assert_eq!(events2.len(), 14, "No change should not add events");

        let mut heating_on = all_off.clone();
        heating_on[0] = ("is_heating", true);
        store.insert_component_changes("spa_001", &heating_on);
        let events3 = store.get_component_events_since("spa_001", &since);
        assert_eq!(events3.len(), 15, "Should add event for heating change");
        assert_eq!(events3[14].component, "is_heating");
        assert_eq!(events3[14].state, 1);
    }

    #[test]
    fn test_null_temp_always_inserts() {
        let mut store = MemoryStore::new();
        store.insert_temperature_sample("spa_001", None, Some(100.0));
        store.insert_temperature_sample("spa_001", Some(100.0), Some(100.0));
        let since = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let samples = store.get_temperature_history_since("spa_001", &since);
        assert_eq!(samples.len(), 2, "Null->value transition should insert");
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("launa_test_save_load");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.json");

        let mut store = MemoryStore::new();
        store.insert_log("spa_001", "info", "hello", 100);
        store.insert_alert("spa_001", r#"{"msg":"test"}"#);
        store.insert_availability("spa_001", "online");
        store.update_device_status("spa_001", "online", Some(7));
        store.insert_temperature_sample("spa_001", Some(100.0), Some(104.0));
        store.set_accessory_config(AccessoryConfigData {
            pumps: 3,
            lights: 2,
            blower: false,
            mister: true,
        });
        store.save(&path);

        let loaded = MemoryStore::load(&path);
        assert_eq!(loaded.get_logs("spa_001", 10).len(), 1);
        assert_eq!(loaded.get_alerts("spa_001", 10).len(), 1);
        assert_eq!(loaded.get_availability_history("spa_001", 10).len(), 1);
        let since = (Utc::now() - Duration::hours(1)).to_rfc3339();
        assert_eq!(
            loaded
                .get_temperature_history_since("spa_001", &since)
                .len(),
            1
        );
        let status = loaded.get_device_status("spa_001").unwrap();
        assert_eq!(status.status, "online");
        assert_eq!(status.boot_id, Some(7));
        let cfg = loaded.get_accessory_config();
        assert_eq!(cfg.pumps, 3);
        assert_eq!(cfg.lights, 2);
        assert!(!cfg.blower);
        assert!(cfg.mister);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
