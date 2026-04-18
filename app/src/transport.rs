//! RS-485 UART transport using esp-hal async UART.
//!
//! Implements the `launa_hal::Transport` trait, providing a unified async
//! transport abstraction shared between production (ESP32 UART) and test
//! (mock/sim) code.

use embassy_time::{Duration, Timer};
use esp_hal::gpio::{AnyPin, Output, OutputConfig, Level};
use esp_hal::uart::Uart;
use esp_hal::Async;
use launa_hal::transport::{Transport, TransportError};
use log::{trace, warn};

/// RS-485 half-duplex UART transport for Balboa spa communication.
///
/// Wraps an async UART and optional DE (Driver Enable) pin for RS-485
/// transceiver control. When a DE pin is configured, it is automatically
/// asserted HIGH during writes and released LOW after the UART TX FIFO
/// and shift register have fully drained.
pub struct Rs485Transport {
    uart: Uart<'static, Async>,
    de_pin: Option<Output<'static>>,
}

impl Rs485Transport {
    /// Create a new RS-485 transport.
    ///
    /// - `uart`: An async UART peripheral configured for 115200 baud.
    /// - `de_pin`: Optional GPIO pin connected to the RS-485 transceiver's
    ///   DE (Driver Enable) input. When `None`, DE pin control is skipped
    ///   (useful for loopback testing or direct UART connections).
    pub fn new(
        uart: Uart<'static, Async>,
        de_pin: Option<AnyPin<'static>>,
    ) -> Self {
        let de = de_pin.map(|pin| {
            Output::new(pin, Level::Low, OutputConfig::default())
        });
        Rs485Transport { uart, de_pin: de }
    }
}

impl Transport for Rs485Transport {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        self.uart.read(buf).map_err(|_e| {
            TransportError::Io
        })
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), TransportError> {
        // Assert DE pin for transmit
        if let Some(ref mut de) = self.de_pin {
            de.set_high();
            Timer::after(Duration::from_micros(50)).await;
        }

        // Write all bytes in a single DE assertion window
        let mut written = 0;
        while written < data.len() {
            let n = self.uart.write(&data[written..]).map_err(|_e| {
                TransportError::Io
            })?;
            written += n;
        }

        // Flush TX FIFO + shift register to ensure all bytes are on the wire
        // before releasing DE pin. esp-hal flush() blocks until TX is complete.
        let flush_result = self.uart.flush();

        // Always release DE pin: on success, TX is confirmed complete; on
        // failure, a safety delay gives the hardware shift register time to
        // finish draining before we drop DE.
        if let Some(ref mut de) = self.de_pin {
            if flush_result.is_err() {
                warn!("UART flush failed — safety delay before releasing DE pin");
                // 1 ms is enough for the shift register to drain at any
                // practical baud rate (≈100 bit-times at 115200).
                Timer::after(Duration::from_millis(1)).await;
            }
            de.set_low();
        }

        if let Err(_e) = flush_result {
            return Err(TransportError::Io);
        }

        trace!("UART wrote all {} bytes", data.len());
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), TransportError> {
        self.uart.flush().map_err(|_| TransportError::Io)
    }
}
