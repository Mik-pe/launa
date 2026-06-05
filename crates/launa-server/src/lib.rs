use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use tokio::signal;
use tokio::signal::unix::{signal as unix_signal, SignalKind};
use tracing::{error, info};

pub mod broker;
pub mod error;
pub mod google_home;
pub mod memory;
pub mod mqtt_bridge;
pub mod notifier;
pub mod web;

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct Config {
    pub mqtt_tcp_port: u16,
    pub mqtt_ws_port: u16,
    pub http_port: u16,
    pub web_dir: PathBuf,
    pub state_path: PathBuf,
    pub google_home: Option<GoogleHomeConfig>,
}

impl Config {
    pub fn default_with_root(project_root: &PathBuf) -> Self {
        Config {
            mqtt_tcp_port: 1883,
            mqtt_ws_port: 9001,
            http_port: 8080,
            web_dir: project_root.join("web"),
            state_path: project_root.join("launa-state.json"),
            google_home: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GoogleHomeConfig {
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub username: String,
    pub password: String,
    pub jwt_secret: String,
    pub service_account_key_path: Option<PathBuf>,
    pub celsius: bool,
}

pub use error::Error;
/// JSON-serializable accessory visibility config served to the web UI.
pub use memory::AccessoryConfigData as AccessoryConfig;

pub fn run(config: Config) -> error::Result<()> {
    let mem_store = Arc::new(RwLock::new(memory::MemoryStore::load(&config.state_path)));

    let mut broker = broker::build(config.mqtt_tcp_port, config.mqtt_ws_port)?;

    // Start MQTT bridge to capture messages into memory store
    mqtt_bridge::start(&broker, mem_store.clone())
        .map_err(|e| error::Error::MqttLink(e.to_string()))?;

    // Create a single tokio runtime for the web server and shutdown handling
    let rt = tokio::runtime::Runtime::new().map_err(error::Error::Io)?;

    // Start web server on the tokio runtime
    let web_config = config.clone();
    let web_mem = mem_store.clone();
    rt.spawn(async move {
        if let Err(e) = web::start(&web_config, web_mem).await {
            error!("Web server error: {e}");
        }
    });

    // Start notification watchdog on the tokio runtime
    let notifier = notifier::Notifier::new(mem_store.clone());
    rt.spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            notifier.check().await;
        }
    });

    // Shutdown handler: listen for both SIGINT (Ctrl+C) and SIGTERM (systemctl stop)
    let save_path = config.state_path.clone();
    let save_store = mem_store.clone();
    rt.spawn(async move {
        let mut sigterm = unix_signal(SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = signal::ctrl_c() => info!("Received SIGINT (Ctrl+C), saving state..."),
            _ = sigterm.recv() => info!("Received SIGTERM, saving state..."),
        }
        save_store.read().unwrap().save(&save_path);
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
        info!("State saved. Shutting down...");
        std::process::exit(0);
    });

    // Start MQTT broker listeners (blocks forever)
    broker
        .start()
        .map_err(|e| error::Error::Broker(e.to_string()))?;

    // If broker.start() ever returns, save state
    info!("Broker exited, saving state...");
    mem_store.read().unwrap().save(&config.state_path);

    Ok(())
}
