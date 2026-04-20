//! Custom logger using raw ESP32 UART0 registers.
//!
//! Bypasses esp-println's ROM function `uart_tx_one_char` which has known
//! issues with character interleaving on ESP32. Writes directly to UART0
//! FIFO with proper spin-waiting and cross-core locking.

extern crate alloc;

use core::sync::atomic::{AtomicBool, Ordering};
use esp_hal::system::Cpu;
use log::{Level, LevelFilter, Metadata, Record};

/// ESP32 UART0 register base address.
const UART0_BASE: usize = 0x60000000;

/// FIFO register (write-only, writes go to TX FIFO).
const UART_FIFO_REG: usize = UART0_BASE;

/// Status register - bits 16-22 contain TX FIFO count.
const UART_STATUS_REG: usize = UART0_BASE + 0x1C;

/// TX FIFO size for ESP32.
const UART_FIFO_SIZE: u16 = 128;

/// Mask for TX FIFO count in status register.
const TX_FIFO_CNT_MASK: u32 = 0x7F << 16;

/// Spinlock for cross-core UART synchronization.
struct Spinlock {
    locked: AtomicBool,
}

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

static UART_LOCK: Spinlock = Spinlock::new();

/// Read TX FIFO count from status register.
#[inline]
fn tx_fifo_count() -> u16 {
    unsafe {
        let status = (UART_STATUS_REG as *const u32).read_volatile();
        ((status & TX_FIFO_CNT_MASK) >> 16) as u16
    }
}

/// Write a single byte to UART0 TX FIFO, waiting if full.
#[inline]
fn write_byte(b: u8) {
    // Spin until there's space in the FIFO
    while tx_fifo_count() >= UART_FIFO_SIZE {
        core::hint::spin_loop();
    }
    // Write byte to FIFO
    unsafe {
        (UART_FIFO_REG as *mut u8).write_volatile(b);
    }
}

/// Write bytes to UART0 with proper FIFO management.
fn write_bytes(data: &[u8]) {
    for &b in data {
        write_byte(b);
    }
}

/// Wait for TX FIFO to drain completely.
fn flush_uart() {
    while tx_fifo_count() > 0 {
        core::hint::spin_loop();
    }
    // Small delay to ensure last byte is fully transmitted
    // (ESP32 UART FSM has a brief idle state after FIFO drains)
    esp_hal::rom::ets_delay_us(10);
}

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
/// Call this after `esp_hal::init()`. The UART0 TX pin (GPIO1) should
/// be configured by the bootloader, so no additional pin setup is needed.
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

    fn flush(&self) {
        UART_LOCK.lock();
        flush_uart();
        UART_LOCK.unlock();
    }
}

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
