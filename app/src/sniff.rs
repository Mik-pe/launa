//! RS-485 sniffer mode for passive frame capture.
//!
//! When the `sniff` feature is enabled, the firmware boots into sniffer mode
//! which passively listens on the RS-485 bus and publishes raw frame data
//! to MQTT for protocol analysis and debugging.

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_mqtt::topics::TopicBuilder;
use log::{error, info, warn};

use crate::*;

/// Maximum hex output buffer size for sniffer frame payloads.
const HEX_BUF_SIZE: usize = 512;

/// Fixed-size stack buffer implementing `core::fmt::Write`.
/// Used by `bytes_to_hex()` to format hex without heap allocation.
struct HexBuf {
    data: [u8; HEX_BUF_SIZE],
    len: usize,
}

impl HexBuf {
    const fn new() -> Self {
        HexBuf {
            data: [0u8; HEX_BUF_SIZE],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        // SAFETY: Only ASCII hex characters (0-9, A-F) are written via
        // core::fmt::UpperHex, which is guaranteed to produce valid UTF-8.
        unsafe { core::str::from_utf8_unchecked(&self.data[..self.len]) }
    }

    fn remaining(&self) -> usize {
        HEX_BUF_SIZE - self.len
    }
}

impl core::fmt::Write for HexBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        if bytes.len() > self.remaining() {
            return Err(core::fmt::Error);
        }
        self.data[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

/// Format a byte slice as uppercase hex using `write!()` into a pre-allocated
/// fixed-size stack buffer.
///
/// Returns the hex string. If the payload exceeds `HEX_BUF_SIZE / 2` bytes,
/// the output is silently truncated (no panic, no heap allocation).
///
/// This replaces per-byte `alloc::format!("{:02X}").collect()` which causes O(n)
/// heap allocations on a 32 KiB ESP32 heap.
fn bytes_to_hex<'a>(bytes: &[u8], buf: &'a mut HexBuf) -> &'a str {
    buf.len = 0; // Reset buffer
    for &b in bytes {
        if buf.remaining() < 2 {
            break; // Truncate rather than overflow
        }
        // SAFETY: write!() into our fixed-size HexBuf. Each call appends
        // exactly 2 hex characters. We checked remaining >= 2 above.
        let _ = core::fmt::write(buf, format_args!("{:02X}", b));
    }
    buf.as_str()
}

#[esp_rtos::main]
pub(crate) async fn main(spawner: Spawner) {
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let config = esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    info!("Launa ESP32 sniffer mode starting...");

    let app_config = match config::AppConfig::open_nvs(peripherals.FLASH) {
        Some(mut nvs) => {
            let mut aes = esp_hal::aes::Aes::new(peripherals.AES);
            let mut rng = esp_hal::rng::Rng::new();
            config::AppConfig::load(&mut nvs, &mut aes, &mut rng)
        }
        None => {
            warn!("NVS unavailable — using default config");
            config::AppConfig::default()
        }
    };
    let device_id = app_config.device_id.clone();
    info!("Config loaded: device_id={}", device_id);

    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("Failed to create UART")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    let mut transport = transport::Rs485Transport::new(uart, Some(peripherals.GPIO4.into()));
    info!("RS-485 UART initialized");

    let wifi_stack = wifi::WifiStack::connect(
        spawner,
        peripherals.WIFI,
        esp_hal::rng::Rng::new(),
        &app_config.wifi_ssid,
        &app_config.wifi_password,
    )
    .await;

    let mut mqtt = {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            match mqtt_client::MqttClient::connect(wifi_stack.stack, &app_config).await {
                Ok(m) => break m,
                Err(e) => {
                    let backoff_secs = (5u64 << attempt.saturating_sub(1).min(4)).min(60);
                    error!(
                        "MQTT connect attempt {} failed: {:?}, retrying in {}s",
                        attempt, e, backoff_secs
                    );
                    Timer::after(Duration::from_secs(backoff_secs)).await;
                    if attempt >= 10 {
                        error!("MQTT connect failed after 10 attempts, resetting");
                        esp_hal::system::software_reset();
                    }
                }
            }
        }
    };

    let _ = mqtt.publish_availability(true).await;
    let _ = mqtt.subscribe_commands().await;

    let topics = TopicBuilder::new(&device_id);
    let sniff_topic = topics.sniff_topic();

    info!("Sniffer mode active - listening passively on RS-485");

    let mut decoder = FrameDecoder::new();
    let mut buf = [0u8; 256];
    let mut hex_buf = HexBuf::new();

    loop {
        match transport.read(&mut buf).await {
            Ok(n) if n > 0 => {
                let frames = decoder.feed_slice(&buf[..n]);
                for frame in &frames {
                    let hex = bytes_to_hex(&frame.payload, &mut hex_buf);
                    let mt = alloc::format!(
                        "{:02X}{:02X}",
                        frame.message_type[0], frame.message_type[1]
                    );

                    // Re-parse to get CRC status
                    let crc_ok = Frame::parse(&frame.payload).is_ok();

                    let json = alloc::format!(
                        r#"{{"raw":"{}","type":"{}","len":{},"crc_ok":{}}}"#,
                        hex,
                        mt,
                        frame.payload.len(),
                        crc_ok
                    );
                    info!("Sniff: {}", json);
                    let _ = mqtt.publish(&sniff_topic, json.as_bytes(), 0, false).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!("Sniffer read error: {:?}", e);
                Timer::after(Duration::from_millis(100)).await;
            }
        }
    }
}
