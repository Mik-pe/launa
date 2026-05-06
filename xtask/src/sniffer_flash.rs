//! Flash the sniffer firmware to an ESP32.
//!
//! Usage: cargo xtask sniffer-flash [--port <device> | --port-index <N>] [--monitor]

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let config = crate::config::load().ok();
    crate::util::flash_app(args, "Sniffer", "app-sniffer", config.as_ref())
}
