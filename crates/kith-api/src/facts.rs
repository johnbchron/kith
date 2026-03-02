//! Handlers for `/facts` endpoints.
//!
//! | Method | Path | Notes |
//! |--------|------|-------|
//! | `GET`  | `/facts` | `?subject_id` required; optional `fact_type`, `as_of`, `include_inactive` |
//! | `GET`  | `/facts/:id` | Single resolved fact |
//! | `POST` | `/facts` | Body: [`NewFactBody`]; returns 201 + stored fact |
//! | `POST` | `/facts/:id/supersede` | Body: [`NewFactBody`]; returns new resolved fact |
//! | `POST` | `/facts/:id/retract` | Body: `{"reason":"..."}` |

use std::sync::Arc;

use axum::{
  Json,
  extract::{Path, Query, State},
  http::StatusCode,
  response::IntoResponse,
};
use chrono::{DateTime, Utc};
use kith_core::{
  fact::{Confidence, EffectiveDate, FactValue, NewFact, RecordingContext},
  lifecycle::{FactStatus, ResolvedFact, Retraction},
  store::ContactStore,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;

// ─── List ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListParams {
  /// Required: the subject whose facts to return.
  pub subject_id:       Uuid,
  /// If set, restrict to facts with this type discriminant (e.g. `"email"`).
  pub fact_type:        Option<String>,
  /// Point-in-time filter on `recorded_at`. Defaults to now.
  pub as_of:            Option<DateTime<Utc>>,
  /// If `true`, also return superseded and retracted facts. Default `false`.
  #[serde(default)]
  pub include_inactive: bool,
}

/// `GET /facts?subject_id=<id>[&fact_type=...][&as_of=...][&include_inactive=true]`
pub async fn list<S>(
  State(store): State<Arc<S>>,
  Query(params): Query<ListParams>,
) -> Result<Json<Vec<ResolvedFact>>, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let mut facts = store
    .get_facts(params.subject_id, params.as_of, params.include_inactive)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?;

  if let Some(ft) = &params.fact_type {
    facts.retain(|rf| rf.fact.value.discriminant() == ft.as_str());
  }

  Ok(Json(facts))
}

// ─── Get one ──────────────────────────────────────────────────────────────────

/// `GET /facts/:id`
pub async fn get_one<S>(
  State(store): State<Arc<S>>,
  Path(id): Path<Uuid>,
) -> Result<Json<ResolvedFact>, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let fact = store
    .get_fact(id)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?
    .ok_or_else(|| ApiError::NotFound(format!("fact {id} not found")))?;
  Ok(Json(fact))
}

// ─── Create ───────────────────────────────────────────────────────────────────

/// JSON body accepted by `POST /facts` and `POST /facts/:id/supersede`.
#[derive(Debug, Deserialize)]
pub struct NewFactBody {
  pub subject_id:        Uuid,
  pub value:             FactValue,
  pub effective_at:      Option<EffectiveDate>,
  pub effective_until:   Option<EffectiveDate>,
  pub source:            Option<String>,
  pub confidence:        Option<Confidence>,
  pub recording_context: Option<RecordingContext>,
  #[serde(default)]
  pub tags:              Vec<String>,
}

impl From<NewFactBody> for NewFact {
  fn from(b: NewFactBody) -> Self {
    NewFact {
      subject_id:        b.subject_id,
      value:             b.value,
      effective_at:      b.effective_at,
      effective_until:   b.effective_until,
      source:            b.source,
      confidence:        b.confidence.unwrap_or_default(),
      recording_context: b.recording_context.unwrap_or_default(),
      tags:              b.tags,
    }
  }
}

/// `POST /facts` — returns 201 + the stored [`Fact`](kith_core::fact::Fact).
pub async fn create<S>(
  State(store): State<Arc<S>>,
  Json(body): Json<NewFactBody>,
) -> Result<impl IntoResponse, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let fact = store
    .record_fact(NewFact::from(body))
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?;
  Ok((StatusCode::CREATED, Json(fact)))
}

// ─── Supersede ────────────────────────────────────────────────────────────────

/// `POST /facts/:id/supersede` — body is the replacement [`NewFactBody`].
///
/// Returns the newly-recorded replacement fact as a [`ResolvedFact`] with
/// `Active` status.
pub async fn supersede_one<S>(
  State(store): State<Arc<S>>,
  Path(old_id): Path<Uuid>,
  Json(body): Json<NewFactBody>,
) -> Result<Json<ResolvedFact>, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let replacement = NewFact::from(body);
  let (_supersession, new_fact) = store
    .supersede(old_id, replacement)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?;
  Ok(Json(ResolvedFact {
    fact:   new_fact,
    status: FactStatus::Active,
  }))
}

// ─── Retract ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RetractBody {
  pub reason: Option<String>,
}

/// `POST /facts/:id/retract` — body: `{"reason":"..."}` (optional).
pub async fn retract_one<S>(
  State(store): State<Arc<S>>,
  Path(fact_id): Path<Uuid>,
  Json(body): Json<RetractBody>,
) -> Result<Json<Retraction>, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let retraction = store
    .retract(fact_id, body.reason)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?;
  Ok(Json(retraction))
}
