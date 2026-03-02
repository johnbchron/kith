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

  let if_none_match = headers
    .get(header::IF_NONE_MATCH)
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
  } else {
    // If-None-Match: * means "fail if the resource exists and is visible".
    if if_none_match.as_deref() == Some("*") {
      let visible = state
        .store
        .materialize(uid, None)
        .await
        .map_err(|e| Error::Store(Box::new(e)))?
        .map(|v| !v.active_facts.is_empty())
        .unwrap_or(false);
      if visible {
        tracing::warn!(
          uid = %uid,
          if_none_match = "*",
          "PUT rejected: resource already exists (If-None-Match: *)",
        );
        return Err(Error::PreconditionFailed);
      }
    }

    if let Some(ref etag_header) = if_match {
      let view = state
        .store
        .materialize(uid, None)
        .await
        .map_err(|e| Error::Store(Box::new(e)))?
        .ok_or(Error::NotFound)?;
      let current_etag = compute_etag(&view);
      if strip_etag_quotes(&current_etag) != strip_etag_quotes(etag_header) {
        tracing::warn!(
          uid = %uid,
          if_match = %etag_header,
          current_etag = %current_etag,
          "PUT rejected: ETag mismatch (If-Match)",
        );
        return Err(Error::PreconditionFailed);
      }
    }
  }

  let current_view = state
    .store
    .materialize(uid, None)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?;

  let result = diff::diff(body, uid, "carddav-put", current_view.as_ref())
    .map_err(|e| {
      // A parse error here means the client sent a malformed vCard — that is
      // a 400, not a 500.  Log a truncated excerpt so the problem vCard can
      // be identified in the logs without emitting the full (potentially
      // large) body.
      let excerpt: String = body.chars().take(256).collect();
      tracing::warn!(
        uid = %uid,
        error = %e,
        vcard_excerpt = %excerpt,
        "PUT rejected: vCard parse error",
      );
      Error::BadRequest(format!("vCard parse error: {e}"))
    })?;

  let mut new_pairs = Vec::new();
  for new_fact in result.new_facts {
    let recorded = state
      .store
      .record_fact(new_fact)
      .await
      .map_err(|e| Error::Store(Box::new(e)))?;
    new_pairs.push((recorded.fact_id, recorded.recorded_at));
  }

  // Apply supersessions and retractions with idempotency recovery.
  //
  // If any operation fails (e.g. due to a concurrent PUT for the same
  // contact), re-materialize and re-diff to check whether the store already
  // matches the incoming vCard.  If so, the concurrent writer already did the
  // work and this PUT is a no-op — skip the remaining operations and proceed
  // to ETag computation.  If the store still diverges, the error is genuine
  // and we propagate it.
  let mut idempotent_success = false;

  'supersessions: for (old_id, replacement) in result.supersessions {
    match state.store.supersede(old_id, replacement).await {
      Ok((_, new_fact)) => {
        new_pairs.push((new_fact.fact_id, new_fact.recorded_at));
      }
      Err(e) => {
        let fresh_view = state
          .store
          .materialize(uid, None)
          .await
          .map_err(|me| Error::Store(Box::new(me)))?;
        let re_diff =
          diff::diff(body, uid, "carddav-put", fresh_view.as_ref())
            .map_err(|de| {
              Error::BadRequest(format!("vCard parse error: {de}"))
            })?;
        if re_diff.new_facts.is_empty()
          && re_diff.supersessions.is_empty()
          && re_diff.retractions.is_empty()
        {
          tracing::debug!(
            uid = %uid,
            "PUT idempotent: concurrent write already applied supersessions",
          );
          idempotent_success = true;
          break 'supersessions;
        }
        return Err(Error::Store(Box::new(e)));
      }
    }
  }

  if !idempotent_success {
    for fact_id in result.retractions {
      match state
        .store
        .retract(fact_id, Some("Superseded by CardDAV PUT".to_string()))
        .await
      {
        Ok(_) => {}
        Err(e) => {
          let fresh_view = state
            .store
            .materialize(uid, None)
            .await
            .map_err(|me| Error::Store(Box::new(me)))?;
          let re_diff =
            diff::diff(body, uid, "carddav-put", fresh_view.as_ref())
              .map_err(|de| {
                Error::BadRequest(format!("vCard parse error: {de}"))
              })?;
          if re_diff.new_facts.is_empty()
            && re_diff.supersessions.is_empty()
            && re_diff.retractions.is_empty()
          {
            tracing::debug!(
              uid = %uid,
              "PUT idempotent: concurrent write already applied retractions",
            );
            break;
          }
          return Err(Error::Store(Box::new(e)));
        }
      }
    }
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
