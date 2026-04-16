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
use log::trace;

pub struct Rs485Transport {
    uart: Uart<'static, Async>,
    de_pin: Option<Output<'static>>,
}

impl Rs485Transport {
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
        let _ = self.uart.flush();

        if let Some(ref mut de) = self.de_pin {
            de.set_low();
        }

        trace!("UART wrote all {} bytes", data.len());
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), TransportError> {
        self.uart.flush().map_err(|_| TransportError::Io)
    }
}
