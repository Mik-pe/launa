//! WiFi connectivity using esp-idf-svc.

use anyhow::{bail, Context, Result};
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use log::info;

pub fn connect_wifi(
    ssid: &str,
    password: &str,
    sys_event_loop: &EspSystemEventLoop,
    nvs: EspDefaultNvs,
) -> Result<BlockingWifi<EspWifi<'static>>> {
    let peripherals = Peripherals::take().context("Peripherals already taken")?;

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_event_loop.clone(), Some(nvs))
            .context("Failed to create EspWifi")?,
        sys_event_loop.clone(),
    )
    .context("Failed to create BlockingWifi")?;

    let wifi_config = Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().context("SSID too long")?,
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: password.try_into().context("Password too long")?,
        channel: None,
    });

    wifi.set_configuration(&wifi_config)
        .context("Failed to set WiFi configuration")?;

    info!("Connecting to WiFi SSID: {}", ssid);

    wifi.start().context("Failed to start WiFi")?;
    wifi.connect().context("Failed to connect to WiFi")?;
    wifi.wait_netif_up().context("WiFi netif failed to come up")?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()
        .context("Failed to get IP info")?;
    info!("WiFi connected! IP: {}", ip_info.ip);

    Ok(wifi)
}
