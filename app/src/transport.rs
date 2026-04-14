//! RS-485 UART transport for Balboa spa communication.
//!
//! Implements `launa_hal::Transport` using `esp-idf-hal` UART with a GPIO
//! direction pin for the RS-485 transceiver.

use anyhow::{Context, Result};
use esp_idf_hal::gpio::{Output, OutputPin, PinDriver};
use esp_idf_hal::prelude::Peripherals;
use esp_idf_hal::uart::{self, UartConfig, UartDriver};
use launa_hal::transport::{Transport, TransportError};
use log::{debug, trace};
use std::time::Duration;

pub struct Rs485Transport {
    uart: UartDriver<'static>,
    de_pin: PinDriver<'static, esp_idf_hal::gpio::AnyOutputPin, Output>,
}

impl Rs485Transport {
    pub fn new(
        tx_pin: i32,
        rx_pin: i32,
        de_pin: i32,
    ) -> Result<Self> {
        let peripherals = Peripherals::take().context("Peripherals already taken")?;

        let config = UartConfig::new()
            .baudrate(115200)
            .data_bits(uart::DataBits::DataBits8)
            .parity(uart::Parity::ParityNone)
            .stop_bits(uart::StopBits::STOP1);

        let uart = UartDriver::new(
            peripherals.uart1,
            peripherals.pins.gpio(tx_pin),
            peripherals.pins.gpio(rx_pin),
            Option::<esp_idf_hal::gpio::Gpio0>::None,
            Option::<esp_idf_hal::gpio::Gpio0>::None,
            &config,
        )
        .context("Failed to create UART driver")?;

        let de = PinDriver::output(peripherals.pins.gpio(de_pin).downgrade())
            .context("Failed to create DE pin driver")?;

        Ok(Rs485Transport {
            uart,
            de_pin: de,
        })
    }
}

impl Transport for Rs485Transport {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        match self.uart.read(buf, Duration::from_millis(100)) {
            Ok(n) => {
                if n > 0 {
                    trace!("UART read {} bytes", n);
                }
                Ok(n)
            }
            Err(e) => {
                debug!("UART read error: {:?}", e);
                Err(TransportError::Io)
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<(), TransportError> {
        // Assert DE pin high for transmit
        self.de_pin.set_high().map_err(|_| TransportError::Io)?;

        // Small delay to let RS-485 transceiver switch direction
        std::thread::sleep(Duration::from_micros(50));

        let result = self
            .uart
            .write(data)
            .map(|_| ())
            .map_err(|_| TransportError::Io);

        // Flush to ensure all bytes are sent before releasing DE
        let _ = self.uart.flush();

        // Release DE pin back to receive mode
        self.de_pin.set_low().map_err(|_| TransportError::Io)?;

        trace!("UART wrote {} bytes", data.len());
        result
    }

    fn flush(&mut self) -> Result<(), TransportError> {
        self.uart.flush().map_err(|_| TransportError::Io)
    }
}
