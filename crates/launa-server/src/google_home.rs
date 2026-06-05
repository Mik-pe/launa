//! Google Smart Home Cloud-to-cloud integration.
//!
//! Implements the fulfillment endpoint and minimal OAuth 2.0 server required
//! by Google Home. Maps spa devices (thermostat, pumps, lights, blower, etc.)
//! to Google device types and traits.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::extract::State;
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use axum::Router;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::memory::MemoryStore;
use crate::web::AppState;

// --- Public API ---

pub fn build_router() -> Router<AppState> {
    Router::new()
        .route("/smarthome", axum::routing::post(fulfillment_handler))
        .route("/auth", axum::routing::get(auth_page))
        .route("/auth/login", axum::routing::post(auth_login))
        .route("/auth/token", axum::routing::post(auth_token))
}

// --- OAuth 2.0 --//

/// In-memory store for pending OAuth authorization codes.
/// Key: authorization code, Value: redirect URI.
static PENDING_AUTHS: std::sync::LazyLock<RwLock<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// GET /auth — Render the login page.
async fn auth_page(axum::extract::Query(params): axum::extract::Query<AuthParams>) -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html><head><title>Launa Spa - Login</title>
<style>
  body {{ font-family: system-ui; display: flex; justify-content: center; align-items: center; height: 100vh; margin: 0; background: #1a1a2e; color: #eee; }}
  form {{ background: #16213e; padding: 2rem; border-radius: 8px; box-shadow: 0 4px 12px rgba(0,0,0,0.3); }}
  input {{ display: block; margin: 0.5rem 0 1rem; padding: 0.5rem; width: 100%; box-sizing: border-box; border: 1px solid #0f3460; border-radius: 4px; background: #1a1a2e; color: #eee; }}
  button {{ background: #e94560; color: #fff; border: none; padding: 0.75rem 2rem; border-radius: 4px; cursor: pointer; font-size: 1rem; width: 100%; }}
  button:hover {{ background: #c73e54; }}
  h1 {{ margin-top: 0; font-size: 1.3rem; }}
  .sub {{ font-size: 0.85rem; color: #888; margin-bottom: 1rem; }}
</style></head><body>
<form method="POST" action="/auth/login">
  <h1>Launa Spa</h1>
  <p class="sub">Link Google Home to your spa</p>
  <input type="hidden" name="redirect_uri" value="{redirect_uri}" />
  <input type="hidden" name="state" value="{state}" />
  <label>Username</label>
  <input type="text" name="username" autocomplete="username" required />
  <label>Password</label>
  <input type="password" name="password" autocomplete="current-password" required />
  <button type="submit">Allow Access</button>
</form>
</body></html>"#,
        redirect_uri = html_escape(&params.redirect_uri),
        state = html_escape(&params.state.as_deref().unwrap_or("")),
    ))
}

#[derive(Deserialize)]
struct AuthParams {
    redirect_uri: String,
    state: Option<String>,
}

/// POST /auth/login — Validate credentials and redirect with auth code.
async fn auth_login(
    State(state): State<AppState>,
    axum::Form(form): axum::Form<LoginForm>,
) -> Response {
    let gh = match &state.config.google_home {
        Some(gh) => gh,
        None => {
            return (axum::http::StatusCode::FORBIDDEN, "Google Home not enabled").into_response()
        }
    };

    if form.username != gh.username || form.password != gh.password {
        return Html(
            "<!DOCTYPE html><html><body><h2>Login failed</h2><p>Invalid credentials. <a href='javascript:history.back()'>Try again</a></p></body></html>"
        ).into_response();
    }

    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    PENDING_AUTHS
        .write()
        .unwrap()
        .insert(code.clone(), form.redirect_uri.clone());

    let redirect_url = if let Some(s) = form.state {
        format!("{}?code={}&state={}", form.redirect_uri, code, s)
    } else {
        format!("{}?code={}", form.redirect_uri, code)
    };

    Redirect::to(&redirect_url).into_response()
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    redirect_uri: String,
    state: Option<String>,
}

/// POST /auth/token — Exchange authorization code for JWT access token.
async fn auth_token(
    State(state): State<AppState>,
    axum::Form(form): axum::Form<TokenRequest>,
) -> Response {
    let gh = match &state.config.google_home {
        Some(gh) => gh,
        None => return Json(serde_json::json!({"error": "invalid_grant"})).into_response(),
    };

    if form.client_id != gh.oauth_client_id || form.client_secret != gh.oauth_client_secret {
        return Json(serde_json::json!({"error": "invalid_client"})).into_response();
    }

    let redirect_uri = {
        let auths = PENDING_AUTHS.read().unwrap();
        match auths.get(&form.code) {
            Some(uri) => uri.clone(),
            None => return Json(serde_json::json!({"error": "invalid_grant"})).into_response(),
        }
    };

    if let Some(ref expected_uri) = form.redirect_uri {
        if expected_uri != &redirect_uri {
            return Json(serde_json::json!({"error": "invalid_grant"})).into_response();
        }
    }

    PENDING_AUTHS.write().unwrap().remove(&form.code);

    let token = match create_jwt(&gh.jwt_secret, &form.code) {
        Ok(t) => t,
        Err(e) => {
            warn!("Failed to create JWT: {e}");
            return Json(serde_json::json!({"error": "server_error"})).into_response();
        }
    };

    Json(serde_json::json!({
        "access_token": token,
        "token_type": "Bearer",
        "expires_in": 86400,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct TokenRequest {
    #[allow(dead_code)]
    grant_type: String,
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: Option<String>,
}

fn create_jwt(secret: &str, code: &str) -> Result<String, jsonwebtoken::errors::Error> {
    use jsonwebtoken::{encode, EncodingKey, Header};

    #[derive(Serialize)]
    struct Claims {
        sub: &'static str,
        code: String,
        exp: usize,
        iat: usize,
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize;

    let claims = Claims {
        sub: "launa-user",
        code: code.to_string(),
        exp: now + 86400,
        iat: now,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

fn validate_jwt(secret: &str, token: &str) -> bool {
    use jsonwebtoken::{decode, DecodingKey, Validation};

    #[derive(Deserialize)]
    struct Claims {
        #[allow(dead_code)]
        sub: String,
        #[allow(dead_code)]
        exp: usize,
    }

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .is_ok()
}

// --- Fulfillment Handler --//

/// POST /smarthome — Main fulfillment endpoint.
async fn fulfillment_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let gh = match &state.config.google_home {
        Some(gh) => gh,
        None => return Json(error_response("notSupported", "Google Home not enabled")),
    };

    // Validate Bearer token
    if let Some(auth) = headers.get("authorization") {
        let auth_str = auth.to_str().unwrap_or("");
        if let Some(token) = auth_str.strip_prefix("Bearer ") {
            if !validate_jwt(&gh.jwt_secret, token) {
                return Json(error_response("authFailure", "Invalid access token"));
            }
        } else {
            return Json(error_response("authFailure", "Missing Bearer token"));
        }
    } else {
        return Json(error_response(
            "authFailure",
            "Missing Authorization header",
        ));
    }

    let request_id = body["requestId"].as_str().unwrap_or("").to_string();
    let inputs = match body["inputs"].as_array() {
        Some(arr) => arr,
        None => return Json(error_response("protocolError", "Missing inputs")),
    };

    let mut payload_devices = serde_json::Map::new();

    for input in inputs {
        let intent = input["intent"].as_str().unwrap_or("");
        match intent {
            "action.devices.SYNC" => {
                return Json(sync_response(&request_id, &state));
            }
            "action.devices.QUERY" => {
                let devices = input
                    .get("payload")
                    .and_then(|p| p.get("devices"))
                    .and_then(|d| d.as_array());
                query_handler(&state.mem, devices, &mut payload_devices);
            }
            "action.devices.EXECUTE" => {
                return Json(execute_handler(&state, &request_id, input));
            }
            "action.devices.DISCONNECT" => {
                info!("Google Home DISCONNECT");
                return Json(serde_json::json!({ "requestId": request_id }));
            }
            _ => {
                warn!("Unknown intent: {intent}");
            }
        }
    }

    Json(serde_json::json!({
        "requestId": request_id,
        "payload": { "devices": payload_devices }
    }))
}

// --- SYNC --//

fn sync_response(request_id: &str, state: &AppState) -> serde_json::Value {
    let gh = state.config.google_home.as_ref().unwrap();
    let mem = state.mem.read().unwrap();
    let devices = mem.list_devices();
    let celsius = gh.celsius;

    let mut google_devices = Vec::new();

    for device in &devices {
        let id = &device.device_id;

        // Thermostat
        let (temp_unit, temp_min_c, temp_max_c) = if celsius {
            ("C", 10.0, 40.0)
        } else {
            ("F", 10.0, 40.0) // Google always uses Celsius internally
        };
        google_devices.push(serde_json::json!({
            "id": format!("{id}_thermostat"),
            "type": "action.devices.types.THERMOSTAT",
            "traits": ["action.devices.traits.TemperatureSetting"],
            "name": { "name": "Spa Temperature", "nicknames": ["spa", "spa temp"] },
            "attributes": {
                "availableThermostatModes": ["heat", "off"],
                "thermostatTemperatureUnit": temp_unit,
                "thermostatTemperatureRange": {
                    "minThresholdCelsius": temp_min_c,
                    "maxThresholdCelsius": temp_max_c,
                },
                "commandOnlyTemperatureSetting": false,
                "queryOnlyTemperatureSetting": false,
            },
            "willReportState": true,
            "deviceInfo": {
                "manufacturer": "Launa",
                "model": "BP6013G1",
            }
        }));

        // Pumps
        let cfg = mem.get_accessory_config();
        for i in 1..=cfg.pumps {
            google_devices.push(serde_json::json!({
                "id": format!("{id}_pump{i}"),
                "type": "action.devices.types.SWITCH",
                "traits": ["action.devices.traits.OnOff"],
                "name": { "name": format!("Spa Pump {i}"), "nicknames": [format!("pump {i}")] },
                "willReportState": true,
            }));
        }

        // Lights
        for i in 1..=cfg.lights {
            google_devices.push(serde_json::json!({
                "id": format!("{id}_light{i}"),
                "type": "action.devices.types.LIGHT",
                "traits": ["action.devices.traits.OnOff"],
                "name": { "name": if i == 1 { "Spa Light".into() } else { format!("Spa Light {i}") },
                         "nicknames": [format!("spa light {i}")] },
                "willReportState": true,
            }));
        }

        // Blower
        if cfg.blower {
            google_devices.push(serde_json::json!({
                "id": format!("{id}_blower"),
                "type": "action.devices.types.FAN",
                "traits": ["action.devices.traits.OnOff"],
                "name": { "name": "Spa Blower", "nicknames": ["blower", "jets"] },
                "willReportState": true,
            }));
        }

        // Mister
        if cfg.mister {
            google_devices.push(serde_json::json!({
                "id": format!("{id}_mister"),
                "type": "action.devices.types.SWITCH",
                "traits": ["action.devices.traits.OnOff"],
                "name": { "name": "Spa Mister", "nicknames": ["mister"] },
                "willReportState": true,
            }));
        }

        // Circulation pump
        google_devices.push(serde_json::json!({
            "id": format!("{id}_circ_pump"),
            "type": "action.devices.types.SWITCH",
            "traits": ["action.devices.traits.OnOff"],
            "name": { "name": "Circulation Pump", "nicknames": ["circ pump", "circulation"] },
            "willReportState": true,
        }));
    }

    serde_json::json!({
        "requestId": request_id,
        "payload": {
            "agentUserId": "launa-user",
            "devices": google_devices,
        }
    })
}

// --- QUERY --//

fn query_handler(
    mem: &Arc<RwLock<MemoryStore>>,
    requested_devices: Option<&Vec<serde_json::Value>>,
    results: &mut serde_json::Map<String, serde_json::Value>,
) {
    let store = mem.read().unwrap();
    let celsius = false; // TODO: read from config

    let device_ids: Vec<String> = match requested_devices {
        Some(devices) => devices
            .iter()
            .filter_map(|d| d["id"].as_str().map(String::from))
            .collect(),
        None => return,
    };

    // Group by spa device: "spa_001_thermostat" -> "spa_001"
    for google_id in &device_ids {
        let (device_id, device_type) = parse_google_device_id(google_id);
        let state = match device_type {
            "thermostat" => {
                let latest = store.get_latest_temperature(&device_id);
                match latest {
                    Some(sample) => {
                        let ambient_c = sample
                            .current_temp
                            .map(|f| if celsius { f } else { (f - 32.0) * 5.0 / 9.0 })
                            .unwrap_or(0.0);
                        let setpoint_c = sample
                            .set_temp
                            .map(|f| if celsius { f } else { (f - 32.0) * 5.0 / 9.0 })
                            .unwrap_or(0.0);
                        serde_json::json!({
                            "status": "SUCCESS",
                            "online": store.get_device_status(&device_id).map_or(false, |s| s.status == "online"),
                            "thermostatMode": "heat",
                            "thermostatTemperatureAmbient": (ambient_c * 10.0).round() / 10.0,
                            "thermostatTemperatureSetpoint": (setpoint_c * 10.0).round() / 10.0,
                        })
                    }
                    None => serde_json::json!({
                        "status": "OFFLINE",
                        "online": false,
                    }),
                }
            }
            "pump" | "light" | "blower" | "mister" | "circ_pump" => {
                let component = match device_type {
                    "pump" => {
                        // Extract pump number from google_id: spa_001_pump3 -> pump3_on
                        let suffix = google_id.rsplit('_').next().unwrap_or("");
                        format!("{}_on", suffix) // "pump3_on"
                    }
                    "light" => {
                        let suffix = google_id.rsplit('_').next().unwrap_or("");
                        suffix.to_string() // "light1"
                    }
                    "blower" => "blower".to_string(),
                    "mister" => "mister".to_string(),
                    "circ_pump" => "circ_pump".to_string(),
                    _ => unreachable!(),
                };
                let is_on = store.get_latest_component_state(&device_id, &component);
                serde_json::json!({
                    "status": "SUCCESS",
                    "online": store.get_device_status(&device_id).map_or(false, |s| s.status == "online"),
                    "on": is_on,
                })
            }
            _ => {
                serde_json::json!({ "status": "ERROR", "errorCode": "deviceNotFound" })
            }
        };
        results.insert(google_id.clone(), state);
    }
}

/// Parse a Google device ID like "spa_001_thermostat" into (device_id, device_type).
fn parse_google_device_id(google_id: &str) -> (String, &str) {
    // Known suffixes that are purely alphabetical
    let suffixes = ["_thermostat", "_circ_pump", "_blower", "_mister"];
    for suffix in &suffixes {
        if let Some(base) = google_id.strip_suffix(suffix) {
            return (base.to_string(), &suffix[1..]);
        }
    }
    // Pump and light: spa_001_pump3, spa_001_light2
    // Find the last number sequence, then check if preceded by _pump or _light
    let trimmed = google_id.trim_end_matches(|c: char| c.is_ascii_digit());
    let num_suffix_len = google_id.len() - trimmed.len();
    if num_suffix_len > 0 {
        if let Some(base) = trimmed.strip_suffix("_pump") {
            return (base.to_string(), "pump");
        }
        if let Some(base) = trimmed.strip_suffix("_light") {
            return (base.to_string(), "light");
        }
    }
    (google_id.to_string(), "unknown")
}

// --- EXECUTE --//

fn execute_handler(
    state: &AppState,
    request_id: &str,
    input: &serde_json::Value,
) -> serde_json::Value {
    let commands = input
        .get("payload")
        .and_then(|p| p.get("commands"))
        .and_then(|c| c.as_array());

    let commands = match commands {
        Some(c) => c,
        None => {
            return serde_json::json!({ "requestId": request_id, "payload": { "commands": [] } })
        }
    };

    let mut results = Vec::new();

    for cmd_group in commands {
        let devices = cmd_group
            .get("devices")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let executions = cmd_group.get("execution").and_then(|e| e.as_array());

        let execution_list = match executions {
            Some(e) => e,
            None => continue,
        };

        for device in &devices {
            let google_id = device["id"].as_str().unwrap_or("");
            let (device_id, device_type) = parse_google_device_id(google_id);

            for execution in execution_list {
                let command = execution["command"].as_str().unwrap_or("");
                let params = execution
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                let result =
                    execute_command(state, &device_id, device_type, google_id, command, &params);
                results.push(result);
            }
        }
    }

    serde_json::json!({
        "requestId": request_id,
        "payload": { "commands": results }
    })
}

fn execute_command(
    state: &AppState,
    device_id: &str,
    device_type: &str,
    google_id: &str,
    command: &str,
    params: &serde_json::Value,
) -> serde_json::Value {
    let celsius = state
        .config
        .google_home
        .as_ref()
        .map_or(false, |gh| gh.celsius);

    let mqtt_topic_and_payload = match command {
        "action.devices.commands.ThermostatTemperatureSetpoint" => {
            let setpoint_c = params["thermostatTemperatureSetpoint"]
                .as_f64()
                .unwrap_or(40.0);
            let setpoint_f = if celsius {
                setpoint_c
            } else {
                setpoint_c * 9.0 / 5.0 + 32.0
            };
            let rounded = setpoint_f.round() as i32;
            Some((
                format!("launa/{device_id}/command/set_temperature"),
                rounded.to_string(),
            ))
        }
        "action.devices.commands.ThermostatSetMode" => {
            let mode = params["thermostatMode"].as_str().unwrap_or("heat");
            let launa_mode = match mode {
                "heat" => "ready",
                "off" => "rest",
                _ => "ready",
            };
            Some((
                format!("launa/{device_id}/command/heat_mode"),
                launa_mode.to_string(),
            ))
        }
        "action.devices.commands.OnOff" => {
            let on = params["on"].as_bool().unwrap_or(false);
            let payload = if on { "true" } else { "false" };
            let topic_suffix = match device_type {
                "pump" => {
                    let suffix = google_id.rsplit('_').next().unwrap_or("pump1");
                    suffix.to_string() // "pump3"
                }
                "light" => {
                    let suffix = google_id.rsplit('_').next().unwrap_or("light1");
                    suffix.to_string() // "light2"
                }
                "blower" => "blower".to_string(),
                "mister" => "mister".to_string(),
                "circ_pump" => "circulation_pump".to_string(),
                _ => {
                    return serde_json::json!({
                        "ids": [google_id],
                        "status": "ERROR",
                        "errorCode": "functionNotSupported",
                    })
                }
            };
            Some((
                format!("launa/{device_id}/command/{topic_suffix}"),
                payload.to_string(),
            ))
        }
        _ => {
            warn!("Unknown Google command: {command}");
            return serde_json::json!({
                "ids": [google_id],
                "status": "ERROR",
                "errorCode": "functionNotSupported",
            });
        }
    };

    match mqtt_topic_and_payload {
        Some((topic, payload)) => {
            publish_mqtt_command(state, &topic, &payload);
            serde_json::json!({
                "ids": [google_id],
                "status": "SUCCESS",
            })
        }
        None => serde_json::json!({
            "ids": [google_id],
            "status": "ERROR",
            "errorCode": "functionNotSupported",
        }),
    }
}

/// Publish a command to the internal MQTT broker.
fn publish_mqtt_command(_state: &AppState, topic: &str, payload: &str) {
    if let Some(tx) = MQTT_CMD_CHANNEL.read().unwrap().as_ref() {
        if let Err(e) = tx.send((topic.to_string(), payload.to_string())) {
            warn!("Failed to send MQTT command to {topic}: {e:?}");
        } else {
            info!("Published MQTT command: {topic} = {payload}");
        }
    } else {
        warn!("MQTT publisher not available, cannot send command to {topic}");
    }
}

/// Channel for sending MQTT commands from Google Home handlers to the bridge thread.
static MQTT_CMD_CHANNEL: std::sync::LazyLock<RwLock<Option<flume::Sender<(String, String)>>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

/// Register the MQTT command channel. Returns the receiver for the bridge thread.
pub fn create_mqtt_command_channel() -> flume::Receiver<(String, String)> {
    let (tx, rx) = flume::unbounded();
    MQTT_CMD_CHANNEL.write().unwrap().replace(tx);
    rx
}

// --- Helpers --//

fn error_response(error_code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "payload": {
            "errorCode": error_code,
            "debugString": message,
        }
    })
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_gh_config() -> crate::GoogleHomeConfig {
        crate::GoogleHomeConfig {
            oauth_client_id: "test-client-id".into(),
            oauth_client_secret: "test-client-secret".into(),
            username: "admin".into(),
            password: "secret".into(),
            jwt_secret: "test-jwt-secret-key-32bytes!!".into(),
            service_account_key_path: None,
            celsius: false,
        }
    }

    fn test_config() -> crate::Config {
        crate::Config {
            mqtt_tcp_port: 0,
            mqtt_ws_port: 0,
            http_port: 0,
            web_dir: PathBuf::from("/dev/null"),
            state_path: PathBuf::from("/dev/null"),
            google_home: Some(test_gh_config()),
        }
    }

    fn test_app_state() -> AppState {
        let mem = Arc::new(RwLock::new(MemoryStore::new()));
        AppState {
            mem,
            config: test_config(),
        }
    }

    fn test_app_state_with_device() -> AppState {
        let state = test_app_state();
        {
            let mut store = state.mem.write().unwrap();
            store.update_device_status("spa_001", "online", Some(1));
            store.insert_temperature_sample("spa_001", Some(100.0), Some(104.0));
            let all_on: Vec<(&str, bool)> = crate::memory::COMPONENT_FIELDS
                .iter()
                .map(|&f| (f, true))
                .collect();
            store.insert_component_changes("spa_001", &all_on);
        }
        state
    }

    #[allow(dead_code)]
    fn bearer_token() -> String {
        create_jwt("test-jwt-secret-key-32bytes!!", "test-code").unwrap()
    }

    #[test]
    fn test_parse_google_device_id_thermostat() {
        let (id, t) = parse_google_device_id("spa_001_thermostat");
        assert_eq!(id, "spa_001");
        assert_eq!(t, "thermostat");
    }

    #[test]
    fn test_parse_google_device_id_circ_pump() {
        let (id, t) = parse_google_device_id("spa_001_circ_pump");
        assert_eq!(id, "spa_001");
        assert_eq!(t, "circ_pump");
    }

    #[test]
    fn test_parse_google_device_id_blower() {
        let (id, t) = parse_google_device_id("spa_001_blower");
        assert_eq!(id, "spa_001");
        assert_eq!(t, "blower");
    }

    #[test]
    fn test_parse_google_device_id_mister() {
        let (id, t) = parse_google_device_id("spa_001_mister");
        assert_eq!(id, "spa_001");
        assert_eq!(t, "mister");
    }

    #[test]
    fn test_parse_google_device_id_pump() {
        let (id, t) = parse_google_device_id("spa_001_pump3");
        assert_eq!(id, "spa_001");
        assert_eq!(t, "pump");
    }

    #[test]
    fn test_parse_google_device_id_light() {
        let (id, t) = parse_google_device_id("spa_001_light2");
        assert_eq!(id, "spa_001");
        assert_eq!(t, "light");
    }

    #[test]
    fn test_parse_google_device_id_unknown() {
        let (id, t) = parse_google_device_id("spa_001_whatever");
        assert_eq!(id, "spa_001_whatever");
        assert_eq!(t, "unknown");
    }

    #[test]
    fn test_jwt_round_trip() {
        let secret = "test-secret";
        let token = create_jwt(secret, "auth-code-123").unwrap();
        assert!(validate_jwt(secret, &token));
    }

    #[test]
    fn test_jwt_invalid_secret_fails() {
        let token = create_jwt("secret1", "code").unwrap();
        assert!(!validate_jwt("wrong-secret", &token));
    }

    #[test]
    fn test_sync_response_has_devices() {
        let state = test_app_state_with_device();
        let resp = sync_response("req-1", &state);
        assert_eq!(resp["requestId"], "req-1");
        assert_eq!(resp["payload"]["agentUserId"], "launa-user");
        let devices = resp["payload"]["devices"].as_array().unwrap();
        assert!(!devices.is_empty(), "SYNC should return devices");

        let ids: Vec<&str> = devices.iter().map(|d| d["id"].as_str().unwrap()).collect();
        assert!(ids.iter().any(|id| id.contains("thermostat")));
        assert!(ids.iter().any(|id| id.contains("pump")));
        assert!(ids.iter().any(|id| id.contains("light")));
        assert!(ids.iter().any(|id| id.contains("blower")));
        assert!(ids.iter().any(|id| id.contains("circ_pump")));
    }

    #[test]
    fn test_sync_thermostat_has_temperature_setting() {
        let state = test_app_state_with_device();
        let resp = sync_response("req-1", &state);
        let devices = resp["payload"]["devices"].as_array().unwrap();
        let thermostat = devices
            .iter()
            .find(|d| d["id"].as_str().unwrap().contains("thermostat"))
            .unwrap();
        let traits = thermostat["traits"].as_array().unwrap();
        assert!(traits
            .iter()
            .any(|t| t.as_str() == Some("action.devices.traits.TemperatureSetting")));
        assert_eq!(thermostat["type"], "action.devices.types.THERMOSTAT");
    }

    #[test]
    fn test_sync_respects_accessory_config() {
        let state = test_app_state_with_device();
        state
            .mem
            .write()
            .unwrap()
            .set_accessory_config(crate::memory::AccessoryConfigData {
                pumps: 3,
                lights: 2,
                blower: false,
                mister: true,
            });
        let resp = sync_response("req-1", &state);
        let devices = resp["payload"]["devices"].as_array().unwrap();
        let ids: Vec<&str> = devices.iter().map(|d| d["id"].as_str().unwrap()).collect();

        assert_eq!(
            ids.iter()
                .filter(|id| id.contains("pump") && !id.contains("circ"))
                .count(),
            3
        );
        assert_eq!(ids.iter().filter(|id| id.contains("light")).count(), 2);
        assert!(
            !ids.iter().any(|id| id.contains("blower")),
            "blower disabled in config"
        );
        assert!(
            ids.iter().any(|id| id.contains("mister")),
            "mister enabled in config"
        );
    }

    #[test]
    fn test_query_thermostat() {
        let state = test_app_state_with_device();
        let mut results = serde_json::Map::new();
        let devices = vec![serde_json::json!({ "id": "spa_001_thermostat" })];
        query_handler(&state.mem, Some(&devices), &mut results);

        let thermo = &results["spa_001_thermostat"];
        assert_eq!(thermo["status"], "SUCCESS");
        assert_eq!(thermo["online"], true);
        assert_eq!(thermo["thermostatMode"], "heat");
        assert!(thermo["thermostatTemperatureAmbient"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn test_query_pump() {
        let state = test_app_state_with_device();
        let mut results = serde_json::Map::new();
        let devices = vec![serde_json::json!({ "id": "spa_001_pump1" })];
        query_handler(&state.mem, Some(&devices), &mut results);

        let pump = &results["spa_001_pump1"];
        assert_eq!(pump["status"], "SUCCESS");
        assert_eq!(pump["on"], true);
    }

    #[test]
    fn test_query_no_data_returns_offline() {
        let state = test_app_state();
        state
            .mem
            .write()
            .unwrap()
            .update_device_status("spa_001", "online", None);
        let mut results = serde_json::Map::new();
        let devices = vec![serde_json::json!({ "id": "spa_001_thermostat" })];
        query_handler(&state.mem, Some(&devices), &mut results);

        assert_eq!(results["spa_001_thermostat"]["status"], "OFFLINE");
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(
            html_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&#39;f"
        );
    }

    #[test]
    fn test_mqtt_command_channel() {
        let rx = create_mqtt_command_channel();
        {
            let guard = MQTT_CMD_CHANNEL.read().unwrap();
            let tx = guard.as_ref().unwrap();
            tx.send(("test/topic".into(), "payload".into())).unwrap();
        }
        let (topic, payload) = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        assert_eq!(topic, "test/topic");
        assert_eq!(payload, "payload");
    }
}
