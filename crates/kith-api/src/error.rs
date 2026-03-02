//! API error type and [`axum::response::IntoResponse`] implementation.

use axum::{
  Json,
  http::StatusCode,
  response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// An error returned by an API handler.
#[derive(Debug, Error)]
pub enum ApiError {
  #[error("not found: {0}")]
  NotFound(String),

  #[error("bad request: {0}")]
  BadRequest(String),

  #[error("store error: {0}")]
  Store(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl IntoResponse for ApiError {
  fn into_response(self) -> Response {
    let (status, message) = match &self {
      ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
      ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
      ApiError::Store(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (status, Json(json!({ "error": message }))).into_response()
  }
}
