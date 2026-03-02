//! Handlers for `/subjects` endpoints.
//!
//! | Method | Path | Notes |
//! |--------|------|-------|
//! | `GET`  | `/subjects` | Optional `?kind=person\|organization\|group` |
//! | `POST` | `/subjects` | Body: `{"kind":"person"}` |
//! | `GET`  | `/subjects/:id` | 404 if not found |

use std::sync::Arc;

use axum::{
  Json,
  extract::{Path, Query, State},
  http::StatusCode,
  response::IntoResponse,
};
use kith_core::{
  store::ContactStore,
  subject::{Subject, SubjectKind},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;

// ─── List ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListParams {
  pub kind: Option<SubjectKind>,
}

/// `GET /subjects[?kind=<kind>]`
pub async fn list<S>(
  State(store): State<Arc<S>>,
  Query(params): Query<ListParams>,
) -> Result<Json<Vec<Subject>>, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let subjects = store
    .list_subjects(params.kind)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?;
  Ok(Json(subjects))
}

// ─── Create ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateBody {
  pub kind: SubjectKind,
}

/// `POST /subjects` — body: `{"kind":"person"}`
pub async fn create<S>(
  State(store): State<Arc<S>>,
  Json(body): Json<CreateBody>,
) -> Result<impl IntoResponse, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let subject = store
    .add_subject(body.kind)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?;
  Ok((StatusCode::CREATED, Json(subject)))
}

// ─── Get one ──────────────────────────────────────────────────────────────────

/// `GET /subjects/:id`
pub async fn get_one<S>(
  State(store): State<Arc<S>>,
  Path(id): Path<Uuid>,
) -> Result<Json<Subject>, ApiError>
where
  S: ContactStore,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let subject = store
    .get_subject(id)
    .await
    .map_err(|e| ApiError::Store(Box::new(e)))?
    .ok_or_else(|| ApiError::NotFound(format!("subject {id} not found")))?;
  Ok(Json(subject))
}
