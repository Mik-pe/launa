use std::path::PathBuf;
use std::sync::Arc;

pub mod broker;
pub mod db;
pub mod mqtt_bridge;
pub mod web;

#[derive(Debug, Clone)]
pub struct Config {
    pub mqtt_tcp_port: u16,
    pub mqtt_ws_port: u16,
    pub http_port: u16,
    pub web_dir: PathBuf,
    pub db_path: PathBuf,
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
    let database = Arc::new(db::Database::open(&config.db_path)?);

    let mut broker = broker::build(&config)?;

    // Start MQTT bridge to capture messages into the database
    mqtt_bridge::start(&broker, database.clone())?;

    // Start web server on a background thread (receives db for API endpoints)
    let web_config = config.clone();
    let web_db = database.clone();
    std::thread::spawn(move || {
        if let Err(e) = web::start(&web_config, web_db) {
            eprintln!("Web server error: {e}");
        }
    });

    // Start MQTT broker listeners (blocks forever)
    broker.start()?;
    Ok(())
}
