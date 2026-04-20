use anyhow::{bail, Context};
use std::process::Command;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port_name = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                if i >= args.len() {
                    bail!("--port requires a value");
                }
                port_name = Some(args[i].clone());
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let config = crate::config::load()?;
    let port_name = port_name.unwrap_or(config.device.serial_port.clone());

    // Build config lines
    let mut lines = Vec::new();
    lines.push("CONFIG_START".to_string());
    lines.push(format!("wifi.ssid={}", config.wifi.ssid));
    lines.push(format!("wifi.password={}", config.wifi.password));
    lines.push(format!("mqtt.host={}", config.mqtt.host));
    lines.push(format!("mqtt.port={}", config.mqtt.port));
    if !config.mqtt.user.is_empty() {
        lines.push(format!("mqtt.user={}", config.mqtt.user));
    }
    if !config.mqtt.password.is_empty() {
        lines.push(format!("mqtt.password={}", config.mqtt.password));
    }
    lines.push(format!("device.id={}", config.device.id));
    lines.push("CONFIG_END".to_string());

    // Write config payload to a temp file so Python can read it (avoids escaping issues)
    let temp_dir = std::env::temp_dir();
    let config_file = temp_dir.join("launa_config_payload.txt");
    let payload: String = lines.join("\n");
    std::fs::write(&config_file, &payload).context("Failed to write temp config file")?;

    // Write Python script to a temp file
    let script_file = temp_dir.join("launa_config_flash.py");
    let python_script = format!(
        r#"
import serial, time, sys

port_name = sys.argv[1]
config_file = sys.argv[2]

with open(config_file, 'r') as f:
    config_lines = f.read()

print(f'Opening {{port_name}}...', file=sys.stderr)
port = serial.Serial(port_name, 115200, timeout=1)

# Wait for ESP32 ready signal
start = time.time()
ready = False
all_output = ''
while time.time() - start < 35:
    data = port.read(4096)
    if data:
        text = data.decode('utf-8', errors='replace')
        all_output += text
        sys.stderr.write(text)
        sys.stderr.flush()
        if 'Waiting for serial config' in all_output:
            ready = True
            break

if not ready:
    print(f'ERROR: ESP32 not ready within 35s. Output so far: {{all_output}}', file=sys.stderr)
    port.close()
    sys.exit(1)

print('ESP32 ready, sending config...', file=sys.stderr)
time.sleep(0.1)

# Send each config line with CRLF line endings
for line in config_lines.strip().split('\n'):
    port.write((line + '\r\n').encode('utf-8'))
    print(f'  Sent: {{line}}', file=sys.stderr)
port.flush()

# Wait for response
start = time.time()
response = b''
while time.time() - start < 10:
    data = port.read(4096)
    if data:
        response += data
        sys.stderr.write(data.decode('utf-8', errors='replace'))
        sys.stderr.flush()
        if b'CONFIG_OK' in response or b'CONFIG_ERROR' in response:
            break

port.close()

if b'CONFIG_OK' in response:
    print('CONFIG_OK')
elif b'CONFIG_ERROR' in response:
    idx = response.find(b'CONFIG_ERROR:')
    error = response[idx+len(b'CONFIG_ERROR:'):].decode('utf-8', errors='replace').strip()
    print(f'CONFIG_ERROR: {{error}}')
else:
    decoded = response.decode('utf-8', errors='replace').strip()
    print(f'NO_RESPONSE: {{decoded}}')
"#
    );
    std::fs::write(&script_file, &python_script).context("Failed to write temp Python script")?;

    println!("Writing config to ESP32 via {} (using Python/pyserial)...", port_name);

    let output = Command::new("python")
        .arg(&script_file)
        .arg(&port_name)
        .arg(&config_file)
        .output()
        .context("Failed to run Python. Is Python with pyserial installed? (pip install pyserial)")?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !stderr.is_empty() {
        eprintln!("Python stderr: {}", stderr);
    }

    // Clean up temp files
    let _ = std::fs::remove_file(&config_file);
    let _ = std::fs::remove_file(&script_file);

    if !output.status.success() {
        bail!("Python config-flash script failed: {}", stderr);
    }

    if stdout.starts_with("CONFIG_OK") {
        println!("Config written successfully!");
        Ok(())
    } else if stdout.starts_with("CONFIG_ERROR:") {
        bail!("ESP32 config error: {}", &stdout["CONFIG_ERROR:".len()..]);
    } else if stdout.starts_with("NO_RESPONSE:") {
        bail!("No acknowledgment from ESP32: {}", &stdout["NO_RESPONSE:".len()..]);
    } else {
        bail!("Unexpected response: {}", stdout);
    }
}
