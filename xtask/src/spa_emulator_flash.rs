//! Flash the spa-emulator firmware to an ESP32.
//!
//! Usage: cargo xtask spa-emulator-flash [--port <device> | --port-index <N>] [--monitor]

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let config = match crate::config::load_without_serial_port_check() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("Warning: could not load config for env vars: {}", e);
            None
        }
    };
    crate::util::flash_app(args, "Spa-emulator", "app-spa-emulator", config.as_ref())
}
