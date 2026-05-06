//! Flash the RS-485 debugger firmware to an ESP32.
//!
//! Usage: cargo xtask rs485-debugger-flash [--port <device> | --port-index <N>] [--monitor]
//!
//! Reads WiFi/MQTT config from launa.toml and passes it as build-time env vars
//! to the firmware so it can connect to WiFi and publish MQTT status.

use anyhow::Context;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let config = crate::config::load().context(
        "RS-485 debugger requires launa.toml for WiFi/MQTT config.\n\
         Copy launa.example.toml to launa.toml and fill in your values.",
    )?;
    crate::util::flash_app(args, "RS-485 debugger", "app-rs485-debugger", Some(&config))
}
