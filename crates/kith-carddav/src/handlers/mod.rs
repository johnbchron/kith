pub mod delete;
pub mod get;
pub mod options;
pub mod propfind;
pub mod put;
pub mod report;

use axum::{
  body::Body,
  http::{StatusCode, header},
  response::Response,
};

pub(super) const CONTENT_TYPE_MULTISTATUS: &str =
  "application/xml; charset=utf-8";

pub(super) fn multistatus_response(body: Vec<u8>) -> Response {
  Response::builder()
    .status(StatusCode::MULTI_STATUS)
    .header(header::CONTENT_TYPE, CONTENT_TYPE_MULTISTATUS)
    .body(Body::from(body))
    .unwrap()
}
