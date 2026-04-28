use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::response::Json;
use axum::Router;
use chrono::Utc;
use serde::Deserialize;
use tracing::info;

use crate::db::{Database, GraphData};
use crate::Config;

pub fn start(config: &Config, db: Arc<Database>) -> Result<(), Box<dyn std::error::Error>> {
    use tower_http::services::{ServeDir, ServeFile};

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

    let app = build_router(app_state).fallback_service(
        ServeDir::new(&web_dir).fallback(ServeFile::new(web_dir.join("index.html"))),
    );

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

pub fn build_router(state: AppState) -> Router {
    let device_routes = Router::new()
        .route("/logs", axum::routing::get(get_logs).delete(clear_logs))
        .route("/status", axum::routing::get(get_status))
        .route("/status/latest", axum::routing::get(get_latest_status))
        .route("/status/graph", axum::routing::get(get_status_graph))
        .route(
            "/diagnostics",
            axum::routing::get(get_diagnostics).delete(clear_diagnostics),
        )
        .route(
            "/alerts",
            axum::routing::get(get_alerts).delete(clear_alerts),
        )
        .route("/sniff", axum::routing::get(get_sniff).delete(clear_sniff))
        .route(
            "/availability/history",
            axum::routing::get(get_availability),
        );

    Router::new()
        .route(
            "/api/config",
            axum::routing::get(get_config).put(set_config),
        )
        .route(
            "/api/devices/{device_id}/availability",
            axum::routing::get(get_device_availability),
        )
        .nest("/api/devices/{device_id}", device_routes)
        .with_state(state)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
pub struct AppState {
    pub accessory_config: Arc<Mutex<AccessoryConfig>>,
    pub db: Arc<Database>,
}

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u64,
    #[serde(default)]
    hours: Option<u64>,
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
    cfg.pumps = new_cfg.pumps.clamp(1, 6);
    cfg.lights = new_cfg.lights.clamp(1, 4);
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
    if let Some(hours) = query.hours {
        let since = (Utc::now() - chrono::Duration::hours(hours as i64)).to_rfc3339();
        Json(state.db.get_status_history_since(&device_id, &since))
    } else {
        Json(state.db.get_status_history(&device_id, query.limit))
    }
}

async fn get_status_graph(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<GraphData> {
    let hours = query.hours.unwrap_or(24);
    let since = (Utc::now() - chrono::Duration::hours(hours as i64)).to_rfc3339();
    Json(GraphData {
        temperatures: state.db.get_temperature_history_since(&device_id, &since),
        components: state.db.get_component_events_since(&device_id, &since),
    })
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

async fn get_availability(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::db::AvailabilityEntry>> {
    if let Some(hours) = query.hours {
        let since = (Utc::now() - chrono::Duration::hours(hours as i64)).to_rfc3339();
        Json(state.db.get_availability_history_since(&device_id, &since))
    } else {
        Json(state.db.get_availability_history(&device_id, query.limit))
    }
}

async fn clear_logs(State(state): State<AppState>, Path(device_id): Path<String>) -> &'static str {
    state.db.clear_logs(&device_id);
    "ok"
}

async fn clear_alerts(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> &'static str {
    state.db.clear_alerts(&device_id);
    "ok"
}

async fn clear_diagnostics(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> &'static str {
    state.db.clear_diagnostics(&device_id);
    "ok"
}

async fn clear_sniff(State(state): State<AppState>, Path(device_id): Path<String>) -> &'static str {
    state.db.clear_sniff_frames(&device_id);
    "ok"
}

async fn get_device_availability(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<Option<crate::db::DeviceStatus>> {
    Json(state.db.get_device_status(&device_id))
}
