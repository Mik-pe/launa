//! WiFi connectivity using esp-radio + embassy-net.
//!
//! All tasks (net_task, connection_task, mqtt_task) run on the same
//! ThreadModeExecutor. This avoids RefCell panics in embassy-net's Stack:
//! the `Runner::run()` poll loop and `TcpSocket` operations both call
//! `stack.with_mut()` → `RefCell::borrow_mut()`. On a single executor
//! these calls are cooperative (never concurrent), so the RefCell is
//! never double-borrowed.

extern crate alloc;

use core::sync::atomic::{AtomicI32, Ordering};

use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, DhcpConfig, Runner, Stack, StackResources};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    sta::StationConfig, Config as WifiConfig, ControllerConfig, Interface, WifiController,
};
use log::{error, info, warn};

use crate::mk_static;
use crate::WIFI_RECONNECT_SIGNAL;

/// Last known WiFi RSSI in dBm, updated by `connection_task` every second.
///
/// A value of `i32::MIN` means no RSSI reading is available yet (not connected).
/// Read from the main loop to include in MQTT state payloads.
pub static WIFI_RSSI: AtomicI32 = AtomicI32::new(i32::MIN);

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
                    // Read RSSI while connected (updates every ~1s).
                    if let Ok(rssi) = controller.rssi() {
                        WIFI_RSSI.store(rssi, Ordering::Relaxed);
                    }
                    Timer::after(Duration::from_secs(1)).await;
                }
                WIFI_RSSI.store(i32::MIN, Ordering::Relaxed);
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
    /// Connect to WiFi, wait for DHCP, and return a WifiStack handle.
    ///
    /// Initializes esp-radio WiFi, creates the embassy-net Stack + Runner,
    /// spawns `connection_task` and `net_task` on the ThreadModeExecutor,
    /// and waits for DHCP.
    ///
    /// All tasks run on the same executor to avoid RefCell panics in
    /// embassy-net's Stack (see module doc comment).
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

        info!(
            "Starting WiFi... (free heap: {} bytes)",
            esp_alloc::HEAP.free()
        );
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

        // Spawn connection_task on ThreadModeExecutor (manages WiFi lifecycle)
        spawner.spawn(connection_task(controller).map_err(|e| {
            error!("Failed to spawn connection_task: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?);

        // Create the embassy-net Stack + Runner directly (no InterruptExecutor).
        let (stack, runner) = embassy_net::new(
            wifi_interface,
            net_config,
            mk_static!(StackResources<4>, StackResources::<4>::new()),
            seed,
        );
        let stack_ref = mk_static!(Stack<'static>, stack);

        // Spawn net_task on the ThreadModeExecutor alongside everything else.
        spawner.spawn(net_task(runner).map_err(|e| {
            error!("Failed to spawn net_task: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?);

        info!("Waiting for DHCP...");
        stack_ref.wait_config_up().await;

        if let Some(config) = stack_ref.config_v4() {
            info!("Got IP: {}", config.address);
        }

        Ok(WifiStack {
            stack: stack_ref,
        })
    }
}
