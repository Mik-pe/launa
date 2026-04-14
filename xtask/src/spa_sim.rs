use anyhow::{bail, Context};
use launa_sim::{SpaSim, SpaState};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port_name = "COM5".to_string();
    let mut duration_secs = 60u64;
    let mut respond = false;
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
            "--respond" => {
                respond = true;
            }
            other => bail!("Unknown argument: {}", other),
        }
        i += 1;
    }

    let port = serialport::new(&port_name, 115200)
        .timeout(Duration::from_millis(100))
        .open()
        .with_context(|| format!("Failed to open serial port {}", port_name))?;

    println!("Spa simulator on {} for {}s (respond={})", port_name, duration_secs, respond);
    println!("Sending Balboa status frames at 1Hz...");
    println!();

    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc_handler(move || {
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    let mut sim = SpaSim::new();
    let mut port = port;
    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    let mut last_tick = Instant::now();

    while Instant::now() < deadline && running.load(std::sync::atomic::Ordering::SeqCst) {
        let now = Instant::now();
        if now - last_tick >= Duration::from_secs(1) {
            last_tick = now;

            // Generate and send spa frames
            let output = sim.tick();
            port.write_all(&output)
                .context("Failed to write to serial port")?;
            port.flush().context("Failed to flush serial port")?;

            print_state(sim.tick_count(), &sim.state);
        }

        // Check for incoming commands if --respond
        if respond {
            let mut buf = [0u8; 256];
            match port.read(&mut buf) {
                Ok(n) => {
                    let responses = sim.process_incoming_bytes(&buf[..n]);
                    if !responses.is_empty() {
                        port.write_all(&responses)
                            .context("Failed to write response")?;
                        port.flush()?;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => bail!("Serial read error: {}", e),
            }
        } else {
            // Small sleep to avoid busy loop
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    println!("\nSpa simulator stopped after {} ticks.", sim.tick_count());
    Ok(())
}

fn print_state(tick: u64, state: &SpaState) {
    use launa_sim::PumpState;
    let pump_str = |p: PumpState| match p {
        PumpState::Off => "off",
        PumpState::Low => "low",
        PumpState::High => "high",
    };
    println!(
        "[tick {:>4}] temp={:.1} set={:.1} heating={} pump1={} pump2={} pump3={} circ={} blower={} light={} mister={} hold={}",
        tick,
        state.current_temp,
        state.set_temp,
        state.is_heating,
        pump_str(state.pump1),
        pump_str(state.pump2),
        pump_str(state.pump3),
        state.circ_pump,
        state.blower,
        state.light1,
        state.mister,
        state.hold,
    );
}

fn ctrlc_handler<F: Fn() + Send + 'static>(handler: F) {
    let _ = ctrlc::set_handler(handler);
}
