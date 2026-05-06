use std::sync::{Arc, RwLock};

use axum::extract::{Path, Query, State};
use axum::response::Json;
use axum::Router;
use chrono::Utc;
use serde::Deserialize;
use tracing::info;

use crate::memory::{
    AccessoryConfigData as AccessoryConfig, DeviceSummary, GraphData, MemoryStore,
};
use crate::Config;

pub async fn start(
    config: &Config,
    mem: Arc<RwLock<MemoryStore>>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tower_http::services::{ServeDir, ServeFile};

    let web_dir = config.web_dir.join("dist");

    if !web_dir.exists() {
        return Err(format!(
            "Web dist directory not found: {:?}\nRun `cd web && npm run build` first.",
            web_dir
        )
        .into());
    }

    let app_state = AppState { mem };

    let app = build_router(app_state).fallback_service(
        ServeDir::new(&web_dir).fallback(ServeFile::new(web_dir.join("index.html"))),
    );

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.http_port));
    info!("Web server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Open http://localhost:{} in your browser", config.http_port);
    axum::serve(listener, app).await?;

    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    let device_routes = Router::new()
        .route("/logs", axum::routing::get(get_logs).delete(clear_logs))
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
        .route("/api/devices", axum::routing::get(list_devices))
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

#[derive(Clone)]
pub struct AppState {
    pub mem: Arc<RwLock<MemoryStore>>,
}

// --- Query parameter types ---

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u64,
}

#[derive(Deserialize)]
struct HoursQuery {
    #[serde(default = "default_hours")]
    hours: u64,
}

#[derive(Deserialize)]
struct AvailabilityQuery {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    hours: Option<u64>,
}

fn default_limit() -> u64 {
    100
}

fn default_hours() -> u64 {
    24
}

// --- API responses ---

#[derive(serde::Serialize)]
struct AckResponse {
    status: &'static str,
}

const OK: Json<AckResponse> = Json(AckResponse { status: "ok" });

// --- Handlers ---

async fn list_devices(State(state): State<AppState>) -> Json<Vec<DeviceSummary>> {
    Json(state.mem.read().unwrap().list_devices())
}

async fn get_config(State(state): State<AppState>) -> Json<AccessoryConfig> {
    Json(state.mem.read().unwrap().get_accessory_config().clone())
}

async fn set_config(
    State(state): State<AppState>,
    Json(new_cfg): Json<AccessoryConfig>,
) -> Json<AccessoryConfig> {
    let clamped = AccessoryConfig {
        pumps: new_cfg.pumps.clamp(1, 6),
        lights: new_cfg.lights.clamp(1, 4),
        blower: new_cfg.blower,
        mister: new_cfg.mister,
    };
    state
        .mem
        .write()
        .unwrap()
        .set_accessory_config(clamped.clone());
    Json(clamped)
}

async fn get_logs(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::memory::LogEntry>> {
    Json(state.mem.read().unwrap().get_logs(&device_id, query.limit))
}

async fn get_status_graph(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<HoursQuery>,
) -> Json<GraphData> {
    let since = (Utc::now() - chrono::Duration::hours(query.hours as i64)).to_rfc3339();
    let mem = state.mem.read().unwrap();
    Json(GraphData {
        temperatures: mem.get_temperature_history_since(&device_id, &since),
        components: mem.get_component_events_since(&device_id, &since),
    })
}

async fn get_diagnostics(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::memory::TimestampedEntry>> {
    Json(
        state
            .mem
            .read()
            .unwrap()
            .get_diagnostics(&device_id, query.limit),
    )
}

async fn get_alerts(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::memory::TimestampedEntry>> {
    Json(
        state
            .mem
            .read()
            .unwrap()
            .get_alerts(&device_id, query.limit),
    )
}

async fn get_sniff(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Json<Vec<crate::memory::TimestampedEntry>> {
    Json(
        state
            .mem
            .read()
            .unwrap()
            .get_sniff_frames(&device_id, query.limit),
    )
}

async fn get_availability(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Query(query): Query<AvailabilityQuery>,
) -> Json<Vec<crate::memory::AvailabilityEntry>> {
    let mem = state.mem.read().unwrap();
    if let Some(hours) = query.hours {
        let since = (Utc::now() - chrono::Duration::hours(hours as i64)).to_rfc3339();
        Json(mem.get_availability_history_since(&device_id, &since))
    } else {
        let limit = query.limit.unwrap_or(500);
        Json(mem.get_availability_history(&device_id, limit))
    }
}

async fn clear_logs(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<AckResponse> {
    state.mem.write().unwrap().clear_logs(&device_id);
    OK
}

async fn clear_alerts(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<AckResponse> {
    state.mem.write().unwrap().clear_alerts(&device_id);
    OK
}

async fn clear_diagnostics(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<AckResponse> {
    state.mem.write().unwrap().clear_diagnostics(&device_id);
    OK
}

async fn clear_sniff(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<AckResponse> {
    state.mem.write().unwrap().clear_sniff_frames(&device_id);
    OK
}

async fn get_device_availability(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<Option<crate::memory::DeviceStatus>> {
    Json(state.mem.read().unwrap().get_device_status(&device_id))
}
