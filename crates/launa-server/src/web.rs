use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;
use tower_http::services::ServeDir;
use tracing::info;

use crate::Config;
use crate::db::Database;

pub fn start(config: &Config, db: Arc<Database>) -> Result<(), Box<dyn std::error::Error>> {
    let web_dir = config.web_dir.join("dist");

    if !web_dir.exists() {
        return Err(format!(
            "Web dist directory not found: {:?}\nRun `cd web && npm run build` first.",
            web_dir
        )
        .into());
    }

    let app_state = AppState {
        accessory_config: Arc::new(Mutex::new(AccessoryConfig::default())),
        db,
    };

    let app = Router::new()
        .route("/api/config", axum::routing::get(get_config).put(set_config))
        .route("/api/devices/{device_id}/logs", axum::routing::get(get_logs))
        .route("/api/devices/{device_id}/status", axum::routing::get(get_status))
        .route(
            "/api/devices/{device_id}/status/latest",
            axum::routing::get(get_latest_status),
        )
        .route(
            "/api/devices/{device_id}/diagnostics",
            axum::routing::get(get_diagnostics),
        )
        .route("/api/devices/{device_id}/alerts", axum::routing::get(get_alerts))
        .route(
            "/api/devices/{device_id}/sniff",
            axum::routing::get(get_sniff),
        )
        .with_state(app_state)
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

#[derive(Clone)]
struct AppState {
    accessory_config: Arc<Mutex<AccessoryConfig>>,
    db: Arc<Database>,
}

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 {
    100
}

async fn get_config(State(state): State<AppState>) -> Json<AccessoryConfig> {
    Json(state.accessory_config.lock().unwrap().clone())
}

async fn set_config(
    State(state): State<AppState>,
    Json(new_cfg): Json<AccessoryConfig>,
) -> Json<AccessoryConfig> {
    let mut cfg = state.accessory_config.lock().unwrap();
    cfg.pumps = new_cfg.pumps.min(6).max(1);
    cfg.lights = new_cfg.lights.min(4).max(1);
    cfg.blower = new_cfg.blower;
    cfg.mister = new_cfg.mister;
    Json(cfg.clone())
}

async fn get_logs(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::db::LogEntry>> {
    Json(state.db.get_logs(&device_id, query.limit))
}

async fn get_status(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::db::StatusEntry>> {
    Json(state.db.get_status_history(&device_id, query.limit))
}

async fn get_latest_status(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<Option<crate::db::StatusEntry>> {
    Json(state.db.get_latest_status(&device_id))
}

async fn get_diagnostics(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::db::TimestampedEntry>> {
    Json(state.db.get_diagnostics(&device_id, query.limit))
}

async fn get_alerts(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::db::TimestampedEntry>> {
    Json(state.db.get_alerts(&device_id, query.limit))
}

async fn get_sniff(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::db::TimestampedEntry>> {
    Json(state.db.get_sniff_frames(&device_id, query.limit))
}
