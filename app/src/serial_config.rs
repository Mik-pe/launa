//! Serial config reception over USB (UART0).
//!
//! Non-blocking, async state machine that checks for incoming config data
//! on every call to `poll()`. When a complete config is received, returns
//! the parsed `AppConfig` so the caller can save it to NVS and reboot.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use log::{info, warn};

use crate::{config, uart_raw};

const MAX_LINE_LEN: usize = 256;

/// Non-blocking serial config receiver.
///
/// Call `poll()` periodically (e.g., once per main loop tick). It drains any
/// pending bytes from the UART0 RX FIFO and advances the protocol state
/// machine. Returns `Some(AppConfig)` when a complete config has been
/// received.
pub(crate) struct SerialConfigReceiver {
    line_buf: Vec<u8>,
    started: bool,
    kv_pairs: Vec<(String, String)>,
}

impl SerialConfigReceiver {
    pub(crate) const fn new() -> Self {
        SerialConfigReceiver {
            line_buf: Vec::new(),
            started: false,
            kv_pairs: Vec::new(),
        }
    }

    /// Non-blocking poll: drain UART0 RX FIFO, advance state machine.
    /// Returns `Some(AppConfig)` when CONFIG_START..CONFIG_END received.
    /// Returns `None` otherwise (no data yet, or partial data).
    pub(crate) fn poll(&mut self) -> Option<config::AppConfig> {
        // Drain all available bytes from UART0 RX FIFO
        let mut rx_byte = uart_raw::read_byte();
        while let Some(byte) = rx_byte {
            if byte == b'\n' {
                let line = {
                    let raw = core::str::from_utf8(&self.line_buf).unwrap_or("");
                    let trimmed = raw.trim_start_matches('\r').trim_end_matches('\r');
                    String::from(trimmed)
                };
                self.line_buf.clear();

                if !self.started {
                    if line == "CONFIG_START" {
                        self.started = true;
                        self.kv_pairs.clear();
                        info!("Serial config reception started");
                    }
                } else if line == "CONFIG_END" {
                    return self.parse_and_respond();
                } else if !line.is_empty() {
                    if let Some(eq_pos) = line.find('=') {
                        let key = &line[..eq_pos];
                        let value = &line[eq_pos + 1..];
                        self.kv_pairs.push((String::from(key), String::from(value)));
                    }
                }
            } else if byte != b'\r' && self.line_buf.len() < MAX_LINE_LEN {
                self.line_buf.push(byte);
            }

            rx_byte = uart_raw::read_byte();
        }

        None
    }

    /// Parse accumulated key-value pairs into AppConfig and send CONFIG_OK.
    fn parse_and_respond(&mut self) -> Option<config::AppConfig> {
        let mut app_config = config::AppConfig::default();

        for (key, value) in &self.kv_pairs {
            match key.as_str() {
                "wifi.ssid" => app_config.wifi_ssid = value.clone(),
                "wifi.password" => app_config.wifi_password = value.clone(),
                "mqtt.host" => app_config.mqtt_host = value.clone(),
                "mqtt.port" => match value.parse::<u16>() {
                    Ok(p) => {
                        app_config.mqtt_port = p;
                    }
                    Err(_) => {
                        warn!("Invalid port: {}", value);
                        uart_raw::write_bytes(
                            format!("CONFIG_ERROR:invalid_port={}\n", value).as_bytes(),
                        );
                        uart_raw::flush();
                        self.reset();
                        return None;
                    }
                },
                "mqtt.user" => app_config.mqtt_user = value.clone(),
                "mqtt.password" => app_config.mqtt_password = value.clone(),
                "device.id" => app_config.device_id = value.clone(),
                other => {
                    warn!("Unknown config key: {}", other);
                }
            }
        }

        info!(
            "Parsed config: ssid=<{} chars> mqtt={}:{} device={}",
            app_config.wifi_ssid.len(),
            app_config.mqtt_host,
            app_config.mqtt_port,
            app_config.device_id
        );

        uart_raw::write_bytes(b"CONFIG_OK\n");
        uart_raw::flush();
        self.reset();
        Some(app_config)
    }

    fn reset(&mut self) {
        self.started = false;
        self.kv_pairs.clear();
        self.line_buf.clear();
    }
}
