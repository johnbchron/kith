//! Handler for `GET /search`.
//!
//! Query params map directly to [`FactQuery`] fields.
//! `fact_types` and `tags` are accepted as comma-separated strings.

use std::sync::Arc;

use axum::{
  Json,
  extract::{Query, State},
};
use chrono::{DateTime, Utc};
use kith_core::{
  fact::Confidence,
  store::{ContactStore, FactQuery},
  subject::{Subject, SubjectKind},
};
use serde::Deserialize;

use crate::error::ApiError;

#[derive(Debug, Deserialize, Default)]
pub struct SearchParams {
  /// Free-text filter applied over serialised fact values.
  pub text:            Option<String>,
  /// Restrict to subjects of a specific kind.
  pub kind:            Option<SubjectKind>,
  /// Comma-separated fact type discriminants, e.g. `email,phone`.
  pub fact_types:      Option<String>,
  /// Comma-separated tags; all must be present.
  pub tags:            Option<String>,
  pub confidence:      Option<Confidence>,
  pub recorded_after:  Option<DateTime<Utc>>,
  pub recorded_before: Option<DateTime<Utc>>,
  pub limit:           Option<usize>,
  pub offset:          Option<usize>,
}

/// `GET /search[?text=...][&kind=...][&fact_types=...][&tags=...][&limit=...][&offset=...]`
pub async fn handler<S>(
  State(store): State<Arc<S>>,
  Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Subject>>, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let query = FactQuery {
    text:            params.text,
    kind:            params.kind,
    fact_types:      params
      .fact_types
      .map(|s| s.split(',').map(|t| t.trim().to_owned()).collect())
      .unwrap_or_default(),
    tags:            params
      .tags
      .map(|s| s.split(',').map(|t| t.trim().to_owned()).collect())
      .unwrap_or_default(),
    confidence:      params.confidence,
    recorded_after:  params.recorded_after,
    recorded_before: params.recorded_before,
    limit:           params.limit,
    offset:          params.offset,
  };

  let subjects = store
    .search(&query)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?;
  Ok(Json(subjects))
}
