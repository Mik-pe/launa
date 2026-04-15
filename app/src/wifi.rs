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
            Ok(()) => {
                info!("WiFi connected");
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

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await;
}

impl WifiStack {
    pub async fn connect(
        spawner: Spawner,
        radio_ctrl: esp_radio::Controller<'static>,
        wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
        _rng: Rng,
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
        let seed = 12345;

        let (stack, runner) = embassy_net::new(
            wifi_interface,
            net_config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
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
