use log::info;

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    esp_idf_svc::log::EspLogger::initialize_default();

    info!("Launa spa controller starting...");

    // TODO: Initialize UART/RS485 transport
    // TODO: Connect to WiFi
    // TODO: Connect to MQTT broker
    // TODO: Start Balboa protocol event loop
    // TODO: Publish state to Home Assistant

    info!("Launa initialization complete. Entering main loop.");

    loop {
        // Main event loop placeholder
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
