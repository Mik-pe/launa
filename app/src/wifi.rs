//! WiFi connectivity using esp-radio + embassy-net.

extern crate alloc;

use alloc::string::String;
use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources, Config as NetConfig, Stack};
use embassy_time::{Duration, Timer};
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    ClientConfig,
    Config as WifiConfig,
    WifiController,
    WifiDevice,
};
use log::{info, warn};

use crate::WIFI_RECONNECT_SIGNAL;
use crate::mk_static;

/// Handle to the embassy-net network stack.
///
/// Provides access to the static `Stack` reference needed for TCP/MQTT
/// connections. Created by `WifiStack::connect()` after WiFi association
/// and DHCP address acquisition succeed.
pub struct WifiStack {
    pub stack: &'static Stack<'static>,
}

/// Embassy task managing WiFi connection lifecycle.
///
/// Handles initial connection, automatic reconnection on disconnect,
/// and signals `WIFI_RECONNECT_SIGNAL` on subsequent reconnections so
/// the MQTT task can force a clean broker reconnect.
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    let mut first_connect = true;
    loop {
        match controller.connect_async().await {
            Ok(()) => {
                info!("WiFi connected");
                // Signal WiFi reconnect so MQTT task can force a clean reconnect
                // Only signal on reconnections, not the initial connect, to avoid
                // racing with MQTT and disconnecting an already-connected session.
                if !first_connect {
                    WIFI_RECONNECT_SIGNAL.signal(());
                }
                first_connect = false;
                loop {
                    if !controller.is_connected().unwrap_or(false) {
                        break;
                    }
                    Timer::after(Duration::from_secs(1)).await;
                }
                warn!("WiFi disconnected");
            }
            Err(e) => {
                warn!("WiFi connect failed: {:?}", e);
            }
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}

/// Embassy task running the embassy-net network stack.
///
/// Must be spawned alongside `connection_task` for the network stack
/// to process packets and manage the TCP/IP stack.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await;
}

impl WifiStack {
    /// Connect to WiFi and wait for DHCP address.
    ///
    /// Initializes the esp-radio WiFi client with the given SSID/password,
    /// spawns the connection management and network stack tasks, and blocks
    /// until a DHCP lease is acquired. Returns a `WifiStack` handle for
    /// creating TCP sockets.
    pub async fn connect(
        spawner: Spawner,
        radio_ctrl: esp_radio::Controller<'static>,
        wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
        rng: Rng,
        ssid: &str,
        password: &str,
    ) -> Self {
        let config = WifiConfig::default();

        info!("Starting WiFi...");
        let radio_ctrl = mk_static!(esp_radio::Controller<'static>, radio_ctrl);
        let (mut controller, interfaces) = esp_radio::wifi::new(
            radio_ctrl,
            wifi_peripheral,
            config,
        )
        .expect("Failed to create WiFi");

        controller
            .set_config(&esp_radio::wifi::ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(String::from(ssid))
                    .with_password(String::from(password)),
            ))
            .expect("Failed to set WiFi config");

        controller.start_async().await.expect("Failed to start WiFi");
        info!("WiFi started, connecting...");

        let wifi_interface = interfaces.sta;

        let net_config = NetConfig::dhcpv4(Default::default());
        let seed = ((rng.random() as u64) << 32) | (rng.random() as u64);

        let (stack, runner) = embassy_net::new(
            wifi_interface,
            net_config,
            mk_static!(StackResources<4>, StackResources::<4>::new()),
            seed,
        );

        let stack = mk_static!(Stack<'static>, stack);

        spawner.spawn(connection_task(controller)).expect("Failed to spawn WiFi connection task");
        spawner.spawn(net_task(runner)).expect("Failed to spawn net task");

        info!("Waiting for DHCP...");
        stack.wait_config_up().await;

        if let Some(config) = stack.config_v4() {
            info!("Got IP: {}", config.address);
        }

        WifiStack { stack }
    }
}
