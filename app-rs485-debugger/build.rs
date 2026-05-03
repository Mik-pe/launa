use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");
    println!("cargo:rustc-link-arg=-Tlinkall.x");

    let short_sha = git_short_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_SHORT_SHA={}", short_sha);

    // Read WiFi/MQTT config from launa.toml directly.
    // This allows `cargo check` to work without xtask setting env vars.
    // The xtask flash path also sets env vars, which take precedence.
    let config_path = project_root().join("launa.toml");
    println!("cargo:rerun-if-changed={}", config_path.display());

    if config_path.exists() {
        let contents = std::fs::read_to_string(&config_path)
            .expect("Failed to read launa.toml");
        let config: toml::Value = toml::from_str(&contents)
            .expect("Failed to parse launa.toml");

        if let Some(val) = config.get("wifi").and_then(|w| w.get("ssid")).and_then(|v| v.as_str()) {
            println!("cargo:rustc-env=LAUNA_WIFI_SSID={}", val);
        }
        if let Some(val) = config.get("wifi").and_then(|w| w.get("password")).and_then(|v| v.as_str()) {
            println!("cargo:rustc-env=LAUNA_WIFI_PASSWORD={}", val);
        }
        if let Some(val) = config.get("mqtt").and_then(|m| m.get("host")).and_then(|v| v.as_str()) {
            println!("cargo:rustc-env=LAUNA_MQTT_HOST={}", val);
        }
        if let Some(val) = config.get("mqtt").and_then(|m| m.get("port")).and_then(|v| v.as_integer()) {
            println!("cargo:rustc-env=LAUNA_MQTT_PORT={}", val);
        }
    } else {
        panic!(
            "Config file not found: {}\nCopy launa.example.toml to launa.toml and fill in your values.",
            config_path.display()
        );
    }
}

fn project_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
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
        return None;
    }
    Some(sha)
}
