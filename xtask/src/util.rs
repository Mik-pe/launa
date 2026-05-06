//! Shared utilities for xtask modules.

use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns the project root directory (parent of xtask/).
///
/// Uses `CARGO_MANIFEST_DIR` which points to `xtask/` at compile time,
/// so we take its parent to reach the workspace root.
pub fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must be inside project root")
        .to_path_buf()
}

/// Install a Ctrl+C handler that calls the given closure on SIGINT.
///
/// Used by long-running commands (monitor, spa-sim, ota-serve, sniff-decode)
/// to gracefully shut down on Ctrl+C.
pub fn ctrlc_handler<F: Fn() + Send + 'static>(handler: F) {
    let _ = ctrlc::set_handler(handler);
}

/// Simple CLI argument parser that eliminates the duplicated while-loop pattern.
///
/// Tracks position internally so callers can declaratively consume flags and values.
pub struct Args<'a> {
    args: &'a [String],
    i: usize,
}

impl<'a> Args<'a> {
    pub fn new(args: &'a [String]) -> Self {
        Self { args, i: 0 }
    }

    /// Returns `true` if there are more arguments to process.
    pub fn has_more(&self) -> bool {
        self.i < self.args.len()
    }

    /// Peek at the current argument without consuming it.
    pub fn peek(&self) -> Option<&'a str> {
        self.args.get(self.i).map(|s| s.as_str())
    }

    /// Require a value for the given flag and advance past both flag and value.
    ///
    /// Call this when you've already matched a flag name via `peek()`.
    /// Produces error messages like `"--port requires a value"`.
    pub fn value(&mut self, flag: &str) -> anyhow::Result<&'a str> {
        self.i += 1;
        if self.i >= self.args.len() {
            bail!("{} requires a value", flag);
        }
        let val = &self.args[self.i];
        self.i += 1;
        Ok(val)
    }

    /// Advance past the current argument (call after `peek()` when you've
    /// handled a valueless flag like `--respond` or `--verbose`).
    pub fn skip(&mut self) {
        self.i += 1;
    }

    /// Convenience: if the current arg matches `flag`, consume both and parse as `T`.
    pub fn optional_parsed<T: std::str::FromStr>(&mut self, flag: &str) -> anyhow::Result<Option<T>>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        if self.peek() == Some(flag) {
            let raw = self.value(flag)?;
            let parsed = raw
                .parse::<T>()
                .with_context(|| format!("Invalid value for {}", flag))?;
            Ok(Some(parsed))
        } else {
            Ok(None)
        }
    }

    /// Return the current argument as an unknown flag error.
    pub fn unknown_arg(&self) -> anyhow::Error {
        anyhow::anyhow!(
            "Unknown argument: {}",
            self.args.get(self.i).map_or("?", |s| s.as_str())
        )
    }
}

/// A detected USB serial port with optional metadata from the USB device.
pub struct DetectedPort {
    pub port_name: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

impl DetectedPort {
    /// Human-readable description for the port picker menu.
    fn description(&self) -> String {
        match (&self.manufacturer, &self.product, &self.serial_number) {
            (Some(mfr), Some(prod), Some(sn)) => format!("{} {} (s/n: {})", mfr, prod, sn),
            (Some(mfr), Some(prod), None) => format!("{} {}", mfr, prod),
            (Some(mfr), None, Some(sn)) => format!("{} (s/n: {})", mfr, sn),
            (Some(mfr), None, None) => mfr.clone(),
            (None, Some(prod), Some(sn)) => format!("{} (s/n: {})", prod, sn),
            (None, Some(prod), None) => prod.clone(),
            (None, None, Some(sn)) => format!("USB device (s/n: {})", sn),
            _ => {
                // Fall back to VID:PID if available
                match (self.vid, self.pid) {
                    (Some(v), Some(p)) => format!("USB device ({:04x}:{:04x})", v, p),
                    _ => "USB device".to_string(),
                }
            }
        }
    }
}

/// Detect all USB serial ports that look like ESP32 adapters.
///
/// Uses `serialport::available_ports()` which provides USB metadata
/// (manufacturer, product name, serial number) on macOS and Linux.
pub fn detect_esp_ports() -> Vec<DetectedPort> {
    let ports = match serialport::available_ports() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut detected: Vec<DetectedPort> = ports
        .into_iter()
        .filter(|p| is_likely_esp_port(&p.port_name))
        .filter_map(|p| {
            let (manufacturer, product, serial_number, vid, pid) = match &p.port_type {
                serialport::SerialPortType::UsbPort(usb) => (
                    usb.manufacturer.clone(),
                    usb.product.clone(),
                    usb.serial_number.clone(),
                    Some(usb.vid),
                    Some(usb.pid),
                ),
                _ => (None, None, None, None, None),
            };
            Some(DetectedPort {
                port_name: p.port_name,
                manufacturer,
                product,
                serial_number,
                vid,
                pid,
            })
        })
        .collect();

    // Sort by port name for deterministic ordering
    detected.sort_by(|a, b| a.port_name.cmp(&b.port_name));
    detected
}

/// Auto-detect a single ESP32 USB serial port.
///
/// Returns `None` if zero or multiple ports are found (use `resolve_port`
/// for the full interactive flow that handles multiple devices).
#[allow(dead_code)]
pub fn auto_detect_serial_port() -> Option<String> {
    let ports = detect_esp_ports();
    if ports.len() == 1 {
        return Some(ports[0].port_name.clone());
    }
    None
}

/// Check if a port name looks like a USB serial adapter (ESP32, CH340, CP210x, etc.).
fn is_likely_esp_port(name: &str) -> bool {
    let path = Path::new(name);
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy())
        .unwrap_or_default();

