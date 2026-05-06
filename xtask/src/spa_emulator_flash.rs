//! Flash the spa-emulator firmware to an ESP32.
//!
//! Usage: cargo xtask spa-emulator-flash [--port <device> | --port-index <N>] [--monitor]

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let config = crate::config::load_without_serial_port_check()
        .map_err(|e| {
            eprintln!("Warning: could not load config for env vars: {}", e);
            e
        })
        .ok();
    crate::util::flash_app(args, "Spa-emulator", "app-spa-emulator", config.as_ref())
}
