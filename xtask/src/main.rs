mod config;
mod config_flash;
mod flash;
mod flash_monitor;
mod monitor;
mod ota_flash;
mod ota_serve;
mod self_test;
mod sniff_decode;
mod spa_sim;

use anyhow::bail;
use std::env;

fn usage() {
    eprintln!("Usage: cargo xtask <command> [args...]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  flash [--feature <name>] [--port <COMx>]         Flash firmware via USB");
    eprintln!("  monitor [--port <COMx>] [--duration <secs>]       Read serial output");
    eprintln!("  flash-monitor [--feature <name>] [--port <COMx>]  Flash + monitor");
    eprintln!("  sniff-decode [--host <host>] [--port <1883>]      Decode sniffer frames from MQTT");
    eprintln!("  spa-sim [--port <COMx>] [--duration <secs>]       Simulate spa over RS-485");
    eprintln!("  ota-serve --firmware <path> [--port <8080>]       Serve firmware over HTTP");
    eprintln!("  ota-flash [--feature <name>] [--device-id <id>]   Build and flash remotely over WiFi");
    eprintln!("  self-test [--port <COMx>]                         Run hardware self-test");
    eprintln!("  config-flash [--port <COMx>]                      Write config to ESP32 NVS");
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
        bail!("No command specified");
    }

    let command = &args[0];
    let sub_args = &args[1..];

    match command.as_str() {
        "flash" => flash::run(sub_args),
        "monitor" => monitor::run(sub_args),
        "flash-monitor" | "flash_monitor" => flash_monitor::run(sub_args),
        "sniff-decode" | "sniff_decode" => sniff_decode::run(sub_args),
        "spa-sim" | "spa_sim" => spa_sim::run(sub_args),
        "ota-serve" | "ota_serve" => ota_serve::run(sub_args),
        "ota-flash" | "ota_flash" => ota_flash::run(sub_args),
        "self-test" | "self_test" => self_test::run(sub_args),
        "config-flash" | "config_flash" => config_flash::run(sub_args),
        other => {
            usage();
            bail!("Unknown command: {}", other);
        }
    }
}
