//! WiFi initialization for ESP32 app crates.

use alloc::string::String;

use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, DhcpConfig, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_hal::rng::Rng;
use log::{info, warn};

use crate::mk_static;

/// Embassy task managing WiFi connection lifecycle.
#[embassy_executor::task]
async fn connection_task(mut controller: esp_radio::wifi::WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(_info) => {
                info!("WiFi connected");
                while controller.is_connected() {
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
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, esp_radio::wifi::Interface<'static>>) {
    runner.run().await;
}

/// Connect to WiFi, wait for DHCP, return network stack.
///
/// `hostname` is the DHCP hostname to advertise (e.g. "launa-sniffer").
/// `wifi_ssid` and `wifi_password` are the WiFi credentials.
pub fn wifi_init(
    spawner: Spawner,
    wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
    rng: Rng,
    wifi_ssid: &str,
    wifi_password: &str,
    hostname: &str,
) -> &'static Stack<'static> {
    let station_config = esp_radio::wifi::Config::Station(
        esp_radio::wifi::sta::StationConfig::default()
            .with_ssid(wifi_ssid)
            .with_password(String::from(wifi_password)),
    );

    info!("Starting WiFi...");
    let (controller, interfaces) = esp_radio::wifi::new(
        wifi_peripheral,
        esp_radio::wifi::ControllerConfig::default().with_initial_config(station_config),
    )
    .expect("WiFi init failed");

    info!("WiFi started, connecting...");

    let wifi_interface = interfaces.station;

    let mut dhcp_config = DhcpConfig::default();
    let hostname_val: heapless::String<32> = hostname.parse().unwrap();
    dhcp_config.hostname = Some(hostname_val);
    let net_config = NetConfig::dhcpv4(dhcp_config);
    let seed = ((rng.random() as u64) << 32) | (rng.random() as u64);

    spawner.spawn(connection_task(controller).unwrap());
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        net_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );
    let stack_ref = mk_static!(Stack<'static>, stack);
    spawner.spawn(net_task(runner).unwrap());

    stack_ref
}
