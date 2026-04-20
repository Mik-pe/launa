mod config;
mod config_flash;
mod flash;
mod flash_monitor;
mod listen;
mod monitor;
mod ota_flash;
mod ota_serve;
mod provision;
mod self_test;
mod sniff_decode;
mod spa_sim;
mod util;

use anyhow::bail;
use std::env;

fn usage() {
    eprintln!("Usage: cargo xtask <command> [args...]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  flash [--feature <name>] [--port <COMx>]         Flash firmware via USB");
    eprintln!("  monitor [--port <COMx>] [--duration <secs>]       Read serial output");
    eprintln!("  flash-monitor [--feature <name>] [--port <COMx>]  Flash + monitor");
    eprintln!(
        "  sniff-decode [--host <host>] [--port <1883>]      Decode sniffer frames from MQTT"
    );
    eprintln!("  spa-sim [--port <COMx>] [--duration <secs>]       Simulate spa over RS-485");
    eprintln!("  ota-serve --firmware <path> [--port <8080>]       Serve firmware over HTTP");
    eprintln!(
        "  ota-flash [--feature <name>] [--device-id <id>]   Build and flash remotely over WiFi"
    );
    eprintln!("  self-test [--port <COMx>]                         Run hardware self-test");
    eprintln!("  config-flash [--port <COMx>]                      Write config to ESP32 NVS");
    eprintln!(
        "  provision [--port <COMx>] [--no-confirm]          Burn AES key to ESP32 eFuse BLOCK3"
    );
    eprintln!(
        "  listen [--host <host>] [--port <1883>] [-t <topic>]  Subscribe to MQTT topics"
    );
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

    // --- flash.rs argument parsing tests ---
    #[test]
    fn test_flash_feature_as_last_arg_returns_error() {
        let args = vec!["--feature".to_string()];
        let result = flash::run(&args);
        assert!(
            run_returns_error_containing(result, "--feature requires a value"),
            "Should error about --feature requiring a value"
        );
    }

    #[test]
    fn test_flash_port_as_last_arg_returns_error() {
        let args = vec!["--port".to_string()];
        let result = flash::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
    }

    // --- monitor.rs argument parsing tests ---
    #[test]
    fn test_monitor_port_as_last_arg_returns_error() {
        let args = vec!["--port".to_string()];
        let result = monitor::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
    }

    #[test]
    fn test_monitor_duration_as_last_arg_returns_error() {
        let args = vec!["--duration".to_string()];
        let result = monitor::run(&args);
        assert!(
            run_returns_error_containing(result, "--duration requires a value"),
            "Should error about --duration requiring a value"
        );
    }

    // --- self_test.rs argument parsing tests ---
    #[test]
    fn test_self_test_port_as_last_arg_returns_error() {
        let args = vec!["--port".to_string()];
        let result = self_test::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
    }

    // --- ota_serve.rs argument parsing tests ---
    #[test]
    fn test_ota_serve_firmware_as_last_arg_returns_error() {
        let args = vec!["--firmware".to_string()];
        let result = ota_serve::run(&args);
        assert!(
            run_returns_error_containing(result, "--firmware requires a value"),
            "Should error about --firmware requiring a value"
        );
    }

    #[test]
    fn test_ota_serve_port_as_last_arg_returns_error() {
        let args = vec![
            "--firmware".to_string(),
            "/dev/null".to_string(),
            "--port".to_string(),
        ];
        let result = ota_serve::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
    }

    // --- sniff_decode.rs argument parsing tests ---
    #[test]
    fn test_sniff_decode_host_as_last_arg_returns_error() {
        let args = vec!["--host".to_string()];
        let result = sniff_decode::run(&args);
        assert!(
            run_returns_error_containing(result, "--host requires a value"),
            "Should error about --host requiring a value"
        );
    }

    #[test]
    fn test_sniff_decode_port_as_last_arg_returns_error() {
        let args = vec!["--port".to_string()];
        let result = sniff_decode::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
    }

    #[test]
    fn test_sniff_decode_output_as_last_arg_returns_error() {
        let args = vec!["--output".to_string()];
        let result = sniff_decode::run(&args);
        assert!(
            run_returns_error_containing(result, "--output requires a value"),
            "Should error about --output requiring a value"
        );
    }

    // --- spa_sim.rs argument parsing tests ---
    #[test]
    fn test_spa_sim_port_as_last_arg_returns_error() {
        let args = vec!["--port".to_string()];
        let result = spa_sim::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
    }

    #[test]
    fn test_spa_sim_duration_as_last_arg_returns_error() {
        let args = vec!["--duration".to_string()];
        let result = spa_sim::run(&args);
        assert!(
            run_returns_error_containing(result, "--duration requires a value"),
            "Should error about --duration requiring a value"
        );
    }

    // --- config_flash.rs argument parsing tests ---
    #[test]
    fn test_config_flash_port_as_last_arg_returns_error() {
        let args = vec!["--port".to_string()];
        let result = config_flash::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
    }

    // --- ota_flash.rs argument parsing tests ---
    #[test]
    fn test_ota_flash_feature_as_last_arg_returns_error() {
        let args = vec!["--feature".to_string()];
        let result = ota_flash::run(&args);
        assert!(
            run_returns_error_containing(result, "--feature requires a value"),
            "Should error about --feature requiring a value"
        );
    }

    #[test]
    fn test_ota_flash_device_id_as_last_arg_returns_error() {
        let args = vec!["--device-id".to_string()];
        let result = ota_flash::run(&args);
        assert!(
            run_returns_error_containing(result, "--device-id requires a value"),
            "Should error about --device-id requiring a value"
        );
    }

    // --- provision.rs argument parsing tests ---
    #[test]
    fn test_provision_port_as_last_arg_returns_error() {
        let args = vec!["--port".to_string()];
        let result = provision::run(&args);
        assert!(
            run_returns_error_containing(result, "--port requires a value"),
            "Should error about --port requiring a value"
        );
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
