use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use reqwest::Client;
use serde_json::json;
use tracing::{error, info};

use crate::memory::MemoryStore;

pub struct Notifier {
    mem: Arc<RwLock<MemoryStore>>,
    http: Client,
    /// Device IDs that have already triggered an offline notification
    /// (so we don't spam every check interval).
    notified: std::sync::Mutex<HashSet<String>>,
}

impl Notifier {
    pub fn new(mem: Arc<RwLock<MemoryStore>>) -> Self {
        Self {
            mem,
            http: Client::new(),
            notified: std::sync::Mutex::new(HashSet::new()),
        }
    }

    /// Run one check cycle. Call this periodically (e.g. every 5 minutes).
    pub async fn check(&self) {
        let (webhook_url, threshold_hours, offline_devices) = {
            let mem = self.mem.read().unwrap();
            let cfg = mem.get_notification_config();
            if cfg.discord_webhook_url.is_empty() {
                return;
            }
            let threshold_secs = (cfg.offline_threshold_hours as i64) * 3600;
            let devices = mem.get_long_offline_devices(threshold_secs);
            (
                cfg.discord_webhook_url.clone(),
                cfg.offline_threshold_hours,
                devices,
            )
        };

        // --- Send offline alerts for newly-offline devices ---
        for (device_id, _since, elapsed_secs) in &offline_devices {
            let already_notified = self.notified.lock().unwrap().contains(device_id);
            if already_notified {
                continue;
            }

            let hours = elapsed_secs / 3600;
            let mins = (elapsed_secs % 3600) / 60;
            let duration_str = if hours > 0 {
                format!("{}h {}m", hours, mins)
            } else {
                format!("{}m", mins)
            };

            info!(
                "Device '{}' offline for {} (threshold: {}h), sending Discord notification",
                device_id, duration_str, threshold_hours
            );

            match self
                .send_offline_alert(&webhook_url, device_id, &duration_str)
                .await
            {
                Ok(()) => {
                    self.notified.lock().unwrap().insert(device_id.clone());
                }
                Err(e) => {
                    error!(
                        "Failed to send Discord notification for '{}': {}",
                        device_id, e
                    );
                    // Don't mark as notified so we retry next cycle
                }
            }
        }

        // --- Send "back online" alerts for devices that recovered ---
        let current_offline: HashSet<String> = offline_devices
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect();
        let recovered: Vec<String> = {
            let notified = self.notified.lock().unwrap();
            notified
                .iter()
                .filter(|id| !current_offline.contains(*id))
                .cloned()
                .collect()
        };

        for device_id in &recovered {
            info!(
                "Device '{}' is back online, sending Discord notification",
                device_id
            );
            if let Err(e) = self.send_back_online(&webhook_url, device_id).await {
                error!(
                    "Failed to send Discord back-online notification for '{}': {}",
                    device_id, e
                );
            }
            self.notified.lock().unwrap().remove(device_id);
        }
    }

    async fn send_offline_alert(
        &self,
        webhook_url: &str,
        device_id: &str,
        duration: &str,
    ) -> Result<(), String> {
        let payload = json!({
            "content": format!("**{}** has been offline for {}", device_id, duration),
            "embeds": [{
                "title": "Spa Offline Alert",
                "color": 15158332,
                "fields": [
                    {"name": "Device", "value": device_id, "inline": true},
                    {"name": "Offline for", "value": duration, "inline": true}
                ]
            }]
        });

        self.send_webhook(webhook_url, &payload).await
    }

    async fn send_back_online(&self, webhook_url: &str, device_id: &str) -> Result<(), String> {
        let payload = json!({
            "content": format!("**{}** is back online", device_id),
            "embeds": [{
                "title": "Spa Back Online",
                "color": 3066993,
                "fields": [
                    {"name": "Device", "value": device_id, "inline": true}
                ]
            }]
        });

        self.send_webhook(webhook_url, &payload).await
    }

    async fn send_webhook(
        &self,
        webhook_url: &str,
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        let resp = self
            .http
            .post(webhook_url)
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Discord returned {}: {}", status, body));
        }

        Ok(())
    }
}
