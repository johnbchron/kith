//! GET and HEAD handlers for vCard resources.

use axum::{
  body::Body,
  http::{Method, StatusCode, header},
  response::Response,
};
use kith_core::store::ContactStore;

use crate::{
  AppState, error::Error, etag::compute_etag, handlers::propfind::parse_uid,
};

pub async fn handler<S>(
  state: &AppState<S>,
  method: &Method,
  uid_vcf: &str,
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let uid = parse_uid(uid_vcf)?;

  let view = state
    .store
    .materialize(uid, None)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?
    .filter(|v| !v.active_facts.is_empty())
    .ok_or(Error::NotFound)?;

  let etag = compute_etag(&view);
  let vcard = kith_vcard::serialize(&view)?;

  let builder = Response::builder()
    .status(StatusCode::OK)
    .header(header::CONTENT_TYPE, "text/vcard; charset=utf-8")
    .header(header::ETAG, &etag)
    .header(header::CONTENT_LENGTH, vcard.len());

  if *method == Method::HEAD {
    Ok(builder.body(Body::empty()).unwrap())
  } else {
    Ok(builder.body(Body::from(vcard)).unwrap())
  }
}
