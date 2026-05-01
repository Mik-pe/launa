//! Shared utilities for xtask modules.

use anyhow::{bail, Context};
use std::path::{Path, PathBuf};

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

/// Auto-detect an ESP32 USB serial port.
///
/// On macOS, scans `/dev/cu.usb*` for common USB-serial adapters.
/// On Linux, scans `/dev/ttyUSB*` and `/dev/ttyACM*`.
/// Returns the first matching device path, or `None` if nothing is found.
pub fn auto_detect_serial_port() -> Option<String> {
    let candidates: Vec<&str> = if cfg!(target_os = "macos") {
        vec![
            "/dev/cu.usbserial*",
            "/dev/cu.usbmodem*",
            "/dev/cu.wchusbserial*",
        ]
    } else if cfg!(target_os = "linux") {
        vec!["/dev/ttyUSB*", "/dev/ttyACM*"]
    } else {
        vec!["COM*"]
    };

    for pattern in candidates {
        if let Ok(entries) = glob::glob(pattern) {
            for entry in entries.flatten() {
                let path = entry.to_string_lossy().to_string();
                // Prefer /dev/cu.* over /dev/tty.* on macOS (call-out device, no carrier detect blocking)
                return Some(path);
            }
        }
    }

    // Fallback: try serialport enumeration
    if let Ok(ports) = serialport::available_ports() {
        for port in ports {
            if is_likely_esp_port(&port.port_name) {
                return Some(port.port_name);
            }
        }
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

/// Resolve the serial port using the "CLI arg → config → auto-detect → error" pattern.
///
/// Priority: `--port` flag > config file > auto-detect USB device.
pub fn resolve_port(
    cli_port: Option<&str>,
    config: Option<&crate::config::Config>,
) -> anyhow::Result<String> {
    if let Some(p) = cli_port {
        return Ok(p.to_string());
    }
    if let Some(cfg) = config {
        return Ok(cfg.device.serial_port.clone());
    }
    if let Some(p) = auto_detect_serial_port() {
        println!("Auto-detected serial port: {}", p);
        return Ok(p);
    }
    bail!(
        "No serial port found. Use --port <device> or set device.serial_port in launa.toml\n\
         Detected serial ports: {}",
        list_available_ports()
    )
}

/// Resolve the serial port with auto-detection fallback.
///
/// Like `resolve_port`, but returns `default` when nothing is found.
#[allow(dead_code)]
pub fn resolve_port_or(
    cli_port: Option<&str>,
    config: Option<&crate::config::Config>,
    default: &str,
) -> String {
    if let Some(p) = cli_port {
        return p.to_string();
    }
    if let Some(cfg) = config {
        return cfg.device.serial_port.clone();
    }
    if let Some(p) = auto_detect_serial_port() {
        return p;
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
