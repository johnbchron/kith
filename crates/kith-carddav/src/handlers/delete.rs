//! DELETE handler — retract all active facts for a contact.
//!
//! The subject row itself is preserved (subjects are permanent envelopes).

use axum::{
  http::StatusCode,
  response::{IntoResponse, Response},
};
use kith_core::store::ContactStore;

use crate::{
  AppState,
  error::Error,
  handlers::propfind::parse_uid,
};

pub async fn handler<S>(
  state:   &AppState<S>,
  uid_vcf: &str,
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let uid = parse_uid(uid_vcf)?;

  state.store
    .get_subject(uid)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?
    .ok_or(Error::NotFound)?;

  let facts = state.store
    .get_facts(uid, None, false)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?;

  for rf in facts {
    state.store
      .retract(rf.fact.fact_id, Some("Deleted via CardDAV".to_string()))
      .await
      .map_err(|e| Error::Store(Box::new(e)))?;
  }

  Ok(StatusCode::NO_CONTENT.into_response())
}
