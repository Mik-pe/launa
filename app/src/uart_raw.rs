//! Raw ESP32 UART0 register access helpers.
//!
//! Provides low-level constants and write primitives for UART0,
//! shared by the panic handler (main.rs) and the serial logger
//! (logger.rs). Both need to bypass the HAL and write directly to
//! hardware registers.

/// ESP32 UART0 register base address.
pub const UART0_BASE: usize = 0x60000000;

/// FIFO register (write-only, writes go to TX FIFO).
pub const UART_FIFO_REG: usize = UART0_BASE;

/// Status register — bits 16-22 contain TX FIFO count.
pub const UART_STATUS_REG: usize = UART0_BASE + 0x1C;

/// TX FIFO size for ESP32.
pub const UART_FIFO_SIZE: u16 = 128;

/// Mask for TX FIFO count in status register.
pub const TX_FIFO_CNT_MASK: u32 = 0x7F << 16;

/// Read the current TX FIFO count from the UART status register.
#[inline]
pub fn tx_fifo_count() -> u16 {
    unsafe {
        let status = (UART_STATUS_REG as *const u32).read_volatile();
        ((status & TX_FIFO_CNT_MASK) >> 16) as u16
    }
}

/// Write a single byte to the UART TX FIFO, spinning until there is space.
#[inline]
pub fn write_byte(b: u8) {
    while tx_fifo_count() >= UART_FIFO_SIZE {
        core::hint::spin_loop();
    }
    unsafe {
        (UART_FIFO_REG as *mut u8).write_volatile(b);
    }
}

/// Write a slice of bytes to the UART TX FIFO, spinning per byte.
#[inline]
pub fn write_bytes(data: &[u8]) {
    for &b in data {
        write_byte(b);
    }
}

/// Spin until the TX FIFO has fully drained, then wait an additional
/// ~10 µs for the shift register to finish transmitting.
#[inline]
pub fn flush() {
    while tx_fifo_count() > 0 {
        core::hint::spin_loop();
    }
    esp_hal::rom::ets_delay_us(10);
}
