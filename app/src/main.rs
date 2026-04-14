use anyhow::{Context, Result};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::timer::EspTimerService;
use launa_protocol::command::Command;
use launa_protocol::dispatcher::dispatch_frame;
use launa_protocol::frame::FrameDecoder;
use launa_protocol::registration::{RegistrationAction, RegistrationStateMachine};
use launa_hal::transport::Transport;
use log::{debug, info, warn};

mod config;
mod mqtt_client;
mod ota;
mod pump_timer;
mod transport;
mod wifi;

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();
    EspLogger::initialize_default();

    info!("Launa spa controller starting...");

    // Initialize NVS and load config
    let nvs = config::AppConfig::open_nvs()?;
    let app_config = config::load_or_default(&nvs);

    // Initialize system services
    let sys_event_loop = EspSystemEventLoop::take()?;
    let _timer_service = EspTimerService::new()?;

    // Connect to WiFi
    let mut wifi = wifi::connect_wifi(
        &app_config.wifi_ssid,
        &app_config.wifi_password,
        &sys_event_loop,
        EspDefaultNvs::new(nvs)?,
    )?;

    // Initialize RS-485 UART transport
    let mut uart = transport::Rs485Transport::new(
        app_config.rs485_tx_pin,
        app_config.rs485_rx_pin,
        app_config.rs485_de_pin,
    )?;

    // Create command channel (MQTT -> main loop)
    let (command_tx, command_rx) = std::sync::mpsc::channel::<Command>();

    // Create MQTT client
    let mut mqtt = mqtt_client::create_mqtt_client(
        &app_config.mqtt_host,
        app_config.mqtt_port,
        &app_config.mqtt_user,
        &app_config.mqtt_password,
        &app_config.device_id,
        command_tx,
    )?;

    // Wait for MQTT connection and publish discovery
    std::thread::sleep(std::time::Duration::from_secs(2));
    mqtt_client::publish_discovery(&mut mqtt, &app_config.device_id)?;
    mqtt_client::publish_availability(&mut mqtt, &app_config.device_id, true)?;
    mqtt_client::subscribe_commands(&mut mqtt, &app_config.device_id)?;

    // Initialize protocol state
    let mut frame_decoder = FrameDecoder::new();
    let mut registration = RegistrationStateMachine::new();
    let mut pump_timers = pump_timer::PumpTimerManager::new();

    let mut last_status_time = std::time::Instant::now();
    let mut uart_buf = [0u8; 256];

    info!("Launa initialization complete. Entering main loop.");

    loop {
        // 1. Read from UART
        match uart.read(&mut uart_buf) {
            Ok(0) => {} // No data
            Ok(n) => {
                let frames = frame_decoder.feed_slice(&uart_buf[..n]);
                for frame in &frames {
                    let message = dispatch_frame(frame);

                    // Handle registration
                    if !registration.is_registered() {
                        let action = registration.process(
                            frame.message_type,
                            &frame.payload,
                        );
                        match action {
                            RegistrationAction::SendIdRequest => {
                                let encoded = launa_protocol::frame::FrameEncoder::encode(
                                    [0xFE, 0xBF],
                                    &[0x01, 0x02, 0xF1, 0x73],
                                );
                                let _ = uart.write(&encoded);
                                debug!("Sent registration ID request");
                            }
                            RegistrationAction::SendIdAck { client_id } => {
                                let encoded = launa_protocol::frame::FrameEncoder::encode(
                                    [client_id, 0xBF],
                                    &[0x03],
                                );
                                let _ = uart.write(&encoded);
                                info!("Registered with client ID: {}", client_id);
                            }
                            RegistrationAction::None => {}
                        }
                        continue;
                    }

                    // Handle incoming messages
                    match message {
                        launa_protocol::dispatcher::IncomingMessage::StatusUpdate(status) => {
                            // Publish state to MQTT
                            if let Err(e) = mqtt_client::publish_state(
                                &mut mqtt,
                                &app_config.device_id,
                                &status,
                            ) {
                                warn!("Failed to publish state: {:?}", e);
                            }

                            // Tick pump timers and send auto-off commands
                            let expired_commands = pump_timers.tick_all(
                                status.pump1,
                                status.pump2,
                                status.pump3,
                            );
                            for cmd in expired_commands {
                                send_command(&mut uart, &registration, &cmd);
                            }

                            last_status_time = std::time::Instant::now();
                        }
                        launa_protocol::dispatcher::IncomingMessage::Ready => {
                            // Process any pending commands from MQTT
                            while let Ok(cmd) = command_rx.try_recv() {
                                send_command(&mut uart, &registration, &cmd);
                            }
                        }
                        launa_protocol::dispatcher::IncomingMessage::NewClientQuery => {
                            // Re-registration may be needed
                        }
                        _ => {
                            debug!("Unhandled message: {:?}", message);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("UART read error: {:?}", e);
            }
        }

        // 2. Check for MQTT commands (non-blocking) even without Ready message
        //    Commands will be buffered and sent on next Ready
        while let Ok(cmd) = command_rx.try_recv() {
            debug!("Queued command from MQTT: {:?}", cmd);
            send_command(&mut uart, &registration, &cmd);
        }

        // 3. Small sleep to prevent busy-waiting
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn send_command(
    uart: &mut dyn Transport,
    registration: &RegistrationStateMachine,
    cmd: &Command,
) {
    let client_id = match registration.client_id() {
        Some(id) => id,
        None => {
            warn!("Cannot send command: not registered");
            return;
        }
    };

    let (msg_type, payload) = cmd.encode();

    // For commands that use the client ID as first byte of message type
    let actual_type = match cmd {
        Command::NothingToSend { .. } => msg_type,
        _ => msg_type,
    };

    let encoded = launa_protocol::frame::FrameEncoder::encode(actual_type, &payload);
    match uart.write(&encoded) {
        Ok(()) => debug!("Sent command: {:?}", cmd),
        Err(e) => warn!("Failed to send command: {:?}", e),
    }
}
