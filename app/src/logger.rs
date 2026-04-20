//! Custom logger with optional serial and remote outputs.
//!
//! Two Cargo features control output:
//! - `serial-log` — writes to UART0 using raw ESP32 registers (off by default)
//! - `remote-log` — forwards to MQTT via the remote_log module (on by default)
//!
//! When serial-log is disabled, the UART register code and spinlock are not
//! compiled, saving code space and eliminating UART overhead in production.

extern crate alloc;

use log::{Level, LevelFilter, Metadata, Record};

// ---------------------------------------------------------------------------
// Serial UART output (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "serial-log")]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "serial-log")]
use esp_hal::system::Cpu;

/// ESP32 UART0 register base address.
#[cfg(feature = "serial-log")]
const UART0_BASE: usize = 0x60000000;

/// FIFO register (write-only, writes go to TX FIFO).
#[cfg(feature = "serial-log")]
const UART_FIFO_REG: usize = UART0_BASE;

/// Status register - bits 16-22 contain TX FIFO count.
#[cfg(feature = "serial-log")]
const UART_STATUS_REG: usize = UART0_BASE + 0x1C;

/// TX FIFO size for ESP32.
#[cfg(feature = "serial-log")]
const UART_FIFO_SIZE: u16 = 128;

/// Mask for TX FIFO count in status register.
#[cfg(feature = "serial-log")]
const TX_FIFO_CNT_MASK: u32 = 0x7F << 16;

/// Spinlock for cross-core UART synchronization.
#[cfg(feature = "serial-log")]
struct Spinlock {
    locked: AtomicBool,
}

#[cfg(feature = "serial-log")]
impl Spinlock {
    const fn new() -> Self {
        Spinlock {
            locked: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    #[inline(always)]
    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

#[cfg(feature = "serial-log")]
static UART_LOCK: Spinlock = Spinlock::new();

#[cfg(feature = "serial-log")]
#[inline]
fn tx_fifo_count() -> u16 {
    unsafe {
        let status = (UART_STATUS_REG as *const u32).read_volatile();
        ((status & TX_FIFO_CNT_MASK) >> 16) as u16
    }
}

#[cfg(feature = "serial-log")]
#[inline]
fn write_byte(b: u8) {
    while tx_fifo_count() >= UART_FIFO_SIZE {
        core::hint::spin_loop();
    }
    unsafe {
        (UART_FIFO_REG as *mut u8).write_volatile(b);
    }
}

#[cfg(feature = "serial-log")]
fn write_bytes(data: &[u8]) {
    for &b in data {
        write_byte(b);
    }
}

#[cfg(feature = "serial-log")]
fn flush_uart() {
    while tx_fifo_count() > 0 {
        core::hint::spin_loop();
    }
    esp_hal::rom::ets_delay_us(10);
}

#[cfg(feature = "serial-log")]
fn color_for_level(level: Level) -> (&'static str, &'static str) {
    const RESET: &str = "\u{001B}[0m";
    const RED: &str = "\u{001B}[31m";
    const GREEN: &str = "\u{001B}[32m";
    const YELLOW: &str = "\u{001B}[33m";
    const BLUE: &str = "\u{001B}[34m";
    const CYAN: &str = "\u{001B}[35m";

    match level {
        Level::Error => (RED, RESET),
        Level::Warn => (YELLOW, RESET),
        Level::Debug => (BLUE, RESET),
        Level::Info => (GREEN, RESET),
        Level::Trace => (CYAN, RESET),
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
            let core_id = match Cpu::current() {
                Cpu::ProCpu => 0,
                Cpu::AppCpu => 1,
            };

            let (color, reset) = color_for_level(record.level());
            let msg = alloc::format!(
                "{}[C{}] {} - {}{}",
                color,
                core_id,
                record.level(),
                record.args(),
                reset,
            );

            UART_LOCK.lock();
            write_bytes(msg.as_bytes());
            write_byte(b'\n');
            UART_LOCK.unlock();
        }
    }

    #[cfg(feature = "serial-log")]
    fn flush(&self) {
        UART_LOCK.lock();
        flush_uart();
        UART_LOCK.unlock();
    }

    #[cfg(not(feature = "serial-log"))]
    fn flush(&self) {}
}
