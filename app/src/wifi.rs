//! WiFi connectivity using esp-radio + embassy-net.
//!
//! Network and MQTT tasks run on an InterruptExecutor (SWI 1, Priority1)
//! so they preempt the ThreadModeExecutor used by the main loop. This
//! prevents the smoltcp poll loop (net_task) from being starved by
//! frame processing or UART reads.

extern crate alloc;

use core::sync::atomic::{AtomicI32, Ordering};

use embassy_executor::{SendSpawner, Spawner};
use embassy_net::{DhcpConfig, Runner, StackResources, Config as NetConfig, Stack};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use esp_hal::interrupt::software::SoftwareInterrupt;
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

/// Signal from net_bootstrap to WifiStack::connect() indicating the Stack
/// has been created and STACK_PTR is valid.
static STACK_READY_SIGNAL: Signal<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, ()> = Signal::new();

/// Stack pointer set by net_bootstrap, read by WifiStack::connect().
/// Uses AtomicUsize because raw pointers are not Send/Sync.
static STACK_PTR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// MQTT configuration arguments passed to `net_bootstrap` for connecting
/// to the MQTT broker from within the InterruptExecutor context.
pub(crate) struct MqttConfigArgs {
    pub device_id: alloc::string::String,
    pub mqtt_host: alloc::string::String,
    pub mqtt_port: u16,
    pub mqtt_user: alloc::string::String,
    pub mqtt_password: alloc::string::String,
    pub boot_id: u32,
}

/// Arguments passed to the `net_bootstrap` task running on the InterruptExecutor.
struct NetBootstrapArgs {
    interface: Interface<'static>,
    net_config: NetConfig,
    seed: u64,
    mqtt_config: MqttConfigArgs,
}

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

/// Bootstrap task for the InterruptExecutor.
///
/// Creates the embassy-net Stack + Runner, signals the Stack reference back
/// to WifiStack::connect(), spawns net_task, connects MQTT, and spawns
/// mqtt_task — all on the InterruptExecutor so they preempt ThreadModeExecutor.
#[embassy_executor::task]
async fn net_bootstrap(args: NetBootstrapArgs) {
    let (stack, runner) = embassy_net::new(
        args.interface,
        args.net_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        args.seed,
    );
    let stack_ref = mk_static!(Stack<'static>, stack);

    // Signal the Stack reference back to WifiStack::connect().
    STACK_PTR.store(stack_ref as *const Stack<'static> as usize, core::sync::atomic::Ordering::Release);
    STACK_READY_SIGNAL.signal(());

    // Get a Spawner for this executor so we can spawn more tasks here.
    // SAFETY: we are inside an embassy task on this executor.
    let spawner = unsafe { Spawner::for_current_executor() }.await;
    spawner.spawn(net_task(runner).unwrap());

    // Connect MQTT inside the InterruptExecutor context, with retries.
    let mqtt_config = crate::config::AppConfig {
        device_id: args.mqtt_config.device_id,
        wifi_ssid: alloc::string::String::new(),
        wifi_password: alloc::string::String::new(),
        mqtt_host: args.mqtt_config.mqtt_host,
        mqtt_port: args.mqtt_config.mqtt_port,
        mqtt_user: args.mqtt_config.mqtt_user,
        mqtt_password: args.mqtt_config.mqtt_password,
        self_test: false,
    };
    let mut mqtt = {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match crate::mqtt_client::MqttClient::connect(
                stack_ref, &mqtt_config, args.mqtt_config.boot_id,
            ).await {
                Ok(m) => break m,
                Err(e) => {
                    let backoff = launa_core::network::backoff_secs(attempt);
                    error!(
                        "MQTT connect attempt {} failed: {:?}, retrying in {}s",
                        attempt, e, backoff
                    );
                    if attempt >= 10 {
                        error!("MQTT connect failed after {} attempts, resetting", attempt);
                        esp_hal::system::software_reset();
                    }
                    Timer::after(Duration::from_secs(backoff)).await;
                }
            }
        }
    };
    if let Err(e) = mqtt.post_connect_publish(false).await {
        warn!("Post-connect publish failed: {:?}", e);
    }
    spawner.spawn(crate::mqtt_task::mqtt_task(mqtt).unwrap());
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
    /// spawns the connection management task on the ThreadModeExecutor,
    /// starts an InterruptExecutor for net_task + MQTT, and blocks until
    /// a DHCP lease is acquired. Returns a `WifiStack` handle for
    /// creating TCP sockets (used by OTA in the main loop).
    pub async fn connect(
        spawner: Spawner,
        wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
        sw_int1: SoftwareInterrupt<'static, 1>,
        rng: Rng,
        ssid: &str,
        password: &str,
        hostname: &str,
        mqtt_config: MqttConfigArgs,
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

        // Spawn connection_task on ThreadModeExecutor (manages WiFi lifecycle)
        spawner.spawn(connection_task(controller).map_err(|e| {
            error!("Failed to spawn connection_task: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?);

        // Start InterruptExecutor for net + MQTT tasks (preempts ThreadModeExecutor)
        let net_executor = mk_static!(
            esp_rtos::embassy::InterruptExecutor<1>,
            esp_rtos::embassy::InterruptExecutor::new(sw_int1)
        );
        let send_spawner: SendSpawner = net_executor.start(esp_hal::interrupt::Priority::Priority1);
        send_spawner.spawn(net_bootstrap(NetBootstrapArgs {
            interface: wifi_interface,
            net_config,
            seed,
            mqtt_config,
        }).map_err(|e| {
            error!("Failed to spawn net_bootstrap: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?);

        // Wait for net_bootstrap to create the Stack and signal it back
        STACK_READY_SIGNAL.wait().await;
        // SAFETY: net_bootstrap stored a valid &'static Stack pointer.
        let stack = unsafe { &*(STACK_PTR.load(core::sync::atomic::Ordering::Acquire) as *const Stack<'static>) };

        info!("Waiting for DHCP...");
        stack.wait_config_up().await;

        if let Some(config) = stack.config_v4() {
            info!("Got IP: {}", config.address);
        }

        Ok(WifiStack { stack })
    }
}
