mod config;
mod config_flash;
mod flash;
mod listen;
mod monitor;
mod ota_flash;
mod ota_serve;
mod provision;
mod rs485_debugger_flash;
mod sniff_decode;
mod sniffer_flash;
mod spa_emulator_flash;
mod spa_sim;
mod util;

use anyhow::bail;
use std::env;

fn usage() {
    eprintln!("Usage: cargo xtask <command> [args...]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  list-ports                                         List detected ESP serial ports with USB serial numbers");
    eprintln!("  flash [--port <device> | --serial <usb-serial> | --port-index <N>] [--feature <name>] [--monitor]");
    eprintln!("                                                    Flash firmware via USB");
    eprintln!("  monitor [--port <device> | --serial <usb-serial> | --port-index <N>] [--duration <secs>]");
    eprintln!("                                                    Read serial output");
    eprintln!(
        "  sniff-decode [--host <host>] [--port <1883>]      Decode sniffer frames from MQTT"
    );
    eprintln!("  spa-sim [--port <device> | --serial <usb-serial> | --port-index <N>] [--duration <secs>]");
    eprintln!("                                                    Simulate spa over RS-485");
    eprintln!("  rs485-debugger-flash [--port <device> | --serial <usb-serial> | --port-index <N>] [--monitor]");
    eprintln!("                                                    Flash RS-485 debugger firmware");
    eprintln!(
        "  sniffer-flash [--port <device> | --serial <usb-serial> | --port-index <N>] [--monitor]"
    );
    eprintln!("                                                    Flash sniffer firmware");
    eprintln!("  spa-emulator-flash [--port <device> | --serial <usb-serial> | --port-index <N>] [--monitor]");
    eprintln!("                                                    Flash spa emulator firmware");
    eprintln!("  ota-serve --firmware <path> [--port <8080>]       Serve firmware over HTTP");
    eprintln!(
        "  ota-flash [--feature <name>] [--device-id <id>]   Build and flash remotely over WiFi"
    );
    eprintln!("  config-flash [--port <device> | --serial <usb-serial> | --port-index <N>]");
    eprintln!("                                                    Write config to ESP32 NVS");
    eprintln!(
        "  provision [--port <device> | --serial <usb-serial> | --port-index <N>] [--no-confirm]"
    );
    eprintln!(
        "                                                    Burn AES key to ESP32 eFuse BLOCK3"
    );
    eprintln!("  listen [--host <host>] [--port <1883>] [-t <topic>]  Subscribe to MQTT topics");
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
        "list-ports" | "list_ports" => crate::util::list_ports(),
        "flash" => flash::run(sub_args),
        "monitor" => monitor::run(sub_args),
        "sniff-decode" | "sniff_decode" => sniff_decode::run(sub_args),
        "spa-sim" | "spa_sim" => spa_sim::run(sub_args),
        "rs485-debugger-flash" | "rs485_debugger_flash" => rs485_debugger_flash::run(sub_args),
        "sniffer-flash" | "sniffer_flash" => sniffer_flash::run(sub_args),
        "spa-emulator-flash" | "spa_emulator_flash" | "spa-emu-flash" | "spa_emu_flash" => {
            spa_emulator_flash::run(sub_args)
        }
        "ota-serve" | "ota_serve" => ota_serve::run(sub_args),
        "ota-flash" | "ota_flash" => ota_flash::run(sub_args),
        "config-flash" | "config_flash" => config_flash::run(sub_args),
        "provision" => provision::run(sub_args),
        "listen" => listen::run(sub_args),
        other => {
            usage();
            bail!("Unknown command: {}", other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: call a module's run() and return whether it errored with expected substring
    fn run_returns_error_containing(result: anyhow::Result<()>, expected: &str) -> bool {
        match result {
            Err(e) => e.to_string().contains(expected),
            Ok(()) => false,
        }
    }

    // --- Parameterized "flag as last arg" tests ---
    // All commands use Args::value() which produces "<flag> requires a value".
    // A single parameterized test covers all cases.
    #[test]
    fn flag_as_last_arg_requires_value() {
        type Runner = fn(&[String]) -> anyhow::Result<()>;

        let cases: &[(&[&str], &str, Runner)] = &[
            // flash
            (&["--feature"], "--feature", flash::run as Runner),
            (&["--port"], "--port", flash::run as Runner),
            // monitor
            (&["--port"], "--port", monitor::run as Runner),
            (&["--duration"], "--duration", monitor::run as Runner),
            // ota_serve
            (&["--firmware"], "--firmware", ota_serve::run as Runner),
            (
                &["--firmware", "/dev/null", "--port"],
                "--port",
                ota_serve::run as Runner,
            ),
            // sniff_decode
            (&["--host"], "--host", sniff_decode::run as Runner),
            (&["--port"], "--port", sniff_decode::run as Runner),
            (&["--output"], "--output", sniff_decode::run as Runner),
            // spa_sim
            (&["--port"], "--port", spa_sim::run as Runner),
            (&["--duration"], "--duration", spa_sim::run as Runner),
            // config_flash
            (&["--port"], "--port", config_flash::run as Runner),
            // ota_flash
            (&["--feature"], "--feature", ota_flash::run as Runner),
            (&["--device-id"], "--device-id", ota_flash::run as Runner),
            // provision
            (&["--port"], "--port", provision::run as Runner),
        ];

        for (args, flag, runner) in cases {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let result = runner(&args);
            assert!(
                result.is_err(),
                "expected error for {flag} as last arg with args {:?}",
                args
            );
            let err = result.unwrap_err();
            assert!(
                err.to_string()
                    .contains(&format!("{flag} requires a value")),
                "expected '{flag} requires a value' in error for args {:?}, got: {err}",
                args
            );
        }
    }

    // --- Empty args handling ---
    // These tests verify empty args don't cause panics (no index out of bounds).
    // Most modules attempt external connections with empty args, so we only test
    // the ones that will fail fast without hanging.
    #[test]
    fn test_empty_args_ota_serve_no_panic() {
        let empty: Vec<String> = vec![];
        let result = ota_serve::run(&empty);
        // Should error about missing --firmware, not panic
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_args_spa_sim_no_panic() {
        // Use a non-existent port to avoid opening a real serial device
        // when the ESP32 is plugged in (e.g. COM5). This keeps the test
        // fast (< 1s) and verifies spa_sim doesn't panic on error.
        let args = vec!["--port".to_string(), "COM_NONEXISTENT_9999".to_string()];
        let result = spa_sim::run(&args);
        assert!(result.is_err(), "Should fail on non-existent port");
    }

    // --- Unknown argument handling ---
    #[test]
    fn test_unknown_argument_returns_error() {
        let args = vec!["--bogus".to_string()];
        let result = flash::run(&args);
        assert!(
            run_returns_error_containing(result, "Unknown argument"),
            "Should error about unknown argument"
        );
    }
}
