//! OPTIONS handler — no auth required.

use axum::{
  http::{HeaderValue, StatusCode, header},
  response::{IntoResponse, Response},
};

pub fn handler() -> Response {
  (
    StatusCode::NO_CONTENT,
    [
      (header::ALLOW, HeaderValue::from_static(
        "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, REPORT",
      )),
      (
        axum::http::HeaderName::from_static("dav"),
        HeaderValue::from_static("1, 3, addressbook"),
      ),
    ],
  )
    .into_response()
}
