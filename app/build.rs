//! Build script that embeds the Git short SHA into the firmware binary.
//!
//! Sets `GIT_SHORT_SHA` as a rustc env variable so it can be used via
//! `env!("GIT_SHORT_SHA")` in the main binary. Falls back to `"unknown"`
//! when git is not available (e.g., CI without git, or tarball builds).

use std::process::Command;

fn main() {
    // Rerun if the Git HEAD changes (new commits, branch switches).
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");

    let short_sha = git_short_sha().unwrap_or_else(|| {
        // Fallback: try GITHUB_SHA env var (CI environments)
        std::env::var("GITHUB_SHA")
            .map(|sha| {
                // GITHUB_SHA is the full SHA; truncate to 7 chars
                if sha.len() >= 7 {
                    sha[..7].to_string()
                } else {
                    sha
                }
            })
            .unwrap_or_else(|_| "unknown".to_string())
    });

    println!("cargo:rustc-env=GIT_SHORT_SHA={}", short_sha);
}

/// Run `git rev-parse --short HEAD` and return the short SHA.
fn git_short_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        return None;
    }

    Some(sha)
}
