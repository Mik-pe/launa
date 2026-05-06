//! Custom logger with optional serial and remote outputs.
//!
//! Two Cargo features control output:
//! - `serial-log` — writes to UART0 using raw ESP32 registers (off by default)
//! - `remote-log` — forwards to MQTT via the remote_log module (on by default)
//!
//! When serial-log is disabled, the UART code is not compiled, saving code
//! space and eliminating UART overhead in production.
//!
//! ## UART output strategy
//!
//! The ESP32 UART0 TX FIFO is only 128 bytes, and the esp-rtos scheduler
//! can preempt between log calls. Two mechanisms prevent interleaved output:
//!
//! 1. **Non-reentrant try-lock** (AtomicBool CAS): only one log call may
//!    write to the UART at a time. If the lock is already held (e.g. the
//!    current task was preempted mid-write, or re-entrancy from the
//!    allocator), the contending message is silently dropped.
//!
//! 2. **Post-write flush**: after writing all bytes, we busy-wait until
//!    the TX FIFO has fully drained. This guarantees the previous message
//!    is completely transmitted before the lock is released and the next
//!    message begins, so no bytes from different messages coexist in the
//!    FIFO.
//!
//! We deliberately do NOT use `esp_sync::RawMutex` — it identifies
//! threads by Xtensa processor ID, which is the same for all RTOS tasks
//! on a single core, making it reentrant and defeating mutual exclusion.

extern crate alloc;

use log::{Level, LevelFilter, Metadata, Record};

#[cfg(feature = "serial-log")]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "serial-log")]
use esp_hal::system::Cpu;

#[cfg(feature = "serial-log")]
static UART_LOCK: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "serial-log")]
fn color_for_level(level: Level) -> &'static str {
    const RED: &str = "\u{001B}[31m";
    const GREEN: &str = "\u{001B}[32m";
    const YELLOW: &str = "\u{001B}[33m";
    const BLUE: &str = "\u{001B}[34m";
    const CYAN: &str = "\u{001B}[36m";

    match level {
        Level::Error => RED,
        Level::Warn => YELLOW,
        Level::Debug => BLUE,
        Level::Info => GREEN,
        Level::Trace => CYAN,
    }
}

// ---------------------------------------------------------------------------
// Level filter and init
// ---------------------------------------------------------------------------

fn level_from_env() -> LevelFilter {
    match option_env!("ESP_LOG") {
        Some("error") => LevelFilter::Error,
        Some("warn") => LevelFilter::Warn,
        Some("debug") => LevelFilter::Debug,
        Some("trace") => LevelFilter::Trace,
        Some("off") => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

/// Initialize the logger.
///
/// Call this after `esp_hal::init()`.
pub fn init() {
    unsafe {
        log::set_logger_racy(&Logger).unwrap();
        log::set_max_level_racy(level_from_env());
    }
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        // Capture info/warn/error to remote log buffer for MQTT publishing
        #[cfg(feature = "remote-log")]
        if record.level() <= Level::Info {
            crate::remote_log::capture_log(record.level(), &alloc::format!("{}", record.args()));
        }

        // Write to UART0 serial output
        #[cfg(feature = "serial-log")]
        {
            // Try to acquire the lock. If already held, drop this message
            // to avoid deadlock from re-entrancy (e.g. allocator -> log).
            if UART_LOCK
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                return;
            }

            let core_id = match Cpu::current() {
                Cpu::ProCpu => 0,
                Cpu::AppCpu => 1,
            };

            let color = color_for_level(record.level());
            const RESET: &str = "\u{001B}[0m";
            let ts_ms = embassy_time::Instant::now().as_millis();
            let msg = alloc::format!(
                "{}[C{}] {:5}.{:03}s {} - {}{}\n",
                color,
                core_id,
                ts_ms / 1000,
                ts_ms % 1000,
                record.level(),
                record.args(),
                RESET,
            );

            crate::uart_raw::write_bytes(msg.as_bytes());
            crate::uart_raw::flush(); // Wait for all bytes to be sent

            UART_LOCK.store(false, Ordering::Release);
        }
    }

    #[cfg(feature = "serial-log")]
    fn flush(&self) {
        crate::uart_raw::flush();
    }

    #[cfg(not(feature = "serial-log"))]
    fn flush(&self) {}
}
