//! Shared utilities for xtask modules.

use anyhow::{bail, Context};
use std::path::PathBuf;

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

/// Resolve the serial port using the "CLI arg → config → fallback" pattern.
///
/// If `cli_port` is `Some`, that wins. Otherwise falls back to
/// `config.device.serial_port`. If neither provides a value, returns an error.
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
    bail!("No serial port specified. Use --port or set device.serial_port in launa.toml")
}

/// Resolve the serial port with a fallback default when no config is available.
///
/// Like `resolve_port`, but returns `default` when neither CLI nor config provides a value.
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
    default.to_string()
}
