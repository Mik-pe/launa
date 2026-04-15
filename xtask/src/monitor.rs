use anyhow::{bail, Context};
use std::io::Read;
use std::time::{Duration, Instant};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port_name = "COM3".to_string();
    let mut duration_secs = 10u64;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                port_name = args[i].clone();
            }
            "--duration" => {
                i += 1;
                duration_secs = args[i].parse().context("Invalid duration")?;
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let port = serialport::new(&port_name, 115200)
        .timeout(Duration::from_millis(100))
        .open()
        .with_context(|| format!("Failed to open serial port {}", port_name))?;

    println!(
        "Monitoring on {} for {}s... (Ctrl+C to stop)",
        port_name, duration_secs
    );
    let start = Instant::now();
    let deadline = start + Duration::from_secs(duration_secs);

    // Ctrl+C handler
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc_handler(move || {
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    let mut buf = [0u8; 256];
    let mut port = port;
    while Instant::now() < deadline && running.load(std::sync::atomic::Ordering::SeqCst) {
        match port.read(&mut buf) {
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                print!("{}", text);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => bail!("Serial read error: {}", e),
        }
    }

    println!("\nMonitor stopped.");
    Ok(())
}

fn ctrlc_handler<F: Fn() + Send + 'static>(handler: F) {
    let _ = ctrlc::set_handler(handler);
}
