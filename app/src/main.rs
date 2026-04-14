//! Launa ESP32 spa controller firmware.
//!
//! Embassy-based async runtime over esp-hal (pure Rust, no_std).
//! Reads Balboa spa protocol over RS-485 UART, publishes state to
//! Home Assistant via MQTT over WiFi.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write as _;
use launa_protocol::command::Command;
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
use launa_protocol::registration::{RegistrationAction, RegistrationStateMachine};
use launa_mqtt::topics::TopicBuilder;
use log::{debug, error, info, warn};

mod config;
mod mqtt_client;
mod ota;
mod pump_timer;
mod transport;
mod wifi;

// Heap allocator: 32 KiB
esp_alloc::heap_allocator!(size: 32 * 1024);

// ── Inter-task channels ────────────────────────────────────────────────

static FRAME_CHANNEL: Channel<embassy_sync::blocking_mutex::raw::NoopRawMutex, Frame, 4> =
    Channel::new();
static COMMAND_CHANNEL: Channel<embassy_sync::blocking_mutex::raw::NoopRawMutex, Command, 4> =
    Channel::new();
static UART_TX_CHANNEL: Channel<embassy_sync::blocking_mutex::raw::NoopRawMutex, Vec<u8>, 4> =
    Channel::new();

// ── Combined UART task (reads frames + writes outgoing bytes) ──────────

#[embassy_executor::task]
async fn uart_task(mut transport: transport::Rs485Transport) {
    let mut decoder = FrameDecoder::new();
    let frame_sender = FRAME_CHANNEL.sender();
    let uart_rx = UART_TX_CHANNEL.receiver();
    let mut buf = [0u8; 128];

    info!("UART task started");

    loop {
        // Check for outgoing data first (prioritize writes)
        if let Ok(data) = uart_rx.try_receive() {
            if let Err(e) = transport.write_all(&data).await {
                error!("UART write error: {:?}", e);
            }
        }

        // Read from UART
        match transport.read(&mut buf).await {
            Ok(n) if n > 0 => {
                for &byte in &buf[..n] {
                    if let Some(frame) = decoder.feed(byte) {
                        frame_sender.send(frame).await;
                    }
                }
            }
            Ok(_) => {
                Timer::after(Duration::from_millis(1)).await;
            }
            Err(e) => {
                error!("UART read error: {:?}", e);
                Timer::after(Duration::from_millis(10)).await;
            }
        }
    }
}

// ── MQTT subscriber task ──────────────────────────────────────────────

