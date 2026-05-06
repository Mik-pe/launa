use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("MQTT broker error: {0}")]
    Broker(String),

    #[error("MQTT link error: {0}")]
    MqttLink(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Config error: {0}")]
    Config(#[from] config::ConfigError),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let message = self.to_string();
        tracing::error!("Request error: {message}");
        (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
    }
}
