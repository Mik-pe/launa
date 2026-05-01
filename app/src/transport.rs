//! RS-485 UART transport using esp-hal async UART.
//!
//! Implements the `launa_hal::Transport` trait, providing a unified async
//! transport abstraction shared between production (ESP32 UART) and test
//! (mock/sim) code.
//!
//! When no DE pin is configured (auto-direction transceiver), a preamble
//! byte (0xFF) is prepended to each write to give the transceiver time to
//! switch direction. Without this, the first byte of the frame (0x7E SOF)
//! can be corrupted on cheap auto-direction modules.

use embassy_time::{Duration, Timer};
use esp_hal::gpio::{AnyPin, Level, Output, OutputConfig};
use esp_hal::uart::Uart;
use esp_hal::Async;
use launa_hal::transport::{Transport, TransportError};
use log::{trace, warn};

/// Delay before releasing DE after flush failure, allowing the shift register
/// to drain at any practical baud rate (~100 bit-times at 115200).
const DE_SAFETY_DELAY_US: u64 = 1000;

/// Default DE assert-to-data delay in microseconds.
/// Only used when an explicit DE pin is configured.
const DE_ASSERT_DELAY_US: u64 = 50;

/// Preamble byte sent before each frame when using an auto-direction
/// transceiver (no DE pin). Must NOT be 0xFF — that's indistinguishable
/// from UART idle (mark state) and won't trigger the direction switch.
/// 0x00 produces a start bit (falling edge) + 8 zero bits, which forces
/// the transceiver into transmit mode. The Balboa parser discards bytes
/// before the 0x7E SOF marker, so this is harmless.
const PREAMBLE_BYTE: u8 = 0x00;

/// RS-485 half-duplex UART transport for Balboa spa communication.
///
/// Wraps an async UART and optional DE (Driver Enable) pin for RS-485
/// transceiver control. When a DE pin is configured, it is automatically
/// asserted HIGH during writes and released LOW after the UART TX FIFO
/// and shift register have fully drained.
///
/// When no DE pin is configured (auto-direction transceiver), preamble
/// bytes are prepended to each write to avoid first-byte corruption.
pub struct Rs485Transport {
    uart: Uart<'static, Async>,
    de_pin: Option<Output<'static>>,
    /// Microseconds to wait after asserting DE before sending data.
    de_assert_delay_us: u64,
}

/// RAII guard that ensures DE pin is set LOW when dropped, even if write()
/// returns early due to an error. Prevents the RS-485 bus from being held
/// in transmit mode indefinitely.
struct DeGuard<'a> {
    de: Option<&'a mut Output<'static>>,
    released: bool,
}

impl DeGuard<'_> {
    /// Explicitly release the DE pin (set LOW) and mark as released so
    /// Drop does not try again.
    fn release(&mut self) {
        if let Some(de) = self.de.as_mut() {
            de.set_low();
        }
        self.released = true;
    }
}

impl Drop for DeGuard<'_> {
    fn drop(&mut self) {
        if !self.released {
            if let Some(de) = self.de.as_mut() {
                de.set_low();
            }
        }
    }
}

impl Rs485Transport {
    /// Create a new RS-485 transport.
    ///
    /// - `uart`: An async UART peripheral configured for 115200 baud.
    /// - `de_pin`: Optional GPIO pin connected to the RS-485 transceiver's
    ///   DE (Driver Enable) input. When `None`, DE pin control is skipped
    ///   and preamble bytes are sent for auto-direction transceivers.
    pub fn new(uart: Uart<'static, Async>, de_pin: Option<AnyPin<'static>>) -> Self {
        let de = de_pin.map(|pin| Output::new(pin, Level::Low, OutputConfig::default()));
        Rs485Transport {
            uart,
            de_pin: de,
            de_assert_delay_us: DE_ASSERT_DELAY_US,
        }
    }

    /// Set the DE assert-to-data delay in microseconds.
    pub fn set_de_assert_delay(&mut self, delay_us: u64) {
        self.de_assert_delay_us = delay_us;
    }

    /// Sync write bytes directly to the UART TX FIFO. No preamble, no DE control.
    /// Used by the registration fast-path for minimum-latency response.
    pub fn write_raw(&mut self, data: &[u8]) -> Result<usize, TransportError> {
        self.uart.write(data).map_err(|_e| TransportError::Io)
    }

    /// Sync flush the UART TX FIFO and shift register.
    pub fn flush_raw(&mut self) -> Result<(), TransportError> {
        self.uart.flush().map_err(|_e| TransportError::Io)
    }
}

impl Transport for Rs485Transport {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        self.uart
            .read_async(buf)
            .await
            .map_err(|_e| TransportError::Io)
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), TransportError> {
        // RAII guard ensures DE is released even on early return due to errors.
        let mut guard = DeGuard {
            de: self.de_pin.as_mut(),
            released: false,
        };

        if let Some(de) = guard.de.as_mut() {
            // Explicit DE pin: assert, wait, send data, flush, release.
            de.set_high();
            if self.de_assert_delay_us > 0 {
                Timer::after(Duration::from_micros(self.de_assert_delay_us)).await;
            }

            let mut written = 0;
            while written < data.len() {
                let n = self
                    .uart
                    .write(&data[written..])
                    .map_err(|_e| TransportError::Io)?;
                written += n;
            }

            let flush_result = self.uart.flush();
            if flush_result.is_err() {
                warn!("UART flush failed — safety delay before releasing DE pin");
                Timer::after(Duration::from_micros(DE_SAFETY_DELAY_US)).await;
            }
            guard.release();

            if let Err(_e) = flush_result {
                return Err(TransportError::Io);
            }
        } else {
            // Auto-direction transceiver: send a preamble byte and flush it
            // to the wire before sending the actual data. The preamble (0x00)
            // triggers the direction-switching circuit to enable the driver.
            // Flushing ensures the transceiver has fully switched before the
            // actual frame data follows. This matches the approach that was
            // verified working with the BP6013G1 controller.
            let preamble = [PREAMBLE_BYTE];
            let mut pw = 0;
            while pw < preamble.len() {
                let n = self
                    .uart
                    .write(&preamble[pw..])
                    .map_err(|_e| TransportError::Io)?;
                pw += n;
            }
            // Flush preamble to wire — ensures transceiver has switched direction
            self.uart.flush().map_err(|_e| TransportError::Io)?;

            // Now send the actual frame data
            let mut written = 0;
            while written < data.len() {
                let n = self
                    .uart
                    .write(&data[written..])
                    .map_err(|_e| TransportError::Io)?;
                written += n;
            }
            self.uart.flush().map_err(|_e| TransportError::Io)?;
        }

        trace!("UART wrote all {} bytes", data.len());
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), TransportError> {
        self.uart.flush().map_err(|_| TransportError::Io)
    }
}