    // macOS patterns
    if file_name.starts_with("cu.usbserial")
        || file_name.starts_with("cu.usbmodem")
        || file_name.starts_with("cu.wchusbserial")
    {
        return true;
    }

    // Linux patterns
    if file_name.starts_with("ttyUSB") || file_name.starts_with("ttyACM") {
        return true;
    }

    // Windows COM ports (COM3 and above are typically USB serial)
    if let Ok(n) = file_name.trim_start_matches("COM").parse::<u32>() {
        return n >= 3;
    }

    false
}

/// Build an error message listing all detected ports with 1-based indices.
fn format_port_list(ports: &[DetectedPort]) -> String {
    let mut lines = Vec::new();
    lines.push("Multiple serial ports detected:".to_string());
    lines.push(String::new());
    for (i, p) in ports.iter().enumerate() {
        let sn_info = p
            .serial_number
            .as_ref()
            .map(|sn| format!(" (s/n: {})", sn))
            .unwrap_or_default();
        lines.push(format!(
            "  [{}] {}  {}{}",
            i + 1,
            p.port_name,
            p.description(),
            sn_info
        ));
    }
    lines.push(String::new());
    lines.push(
        "Use --port-index <N>, --port <device>, or --serial <usb-serial> to select one."
            .to_string(),
    );
    lines.join("\n")
}

/// Resolve a serial port by USB serial number.
///
/// Scans all detected ESP ports and returns the one whose USB serial number
/// matches (case-insensitive, partial match from the end of the serial number).
/// This is stable across replugs — the USB serial number doesn't change even
/// when macOS assigns a different `/dev/cu.usbserial-*` path.
pub fn resolve_port_by_serial(serial: &str) -> anyhow::Result<String> {
    let ports = detect_esp_ports();
    let serial_lower = serial.to_lowercase();

    let matches: Vec<&DetectedPort> = ports
        .iter()
        .filter(|p| {
            p.serial_number
                .as_ref()
                .map_or(false, |sn| sn.to_lowercase().ends_with(&serial_lower))
        })
        .collect();

    match matches.len() {
        0 => {
            let available: Vec<String> = ports
                .iter()
                .filter_map(|p| {
                    p.serial_number
                        .as_ref()
                        .map(|sn| format!("  {} (s/n: {})", p.port_name, sn))
                })
                .collect();
            if available.is_empty() {
                bail!(
                    "No ESP serial port found with USB serial number matching '{}'.\n\
                     No ports with serial numbers detected at all.",
                    serial
                );
            } else {
                bail!(
                    "No ESP serial port found with USB serial number matching '{}'.\n\
                     Available:\n{}",
                    serial,
                    available.join("\n")
                );
            }
        }
        1 => {
            let p = matches[0];
            println!(
                "Matched USB serial '{}': {} (s/n: {})",
                serial,
                p.port_name,
                p.serial_number.as_deref().unwrap_or("?")
            );
            Ok(p.port_name.clone())
        }
        _ => {
            let list: Vec<String> = matches
                .iter()
                .map(|p| {
                    format!(
                        "  {} (s/n: {})",
                        p.port_name,
                        p.serial_number.as_deref().unwrap_or("?")
                    )
                })
                .collect();
            bail!(
                "Multiple ports match USB serial '{}':\n{}\n\
                 Use a more specific serial number or --port to disambiguate.",
                serial,
                list.join("\n")
            );
        }
    }
}

