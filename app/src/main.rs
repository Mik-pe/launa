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
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write as _;
use launa_protocol::command::Command;
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
use launa_protocol::registration::{RegistrationAction, RegistrationStateMachine};
use launa_protocol::status::StatusUpdate;
use launa_mqtt::topics::TopicBuilder;
use launa_mqtt::state::status_to_json;
use log::{debug, error, info, warn};

mod command_tracker;
mod config;
mod mqtt_client;
mod ota;
mod pump_timer;
mod transport;
mod wifi;

// Heap allocator: 32 KiB
esp_alloc::heap_allocator!(size: 32 * 1024);

// ── Inter-task channels ────────────────────────────────────────────────

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, Frame, 4> = Channel::new();
static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, Command, 4> = Channel::new();
static UART_TX_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();
static STATE_CHANNEL: Channel<CriticalSectionRawMutex, StatusUpdate, 2> = Channel::new();

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

// ── MQTT task (subscribes to commands + publishes state) ──────────────

#[embassy_executor::task]
async fn mqtt_task(mut mqtt: mqtt_client::MqttClient) {
    let cmd_sender = COMMAND_CHANNEL.sender();
    let state_rx = STATE_CHANNEL.receiver();
    let topics = TopicBuilder::new(&mqtt.device_id);
    let cmd_base = topics.command_topic();

    info!("MQTT task started");

    loop {
        // Check for state updates to publish (non-blocking)
        if let Ok(status) = state_rx.try_receive() {
            if let Err(e) = mqtt.publish_state(&status).await {
                warn!("MQTT state publish failed: {:?}", e);
            }
            continue; // Prioritize draining state queue
        }

        // Check for incoming MQTT messages (non-blocking via small timeout)
        match mqtt.recv().await {
            Some((topic, payload)) => {
                debug!("MQTT received: {} ({} bytes)", topic, payload.len());

                // Handle OTA commands
                if mqtt.is_ota_topic(&topic) {
                    if let Some(url) = mqtt_client::MqttClient::parse_ota_url(&payload) {
                        info!("OTA firmware URL: {}", url);
                        ota::perform_ota_update(&url).await;
                    } else {
                        warn!("Invalid OTA payload");
                    }
                    continue;
                }

                // Handle HA status (re-publish discovery when HA restarts)
                if mqtt.is_ha_status_topic(&topic) {
                    let status = core::str::from_utf8(&payload).unwrap_or("");
                    if status == "online" {
                        info!("Home Assistant came online, re-publishing discovery");
                        let _ = mqtt.publish_discovery().await;
                        let _ = mqtt.publish_availability(true).await;
                    }
                    continue;
                }

                // Handle regular commands
                if let Some(cmd) = mqtt_client::parse_command(&cmd_base, &topic, &payload) {
                    info!("MQTT command: {:?}", cmd);
                    cmd_sender.send(cmd).await;
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

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Embassy timer + scheduler init (TIMG0 timer0)
    // On xtensa (ESP32), esp_rtos::start() takes only the timer
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("Launa ESP32 firmware starting...");

    // ── Load config from NVS ────────────────────────────────────────
    let mut nvs = config::AppConfig::open_nvs();
    let app_config = config::AppConfig::load(&mut nvs);
    info!("Config loaded: device_id={}", app_config.device_id);

    // ── Initialize RS-485 UART (UART1, 115200 baud, TX=GPIO17, RX=GPIO16) ──
    let uart_config = esp_hal::uart::Config::default().with_baudrate(115200);
    let uart = esp_hal::uart::Uart::new(peripherals.UART1, uart_config)
        .expect("Failed to create UART")
        .with_tx(peripherals.GPIO17)
        .with_rx(peripherals.GPIO16)
        .into_async();

    let uart_transport = transport::Rs485Transport::new(uart, Some(peripherals.GPIO4.into()));
    info!("RS-485 UART initialized");

    // ── Initialize esp-radio (required before WiFi) ──────────────────
    let radio_ctrl = esp_radio::init().expect("Failed to init esp-radio");

    // ── Connect WiFi ────────────────────────────────────────────────
    let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    let wifi_stack = wifi::WifiStack::connect(
        spawner,
        radio_ctrl,
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

    // Mark firmware as valid (boot successful: WiFi + MQTT connected).
    // If we crash before reaching this point, bootloader auto-rolls back.
    let mut ota = ota::EspOta::new();
    if let Err(e) = ota.mark_valid() {
        warn!("Failed to mark firmware valid: {:?}", e);
    } else {
        info!("Firmware marked valid (boot validation passed)");
    }

    // Spawn background tasks
    spawner
        .spawn(mqtt_task(mqtt))
        .expect("Failed to spawn MQTT task");
    spawner
        .spawn(uart_task(uart_transport))
        .expect("Failed to spawn UART task");

    // ── Main event loop ─────────────────────────────────────────────
    info!("Entering main event loop");

    let frame_rx = FRAME_CHANNEL.receiver();
    let cmd_rx = COMMAND_CHANNEL.receiver();

    let mut registration = RegistrationStateMachine::new();
    let mut pump_timers = pump_timer::PumpTimerManager::new();
    let mut hold_timer = pump_timer::HoldModeTimer::new();
    let mut cmd_tracker = command_tracker::CommandTracker::new();
    let mut last_status: Option<launa_protocol::status::StatusUpdate> = None;

    loop {
        // Wait for either a frame or a command
        let frame_fut = frame_rx.receive();
        let cmd_fut = cmd_rx.receive();
        embassy_futures::select::select(frame_fut, cmd_fut).await;

        // Drain all available frames
        while let Ok(frame) = frame_rx.try_receive() {
            handle_frame(&frame, &mut registration, &mut pump_timers, &mut hold_timer, &mut cmd_tracker, &mut last_status).await;
        }

        // Drain all available commands
        while let Ok(cmd) = cmd_rx.try_receive() {
            if registration.is_registered() {
                let (msg_type, payload) = cmd.encode();
                send_frame(msg_type, &payload).await;
                debug!("Sent command: {:?}", cmd);
                // Track for ACK verification
                if let Some(ref pre_status) = last_status {
                    cmd_tracker.track(cmd.clone(), pre_status);
                }
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
    hold_timer: &mut pump_timer::HoldModeTimer,
    cmd_tracker: &mut command_tracker::CommandTracker,
    last_status: &mut Option<launa_protocol::status::StatusUpdate>,
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

            // Verify pending commands against new status
            let retries = cmd_tracker.verify(&status);
            for cmd in retries {
                let (msg_type, payload) = cmd.encode();
                send_frame(msg_type, &payload).await;
            }

            let expired = pump_timers.tick_all(status.pump1, status.pump2, status.pump3);
            for cmd in expired {
                let (msg_type, payload) = cmd.encode();
                send_frame(msg_type, &payload).await;
            }

            // Hold mode safety timeout
            if let Some(cmd) = hold_timer.tick(status.is_hold) {
                let (msg_type, payload) = cmd.encode();
                send_frame(msg_type, &payload).await;
            }

            // Save for command tracking context
            *last_status = Some(status.clone());

            // Publish state to MQTT via state channel
            STATE_CHANNEL.send(status).await;
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
