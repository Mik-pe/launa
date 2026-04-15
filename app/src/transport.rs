//! RS-485 UART transport using esp-hal async UART.

use embedded_io_async::{self, Read, Write, ErrorType};
use embassy_time::{Duration, Timer};
use esp_hal::gpio::{AnyPin, Output, OutputConfig, Level};
use esp_hal::uart::Uart;
use esp_hal::Async;
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

#[derive(Debug)]
pub struct TransportError;

impl embedded_io_async::Error for TransportError {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        embedded_io_async::ErrorKind::Other
    }
}

impl ErrorType for Rs485Transport {
    type Error = TransportError;
}

impl Read for Rs485Transport {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.uart.read(buf).map_err(|e| {
            log::warn!("UART read error: {:?}", e);
            TransportError
        })
    }
}

impl Write for Rs485Transport {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        // Assert DE pin for transmit
        if let Some(ref mut de) = self.de_pin {
            de.set_high();
            Timer::after(Duration::from_micros(50)).await;
        }

        let n = self.uart.write(buf).map_err(|e| {
            log::warn!("UART write error: {:?}", e);
            TransportError
        })?;

        // Flush TX FIFO + shift register to ensure all bytes are on the wire
        // before releasing DE pin. esp-hal flush() blocks until TX is complete.
        let _ = self.uart.flush();

        if let Some(ref mut de) = self.de_pin {
            de.set_low();
        }

        trace!("UART wrote {} bytes", n);
        Ok(n)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.uart.flush().map_err(|_| TransportError)
    }

    // Override default write_all to keep DE pin asserted for the entire
    // operation, avoiding glitches on the RS-485 bus from per-chunk toggle.
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        if let Some(ref mut de) = self.de_pin {
            de.set_high();
            Timer::after(Duration::from_micros(50)).await;
        }

        let mut written = 0;
        while written < buf.len() {
            let n = self.uart.write(&buf[written..]).map_err(|e| {
                log::warn!("UART write error at offset {}: {:?}", written, e);
                TransportError
            })?;
            written += n;
        }

        // Flush TX FIFO + shift register before releasing DE
        let _ = self.uart.flush();

        if let Some(ref mut de) = self.de_pin {
            de.set_low();
        }

        trace!("UART wrote all {} bytes", buf.len());
        Ok(())
    }
}
