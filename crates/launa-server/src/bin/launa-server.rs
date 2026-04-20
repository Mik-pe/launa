use clap::Parser;
use launa_server::Config;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(name = "launa-server", about = "Launa MQTT broker with web UI")]
struct Cli {
    /// MQTT TCP port
    #[arg(long, default_value_t = 1883)]
    mqtt_port: u16,

    /// MQTT WebSocket port (for web GUI)
    #[arg(long, default_value_t = 9001)]
    ws_port: u16,

    /// HTTP port for serving the web UI
    #[arg(long, default_value_t = 8080)]
    http_port: u16,

    /// Path to the web directory (containing dist/)
    #[arg(long)]
    web_dir: Option<String>,

    /// Path to the SQLite database file
    #[arg(long)]
    db_path: Option<String>,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("launa_server=info,rumqttd=warn")
        .init();

    let cli = Cli::parse();

    let web_dir = cli.web_dir.map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("web")
    });

    let db_path = cli.db_path.map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("launa.db")
    });

    let config = Config {
        mqtt_tcp_port: cli.mqtt_port,
        mqtt_ws_port: cli.ws_port,
        http_port: cli.http_port,
        web_dir,
        db_path,
    };

    info!("Launa MQTT broker starting...");
    info!("  MQTT TCP:  0.0.0.0:{}", config.mqtt_tcp_port);
    info!("  MQTT WS:   0.0.0.0:{}", config.mqtt_ws_port);
    info!("  Web UI:    http://localhost:{}", config.http_port);
    info!("  Database:  {:?}", config.db_path);

    if let Err(e) = launa_server::run(config) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
