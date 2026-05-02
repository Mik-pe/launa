use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");
    println!("cargo:rustc-link-arg=-Tlinkall.x");

    let short_sha = git_short_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_SHORT_SHA={}", short_sha);

    forward_env("LAUNA_WIFI_SSID");
    forward_env("LAUNA_WIFI_PASSWORD");
    forward_env("LAUNA_MQTT_HOST");
    forward_env("LAUNA_MQTT_PORT");
}

fn forward_env(name: &str) {
    if let Ok(val) = std::env::var(name) {
        println!("cargo:rustc-env={}={}", name, val);
    }
    println!("cargo:rerun-if-env-changed={}", name);
}

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
        None
    } else {
        Some(sha)
    }
}