/// Resolve the serial port using the "CLI arg → serial → config → auto-detect → error" pattern.
///
/// Priority:
/// 1. `--port <device>` — explicit device path
/// 2. `--serial <usb-serial>` — match by USB serial number (stable across replugs)
/// 3. `--port-index <N>` — 1-based index into auto-detected ports
/// 4. Config file `device.serial_port`
/// 5. Single auto-detected port (no ambiguity)
/// 6. Error with numbered port list (for `--port-index` on retry)
///
/// When multiple USB serial devices are found and no `--port-index` is given,
/// prints a numbered list and returns an error — this is machine-readable so
/// an LLM or script can parse it and retry with `--port-index N`.
pub fn resolve_port(
    cli_port: Option<&str>,
    usb_serial: Option<&str>,
    port_index: Option<usize>,
    config: Option<&crate::config::Config>,
) -> anyhow::Result<String> {
    // 1. Explicit port path wins
    if let Some(p) = cli_port {
        return Ok(p.to_string());
    }

    // 2. USB serial number lookup
    if let Some(serial) = usb_serial {
        return resolve_port_by_serial(serial);
    }

    let ports = detect_esp_ports();

    // 3. Port index selects from auto-detected list
    if let Some(idx) = port_index {
        if idx < 1 || idx > ports.len() {
            if ports.is_empty() {
                bail!("No serial ports detected — cannot use --port-index {}", idx);
            }
            bail!(
                "--port-index {} out of range (1-{}).\n{}",
                idx,
                ports.len(),
                format_port_list(&ports)
            );
        }
        let selected = &ports[idx - 1];
        println!(
            "Selected port [{}]: {} ({})",
            idx,
            selected.port_name,
            selected.description()
        );
        return Ok(selected.port_name.clone());
    }

    // 4. Config file
    if let Some(cfg) = config {
        return Ok(cfg.device.serial_port.clone());
    }

    // 5. Single auto-detected port
    if ports.len() == 1 {
        let p = &ports[0];
        println!("Auto-detected serial port: {}", p.port_name);
        if p.description() != "USB device" {
            println!("  {}", p.description());
        }
        return Ok(p.port_name.clone());
    }

    // 6. Multiple ports — error with numbered list
    if !ports.is_empty() {
        bail!("{}", format_port_list(&ports));
    }

    bail!(
        "No serial port found. Use --port <device>, --serial <usb-serial>, or set device.serial_port in launa.toml\n\
         Detected serial ports: {}",
        list_available_ports()
    )
}

/// List all detected ESP serial ports with their USB serial numbers.
/// Used by `cargo xtask list-ports` for discovering stable identifiers.
pub fn list_ports() -> anyhow::Result<()> {
    let ports = detect_esp_ports();
    if ports.is_empty() {
        println!("No ESP serial ports detected.");
        return Ok(());
    }

    println!("Detected ESP serial ports:\n");
    for (i, p) in ports.iter().enumerate() {
        println!("  [{}] {}", i + 1, p.port_name);
        if let Some(ref sn) = p.serial_number {
            println!("       USB serial: {}", sn);
        }
        if let Some(ref mfr) = p.manufacturer {
            println!("       Manufacturer: {}", mfr);
        }
        if let Some(ref prod) = p.product {
            println!("       Product: {}", prod);
        }
        if let (Some(v), Some(pid)) = (p.vid, p.pid) {
            println!("       VID:PID: {:04x}:{:04x}", v, pid);
        }
        println!();
    }

    println!("Tip: Use --serial <usb-serial> to target a specific device across replugs.");
    println!("     The USB serial number is stable even when the /dev/ path changes.");
    Ok(())
}

/// Resolve the serial port with auto-detection fallback.
///
/// Like `resolve_port`, but returns `default` when nothing is found.
/// Does not print a port list or error — used for non-critical port selection.
#[allow(dead_code)]
pub fn resolve_port_or(
    cli_port: Option<&str>,
    usb_serial: Option<&str>,
    config: Option<&crate::config::Config>,
    default: &str,
) -> String {
    if let Some(p) = cli_port {
        return p.to_string();
    }
    if let Some(serial) = usb_serial {
        if let Ok(port) = resolve_port_by_serial(serial) {
            return port;
        }
    }
    if let Some(cfg) = config {
        return cfg.device.serial_port.clone();
    }
    let ports = detect_esp_ports();
    if ports.len() == 1 {
        return ports[0].port_name.clone();
    }
    default.to_string()
}

