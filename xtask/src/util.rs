//! Shared utilities for xtask modules.

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
