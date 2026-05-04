//! Custom panic handler for ESP32.
//!
//! Logs panic location and message directly to UART0 registers (bypassing
//! the logger lock), stores crash info to NVS for next-boot MQTT publishing,
//! waits for UART flush, then triggers a software reset.

use crate::{crash_info, uart_raw};

/// Custom panic handler: logs panic location, stores crash info to NVS,
/// waits for UART flush, then triggers a software reset.
/// Replaces esp-backtrace's default infinite loop to allow automatic recovery
/// from panics. Crash info is published via MQTT on next boot.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Write directly to UART0 registers — don't use the logger since
    // the panic might have occurred while holding the logger lock.

    // Print heap free first — uses only stack, no allocation.
    let heap_free = esp_alloc::HEAP.free();
    {
        let heap_msg = core::format_args!("\nHEAP free: {} bytes\n", heap_free);
        let mut heap_buf = [0u8; 48];
        let mut w = SliceWrite::new(&mut heap_buf);
        let _ = core::fmt::Write::write_fmt(&mut w, heap_msg);
        let heap_len = w.len();
        uart_raw::write_bytes(&heap_buf[..heap_len]);
        uart_raw::flush();
    }

    // Print location — short format (filename only) to avoid truncation.
    if let Some(loc) = info.location() {
        let file = loc.file();
        let filename = file.rsplit('/').next().unwrap_or(file);
        let loc_msg = core::format_args!(
            "PANIC {}:{}\n",
            filename,
            loc.line(),
        );
        let mut loc_buf = [0u8; 80];
        let mut w = SliceWrite::new(&mut loc_buf);
        let _ = core::fmt::Write::write_fmt(&mut w, loc_msg);
        let loc_len = w.len();
        uart_raw::write_bytes(&loc_buf[..loc_len]);
        uart_raw::flush();
    }

    // Print full panic message (may be long for OOM).
    // Use heap check: if heap is zero/critically low, skip the full
    // message since format! would re-trigger OOM → infinite recursion.
    if heap_free > 256 {
        let msg = core::format_args!("MSG: {}\n", info);
        let mut buf = [0u8; 1024];
        let mut writer = SliceWrite::new(&mut buf);
        let _ = core::fmt::Write::write_fmt(&mut writer, msg);
        let written = writer.len();
        uart_raw::write_bytes(&buf[..written]);
        // Flush twice to ensure all bytes are sent before the delay
        uart_raw::flush();
        uart_raw::flush();

        // Write crash info to NVS (pre-check prevents repeated writes in crash loops)
        let panic_msg = core::str::from_utf8(&buf[..written]).unwrap_or("PANIC");
        let reason = crash_info::CrashReason::classify(panic_msg);
        crash_info::write_crash_info(reason, panic_msg);
    }

    // Busy-wait ~1s to allow UART TX to fully transmit.
    const PANIC_DELAY_ITERATIONS: u32 = 10_000_000;
    let mut counter: u32 = 0;
    while counter < PANIC_DELAY_ITERATIONS {
        counter += 1;
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }

    esp_hal::system::software_reset()
}

/// Minimal writer that writes to a byte slice and tracks position.
struct SliceWrite<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> SliceWrite<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        SliceWrite { buf, pos: 0 }
    }

    fn len(&self) -> usize {
        self.pos
    }
}

impl<'a> core::fmt::Write for SliceWrite<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = &mut self.buf[self.pos..];
        let len = bytes.len().min(remaining.len());
        remaining[..len].copy_from_slice(&bytes[..len]);
        self.pos += len;
        Ok(())
    }
}