#[embassy_executor::task]
async fn mqtt_subscriber_task(mut mqtt: mqtt_client::MqttClient) {
    let sender = COMMAND_CHANNEL.sender();
    let topics = TopicBuilder::new(&mqtt.device_id);
    let cmd_base = topics.command_topic();

    info!("MQTT subscriber task started");

    loop {
        match mqtt.recv().await {
            Some((topic, payload)) => {
                debug!("MQTT received: {} ({} bytes)", topic, payload.len());
                if let Some(cmd) = mqtt_client::parse_command(&cmd_base, &topic, &payload) {
                    info!("MQTT command: {:?}", cmd);
                    sender.send(cmd).await;
                }
            }
            None => {
                warn!("MQTT connection lost");
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

async fn send_frame(msg_type: [u8; 2], payload: &[u8]) {
    let encoded = FrameEncoder::encode(msg_type, payload);
    UART_TX_CHANNEL.send(encoded).await;
}

// ── Main entry point ──────────────────────────────────────────────────

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Embassy timer init (TIMG0 timer0)
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    info!("Launa ESP32 firmware starting...");

    // ── Load config from NVS ────────────────────────────────────────
    let flash = esp_storage::FlashStorage::new();
    let mut nvs = config::AppConfig::open_nvs(flash);
    let app_config = config::AppConfig::load(&mut nvs);
    info!("Config loaded: device_id={}", app_config.device_id);

    // ── Initialize RS-485 UART (UART1, 115200 baud, TX=GPIO17, RX=GPIO16) ──
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    let uart_transport = transport::Rs485Transport::new(uart, Some(peripherals.GPIO4));
    info!("RS-485 UART initialized");

    // ── Connect WiFi ────────────────────────────────────────────────
    let rng = esp_hal::rng::Rng::new();
    let wifi_stack = wifi::WifiStack::connect(
        spawner,
        peripherals.WIFI,
        rng,
        &app_config.wifi_ssid,
        &app_config.wifi_password,
    )
    .await;

    // ── Connect MQTT ────────────────────────────────────────────────
    let mut mqtt = match mqtt_client::MqttClient::connect(wifi_stack.stack, &app_config).await {
        Ok(m) => m,
        Err(e) => {
            error!("MQTT connect failed: {:?}", e);
            panic!("MQTT connect failed")
        }
    };

    // Publish availability + discovery + subscribe
    let _ = mqtt.publish_availability(true).await;
    let _ = mqtt.publish_discovery().await;
    let _ = mqtt.subscribe_commands().await;

    // Spawn background tasks
    spawner
        .spawn(mqtt_subscriber_task(mqtt))
        .expect("Failed to spawn MQTT subscriber");
    spawner
        .spawn(uart_task(uart_transport))
        .expect("Failed to spawn UART task");

    // ── Main event loop ─────────────────────────────────────────────
    info!("Entering main event loop");

    let frame_rx = FRAME_CHANNEL.receiver();
    let cmd_rx = COMMAND_CHANNEL.receiver();

    let mut registration = RegistrationStateMachine::new();
    let mut pump_timers = pump_timer::PumpTimerManager::new();

    loop {
        // Wait for either a frame or a command
        let frame_fut = frame_rx.receive();
        let cmd_fut = cmd_rx.receive();
        embassy_futures::select::select(frame_fut, cmd_fut).await;

        // Drain all available frames
        while let Ok(frame) = frame_rx.try_receive() {
            handle_frame(&frame, &mut registration, &mut pump_timers).await;
        }

        // Drain all available commands
        while let Ok(cmd) = cmd_rx.try_receive() {
            if registration.is_registered() {
                let (msg_type, payload) = cmd.encode();
                send_frame(msg_type, &payload).await;
                debug!("Sent command: {:?}", cmd);
            } else {
                warn!("Cannot send command: not registered");
            }
        }
    }
}

async fn handle_frame(
    frame: &Frame,
    registration: &mut RegistrationStateMachine,
    pump_timers: &mut pump_timer::PumpTimerManager,
) {
    // ── Registration ────────────────────────────────────────────────
    if !registration.is_registered() {
        let action = registration.process(frame.message_type, &frame.payload);
        match action {
            RegistrationAction::SendIdRequest => {
                send_frame([0xFE, 0xBF], &[0x01, 0x02, 0xF1, 0x73]).await;
                debug!("Sent registration ID request");
            }
            RegistrationAction::SendIdAck { client_id } => {
                send_frame([client_id, 0xBF], &[0x03]).await;
                info!("Registered with client ID: {}", client_id);
            }
            RegistrationAction::None => {}
        }
        return;
    }

    // ── Dispatch incoming message ───────────────────────────────────
    let message = dispatch_frame(frame);

    match message {
        IncomingMessage::StatusUpdate(status) => {
            debug!(
                "Status: temp={:?} set={:.0} heating={}",
                status.current_temp, status.set_temp, status.is_heating
            );

            let expired = pump_timers.tick_all(status.pump1, status.pump2, status.pump3);
            for cmd in expired {
                let (msg_type, payload) = cmd.encode();
                send_frame(msg_type, &payload).await;
            }
            // TODO: Publish state to MQTT via a state channel
        }
        IncomingMessage::Ready => {
            debug!("Spa ready");
        }
        IncomingMessage::NewClientQuery => {
            debug!("New client query -- may need re-registration");
        }
        IncomingMessage::ClientIdAssignment { id } => {
            info!("Client ID assigned: {}", id);
        }
        IncomingMessage::ConfigurationResponse(_) => {
            info!("Spa configuration received");
        }
        IncomingMessage::InformationResponse(_) => {
            info!("Information response received");
        }
        IncomingMessage::FaultLogResponse(_) => {
            info!("Fault log response received");
        }
        IncomingMessage::FilterCyclesResponse(_) => {
            info!("Filter cycles response received");
        }
        IncomingMessage::ControlConfiguration(_) => {
            info!("Control configuration received");
        }
        IncomingMessage::Unknown { message_type, .. } => {
            debug!("Unknown message: {:02X?}", message_type);
        }
    }
}