/// A parsed entry from an ESP-IDF partition table CSV.
pub struct PartitionEntry {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

/// Parse an ESP-IDF `partitions.csv` file.
///
/// Skips comment lines (starting with `#`) and blank lines.
/// Each data line is: `name, type, subtype, offset, size[, flags]`
/// where offset and size are hex strings like `0x20000`.
pub fn parse_partitions_csv(path: &Path) -> anyhow::Result<Vec<PartitionEntry>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read partition table: {}", path.display()))?;
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(|f| f.trim()).collect();
        if fields.len() < 5 {
            continue;
        }
        let name = fields[0].to_string();
        let offset = u32::from_str_radix(fields[3].trim_start_matches("0x"), 16)
            .with_context(|| format!("Invalid offset '{}' for partition '{}'", fields[3], name))?;
        let size = u32::from_str_radix(fields[4].trim_start_matches("0x"), 16)
            .with_context(|| format!("Invalid size '{}' for partition '{}'", fields[4], name))?;
        entries.push(PartitionEntry { name, offset, size });
    }
    Ok(entries)
}

/// Find a partition by name from a parsed partition table.
pub fn find_partition<'a>(
    entries: &'a [PartitionEntry],
    name: &str,
) -> anyhow::Result<&'a PartitionEntry> {
    entries
        .iter()
        .find(|e| e.name == name)
        .with_context(|| format!("Partition '{}' not found in partition table", name))
}

/// Return a comma-separated list of available serial ports (for error messages).
fn list_available_ports() -> String {
    match serialport::available_ports() {
        Ok(ports) if ports.is_empty() => "none".to_string(),
        Ok(ports) => ports
            .iter()
            .map(|p| p.port_name.clone())
            .collect::<Vec<_>>()
            .join(", "),
        Err(_) => "unable to enumerate".to_string(),
    }
}

/// Shared implementation for flashing auxiliary ESP32 firmware (sniffer, debugger, spa-emulator).
///
/// Parses the standard `--port`/`--serial`/`--port-index`/`--monitor` flags,
/// builds with `cargo +esp espflash flash`, and optionally passes WiFi/MQTT
/// config as build-time env vars.
///
/// # Arguments
/// * `args` - CLI arguments after the command name
/// * `label` - Human-readable name for log/error messages (e.g. "RS-485 debugger")
/// * `app_dir_name` - Subdirectory name under the project root (e.g. "app-sniffer")
/// * `config` - Optional loaded config. If `Some`, WiFi/MQTT env vars are passed to the build.
pub fn flash_app(
    args: &[String],
    label: &str,
    app_dir_name: &str,
    config: Option<&crate::config::Config>,
) -> anyhow::Result<()> {
    let mut port = None;
    let mut serial = None;
    let mut port_index = None;
    let mut monitor = false;
    let mut parser = Args::new(args);
    while parser.has_more() {
        match parser.peek().unwrap() {
            "--port" => port = Some(parser.value("--port")?.to_string()),
            "--serial" => serial = Some(parser.value("--serial")?.to_string()),
            "--port-index" => port_index = parser.optional_parsed("--port-index")?,
            "--monitor" => {
                monitor = true;
                parser.skip();
            }
            _ => return Err(parser.unknown_arg()),
        }
    }

    let resolved_port = resolve_port(port.as_deref(), serial.as_deref(), port_index, config)?;
    let port = Some(resolved_port);

    let app_dir = project_root().join(app_dir_name);

    let mut cmd = Command::new("cargo");
    cmd.arg("+esp")
        .arg("espflash")
        .arg("flash")
        .arg("--chip")
        .arg("esp32");
    cmd.arg("--partition-table").arg("partitions.csv");

    if monitor {
        cmd.arg("--monitor");
    }
    if let Some(ref p) = port {
        cmd.arg("-p").arg(p);
    }

    // Pass WiFi/MQTT config as build-time env vars if available
    if let Some(cfg) = config {
        cmd.env("LAUNA_WIFI_SSID", &cfg.wifi.ssid);
        cmd.env("LAUNA_WIFI_PASSWORD", &cfg.wifi.password);
        cmd.env("LAUNA_MQTT_HOST", &cfg.mqtt.host);
        cmd.env("LAUNA_MQTT_PORT", cfg.mqtt.port.to_string());
    }

    cmd.current_dir(&app_dir);

    println!("Flashing {} firmware...", label);
    println!("  Working dir: {}", app_dir.display());
    if let Some(ref p) = port {
        println!("  Port: {}", p);
    }
    if monitor {
        println!("  Monitor: enabled (serial log after flash)");
    }
    if config.is_some() {
        let cfg = config.unwrap();
        println!("  WiFi SSID: {}", cfg.wifi.ssid);
        println!("  MQTT host: {}:{}", cfg.mqtt.host, cfg.mqtt.port);
    }

    let status = cmd
        .status()
        .context("Failed to run cargo espflash. Is cargo-espflash installed?")?;

    if status.success() {
        println!("{} flash successful.", label);
        Ok(())
    } else {
        bail!("{} flash failed with exit code {:?}", label, status.code());
    }
}
