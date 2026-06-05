use std::path::PathBuf;
use tracing::info;

use launa_server::{Config, GoogleHomeConfig};

#[derive(clap::Parser)]
#[command(name = "launa-server", about = "Launa MQTT broker with web UI")]
struct Cli {
    /// Path to config file
    #[arg(long, default_value = "launa-server.toml")]
    config: String,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("launa_server=info,rumqttd=warn")
        .init();

    let cli = <Cli as clap::Parser>::parse();

    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let config_path = std::path::Path::new(&cli.config);
    let config = if config_path.exists() {
        load_config(config_path, &project_root)
    } else {
        info!("Config file not found at {:?}, using defaults", config_path);
        Config::default_with_root(&project_root)
    };

    info!("Launa MQTT broker starting...");
    info!("  MQTT TCP:  0.0.0.0:{}", config.mqtt_tcp_port);
    info!("  MQTT WS:   0.0.0.0:{}", config.mqtt_ws_port);
    info!("  Web UI:    http://localhost:{}", config.http_port);
    info!("  State:     {:?}", config.state_path);
    if config.google_home.is_some() {
        info!("  Google Home: enabled");
    }

    if let Err(e) = launa_server::run(config) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn load_config(path: &std::path::Path, project_root: &PathBuf) -> Config {
    let settings = config::Config::builder()
        .set_default("server.mqtt_tcp_port", 1883u64)
        .unwrap()
        .set_default("server.mqtt_ws_port", 9001u64)
        .unwrap()
        .set_default("server.http_port", 8080u64)
        .unwrap()
        .set_default("server.web_dir", "web")
        .unwrap()
        .set_default("server.state_path", "launa-state.json")
        .unwrap()
        .set_default("google_home.username", "admin")
        .unwrap()
        .set_default("google_home.password", "changeme")
        .unwrap()
        .set_default("google_home.jwt_secret", "")
        .unwrap()
        .set_default("google_home.service_account_key_path", "")
        .unwrap()
        .set_default("google_home.celsius", false)
        .unwrap()
        .add_source(config::File::from(path))
        .build()
        .expect("Failed to parse config file");

    let server: ServerConfig = settings
        .get_table("server")
        .expect("Missing [server] section")
        .into_iter()
        .collect::<std::collections::HashMap<String, config::Value>>()
        .into();

    let gh: Option<GoogleHomeConfig> = if settings.get_table("google_home").is_ok() {
        let gh_table = settings.get_table("google_home").unwrap();
        let client_id = get_string(&gh_table, "oauth_client_id", "");
        let client_secret = get_string(&gh_table, "oauth_client_secret", "");

        if client_id.is_empty() || client_secret.is_empty() {
            eprintln!("Error: [google_home] section present but oauth_client_id or oauth_client_secret is missing");
            std::process::exit(1);
        }

        let jwt_secret = get_string(&gh_table, "jwt_secret", "");
        let jwt_secret = if jwt_secret.is_empty() {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            (0..32)
                .map(|_| format!("{:02x}", rng.gen::<u8>()))
                .collect()
        } else {
            jwt_secret
        };

        let sa_path = get_string(&gh_table, "service_account_key_path", "");
        Some(GoogleHomeConfig {
            oauth_client_id: client_id,
            oauth_client_secret: client_secret,
            username: get_string(&gh_table, "username", "admin"),
            password: get_string(&gh_table, "password", "changeme"),
            jwt_secret,
            service_account_key_path: if sa_path.is_empty() {
                None
            } else {
                Some(PathBuf::from(sa_path))
            },
            celsius: get_bool(&gh_table, "celsius", false),
        })
    } else {
        None
    };

    let web_dir = if std::path::Path::new(&server.web_dir).is_absolute() {
        PathBuf::from(&server.web_dir)
    } else {
        project_root.join(&server.web_dir)
    };

    let state_path = if std::path::Path::new(&server.state_path).is_absolute() {
        PathBuf::from(&server.state_path)
    } else {
        project_root.join(&server.state_path)
    };

    Config {
        mqtt_tcp_port: server.mqtt_tcp_port,
        mqtt_ws_port: server.mqtt_ws_port,
        http_port: server.http_port,
        web_dir,
        state_path,
        google_home: gh,
    }
}

struct ServerConfig {
    mqtt_tcp_port: u16,
    mqtt_ws_port: u16,
    http_port: u16,
    web_dir: String,
    state_path: String,
}

fn get_string(
    table: &std::collections::HashMap<String, config::Value>,
    key: &str,
    default: &str,
) -> String {
    table
        .get(key)
        .and_then(|v| v.clone().into_string().ok())
        .unwrap_or_else(|| default.to_string())
}

fn get_bool(
    table: &std::collections::HashMap<String, config::Value>,
    key: &str,
    default: bool,
) -> bool {
    table
        .get(key)
        .and_then(|v| v.clone().into_bool().ok())
        .unwrap_or(default)
}

impl From<std::collections::HashMap<String, config::Value>> for ServerConfig {
    fn from(map: std::collections::HashMap<String, config::Value>) -> Self {
        ServerConfig {
            mqtt_tcp_port: map
                .get("mqtt_tcp_port")
                .and_then(|v| v.clone().into_uint().ok())
                .unwrap_or(1883) as u16,
            mqtt_ws_port: map
                .get("mqtt_ws_port")
                .and_then(|v| v.clone().into_uint().ok())
                .unwrap_or(9001) as u16,
            http_port: map
                .get("http_port")
                .and_then(|v| v.clone().into_uint().ok())
                .unwrap_or(8080) as u16,
            web_dir: get_string(&map, "web_dir", "web"),
            state_path: get_string(&map, "state_path", "launa-state.json"),
        }
    }
}
