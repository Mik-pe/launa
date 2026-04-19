//! Custom logger that prepends the CPU core ID to every log line.
//!
//! Wraps esp-println's output with a `[C0]`/`[C1]` prefix so that
//! dual-core issues are immediately visible in serial output.

use esp_hal::system::Cpu;
use log::{Level, LevelFilter, Metadata, Record};

struct CoreLogger;

static LOGGER: CoreLogger = CoreLogger;

/// Parse log level from ESP_LOG env var at runtime.
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

/// Initialize the core-aware logger.
///
/// Call this instead of `esp_println::logger::init_logger_from_env()`.
pub fn init() {
    unsafe {
        log::set_logger_racy(&LOGGER).unwrap();
        log::set_max_level_racy(level_from_env());
    }
}

impl log::Log for CoreLogger {
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

        esp_println::println!(
            "{}[C{}] {} - {}{}",
            color,
            core_id,
            record.level(),
            record.args(),
            reset,
        );
    }

    fn flush(&self) {}
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
        Level::Info => (GREEN, RESET),
        Level::Debug => (BLUE, RESET),
        Level::Trace => (CYAN, RESET),
    }
}
