//! PUT handler — create or update a vCard resource.

use axum::{
  http::{HeaderMap, StatusCode, header},
  response::{IntoResponse, Response},
};
use kith_core::{store::ContactStore, subject::SubjectKind};

use crate::{
  AppState, diff,
  error::Error,
  etag::{compute_etag, compute_etag_from_pairs},
  handlers::propfind::parse_uid,
};

pub async fn handler<S>(
  state: &AppState<S>,
  headers: &HeaderMap,
  uid_vcf: &str,
  body: &str,
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let uid = parse_uid(uid_vcf)?;

  let if_match = headers
    .get(header::IF_MATCH)
    .and_then(|v| v.to_str().ok())
    .map(|s| s.to_string());

  let existing_subject = state
    .store
    .get_subject(uid)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?;

  let is_new = existing_subject.is_none();

  if is_new {
    if if_match.is_some() {
      return Err(Error::PreconditionFailed);
    }
    state
      .store
      .add_subject_with_id(uid, SubjectKind::Person)
      .await
      .map_err(|e| Error::Store(Box::new(e)))?;
  } else if let Some(ref etag_header) = if_match {
    let view = state
      .store
      .materialize(uid, None)
      .await
      .map_err(|e| Error::Store(Box::new(e)))?
      .ok_or(Error::NotFound)?;
    let current_etag = compute_etag(&view);
    if strip_etag_quotes(&current_etag) != strip_etag_quotes(etag_header) {
      return Err(Error::PreconditionFailed);
    }
  }

  let current_view = state
    .store
    .materialize(uid, None)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?;

  let result = diff::diff(body, uid, "carddav-put", current_view.as_ref())?;

  let mut new_pairs = Vec::new();
  for new_fact in result.new_facts {
    let recorded = state
      .store
      .record_fact(new_fact)
      .await
      .map_err(|e| Error::Store(Box::new(e)))?;
    new_pairs.push((recorded.fact_id, recorded.recorded_at));
  }

  for (old_id, replacement) in result.supersessions {
    let (_, new_fact) = state
      .store
      .supersede(old_id, replacement)
      .await
      .map_err(|e| Error::Store(Box::new(e)))?;
    new_pairs.push((new_fact.fact_id, new_fact.recorded_at));
  }

  for fact_id in result.retractions {
    state
      .store
      .retract(fact_id, Some("Superseded by CardDAV PUT".to_string()))
      .await
      .map_err(|e| Error::Store(Box::new(e)))?;
  }

  let new_etag = match state
    .store
    .materialize(uid, None)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?
  {
    Some(view) => compute_etag(&view),
    None => compute_etag_from_pairs(&mut new_pairs),
  };

  let status = if is_new {
    StatusCode::CREATED
  } else {
    StatusCode::NO_CONTENT
  };
  Ok((status, [(header::ETAG, new_etag)]).into_response())
}

/// Strip surrounding double-quotes from an ETag value.
///
/// `If-Match` headers may carry ETags with or without the surrounding `"`
/// required by RFC 7232. Normalise before comparing so both forms are accepted.
fn strip_etag_quotes(s: &str) -> &str { s.trim_matches('"') }
