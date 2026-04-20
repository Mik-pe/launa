use std::path::PathBuf;

pub mod broker;
pub mod web;

#[derive(Debug, Clone)]
pub struct Config {
    pub mqtt_tcp_port: u16,
    pub mqtt_ws_port: u16,
    pub http_port: u16,
    pub web_dir: PathBuf,
}

/// JSON-serializable accessory visibility config served to the web UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccessoryConfig {
    pub pumps: u8,
    pub lights: u8,
    pub blower: bool,
    pub mister: bool,
}

pub fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut broker = broker::build(&config)?;

    // Start web server on a background thread
    let web_config = config.clone();
    std::thread::spawn(move || {
        if let Err(e) = web::start(&web_config) {
            eprintln!("Web server error: {e}");
        }
    });

    // Start MQTT broker listeners (blocks forever)
    broker.start()?;
    Ok(())
}
