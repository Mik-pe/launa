//! WiFi connectivity using esp-radio + embassy-net.

extern crate alloc;

use alloc::format;
use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources, Config as NetConfig, Stack};
use embassy_time::{Duration, Timer};
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    Config as WifiConfig,
    ControllerConfig,
    Interface,
    WifiController,
    sta::StationConfig,
};
use log::{info, warn};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

pub struct WifiStack {
    pub stack: &'static Stack<'static>,
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(info) => {
                info!("WiFi connected: {:?}", info);
                let info = controller.wait_for_disconnect_async().await.ok();
                warn!("WiFi disconnected: {:?}", info);
            }
            Err(e) => {
                warn!("WiFi connect failed: {:?}", e);
            }
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await;
}

impl WifiStack {
    pub async fn connect(
        spawner: Spawner,
        wifi_peripheral: esp_hal::peripherals::WIFI,
        rng: Rng,
        ssid: &str,
        password: &str,
    ) -> Self {
        let station_config = WifiConfig::Station(
            StationConfig::default()
                .with_ssid(ssid)
                .with_password(password.into()),
        );

        info!("Starting WiFi...");
        let (controller, interfaces) = esp_radio::wifi::new(
            wifi_peripheral,
            ControllerConfig::default().with_initial_config(station_config),
        )
        .expect("Failed to create WiFi");

        let wifi_interface = interfaces.station;

        let net_config = NetConfig::dhcpv4(Default::default());
        let seed = (rng.random() as u64) << 32 | rng.random() as u64;

        let (stack, runner) = embassy_net::new(
            wifi_interface,
            net_config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
            seed,
        );

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
