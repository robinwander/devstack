use anyhow::anyhow;
use axum::{Json, response::IntoResponse};
use hyper::StatusCode;

#[derive(Debug)]
pub enum AppError {
    NotFound(String),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(value: E) -> Self {
        Self::Internal(value.into())
    }
}

impl AppError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(anyhow!(message.into()))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::NotFound(message) => {
                let body = serde_json::json!({ "error": message });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            Self::BadRequest(message) => {
                let body = serde_json::json!({ "error": message });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            Self::Internal(err) => {
                let body = serde_json::json!({ "error": err.to_string() });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
        }
    }
}
