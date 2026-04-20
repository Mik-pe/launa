use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::response::Json;
use tower_http::services::ServeDir;
use tracing::info;

use crate::Config;

pub fn start(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let web_dir = config.web_dir.join("dist");

    if !web_dir.exists() {
        return Err(format!(
            "Web dist directory not found: {:?}\nRun `cd web && npm run build` first.",
            web_dir
        )
        .into());
    }

    let accessory_config = Arc::new(Mutex::new(AccessoryConfig::default()));

    let app = Router::new()
        .route("/api/config", axum::routing::get(get_config).put(set_config))
        .with_state(accessory_config)
        .fallback_service(ServeDir::new(&web_dir).fallback(
            ServeDir::new(web_dir.join("index.html")),
        ));

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.http_port));
    info!("Web server listening on http://{}", addr);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!("Open http://localhost:{} in your browser", config.http_port);
        axum::serve(listener, app).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;

    Ok(())
}

/// JSON-serializable accessory visibility config served to the web UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccessoryConfig {
    pub pumps: u8,
    pub lights: u8,
    pub blower: bool,
    pub mister: bool,
}

impl Default for AccessoryConfig {
    fn default() -> Self {
        AccessoryConfig {
            pumps: 2,
            lights: 1,
            blower: true,
            mister: false,
        }
    }
}

type SharedConfig = Arc<Mutex<AccessoryConfig>>;

async fn get_config(State(cfg): State<SharedConfig>) -> Json<AccessoryConfig> {
    Json(cfg.lock().unwrap().clone())
}

async fn set_config(
    State(cfg): State<SharedConfig>,
    Json(new_cfg): Json<AccessoryConfig>,
) -> Json<AccessoryConfig> {
    let mut cfg = cfg.lock().unwrap();
    cfg.pumps = new_cfg.pumps.min(6).max(1);
    cfg.lights = new_cfg.lights.min(4).max(1);
    cfg.blower = new_cfg.blower;
    cfg.mister = new_cfg.mister;
    Json(cfg.clone())
}
