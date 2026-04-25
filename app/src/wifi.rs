//! WiFi connectivity using esp-radio + embassy-net.

extern crate alloc;

use embassy_executor::Spawner;
use embassy_net::{DhcpConfig, Runner, StackResources, Config as NetConfig, Stack};
use embassy_time::{Duration, Timer};
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    Config as WifiConfig,
    ControllerConfig,
    Interface,
    WifiController,
    sta::StationConfig,
};
use log::{error, info, warn};

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
    loop {
        match controller.connect_async().await {
            Ok(_info) => {
                info!("WiFi connected");
                // Signal WiFi reconnect so MQTT task can force a clean reconnect.
                // WIFI_RECONNECT_SIGNAL is only consumed on reconnections, not initial.
                WIFI_RECONNECT_SIGNAL.signal(());
                loop {
                    if !controller.is_connected() {
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
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
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
        wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
        rng: Rng,
        ssid: &str,
        password: &str,
        hostname: &str,
    ) -> Result<Self, esp_radio::wifi::WifiError> {
        let station_config = WifiConfig::Station(
            StationConfig::default()
                .with_ssid(ssid)
                .with_password(alloc::string::String::from(password)),
        );

        info!("Starting WiFi... (free heap: {} bytes)", esp_alloc::HEAP.free());
        let (controller, interfaces) = esp_radio::wifi::new(
            wifi_peripheral,
            ControllerConfig::default().with_initial_config(station_config),
        )
        .inspect_err(|e| {
            error!(
                "WiFi init failed: {:?} (free heap: {} bytes)",
                e,
                esp_alloc::HEAP.free()
            );
        })?;

        info!("WiFi started, connecting...");

        let wifi_interface = interfaces.station;

        let mut dhcp_config = DhcpConfig::default();
        // Truncate hostname to 32 bytes (DHCP Option 12 limit).
        let truncated: heapless::String<32> = hostname
            .char_indices()
            .take_while(|(i, _)| *i < 32)
            .map(|(_, c)| c)
            .collect();
        if !truncated.is_empty() {
            dhcp_config.hostname = Some(truncated);
        }
        let net_config = NetConfig::dhcpv4(dhcp_config);
        let seed = ((rng.random() as u64) << 32) | (rng.random() as u64);

        let (stack, runner) = embassy_net::new(
            wifi_interface,
            net_config,
            mk_static!(StackResources<4>, StackResources::<4>::new()),
            seed,
        );

        let stack = mk_static!(Stack<'static>, stack);

        spawner.spawn(connection_task(controller).map_err(|e| {
            error!("Failed to spawn connection_task: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?);
        spawner.spawn(net_task(runner).map_err(|e| {
            error!("Failed to spawn net_task: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?);

        info!("Waiting for DHCP...");
        stack.wait_config_up().await;

        if let Some(config) = stack.config_v4() {
            info!("Got IP: {}", config.address);
        }

        Ok(WifiStack { stack })
    }
}
