//! Error types and axum `IntoResponse` implementation.

use axum::{
  http::{HeaderValue, StatusCode, header},
  response::{IntoResponse, Response},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
  #[error("unauthorized")]
  Unauthorized,
  #[error("not found")]
  NotFound,
  #[error("precondition failed")]
  PreconditionFailed,
  #[error("conflict: {0}")]
  Conflict(String),
  #[error("bad request: {0}")]
  BadRequest(String),
  #[error("xml error: {0}")]
  Xml(String),
  #[error("vcard error: {0}")]
  Vcard(#[from] kith_vcard::Error),
  #[error("store error: {0}")]
  Store(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl IntoResponse for Error {
  fn into_response(self) -> Response {
    match self {
      Error::Unauthorized => {
        let mut res =
          (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        res.headers_mut().insert(
          header::WWW_AUTHENTICATE,
          HeaderValue::from_static("Basic realm=\"kith\""),
        );
        res
      }
      Error::NotFound => (StatusCode::NOT_FOUND, "Not Found").into_response(),
      Error::PreconditionFailed => {
        tracing::warn!("precondition failed (412)");
        (StatusCode::PRECONDITION_FAILED, "Precondition Failed").into_response()
      }
      Error::Conflict(msg) => {
        tracing::warn!(reason = %msg, "conflict (409)");
        (StatusCode::CONFLICT, msg).into_response()
      }
      Error::BadRequest(msg) => {
        tracing::warn!(reason = %msg, "bad request (400)");
        (StatusCode::BAD_REQUEST, msg).into_response()
      }
      Error::Xml(msg) => {
        tracing::error!(reason = %msg, "XML processing error (500)");
        (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
      }
      Error::Vcard(e) => {
        tracing::error!(error = %e, "vCard processing error (500)");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
      }
      Error::Store(e) => {
        tracing::error!(error = %e, "store error (500)");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
      }
    }
  }
}
