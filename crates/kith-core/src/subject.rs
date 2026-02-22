//! Subject — the thin envelope that aggregates facts.
//!
//! A subject holds only identity metadata. The "contact" view is assembled on
//! read by materialising all active facts for the subject.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The kind of entity a subject represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubjectKind {
  Person,
  Organization,
  Group,
}

/// A thin envelope that owns a UUID and a creation timestamp.
/// All meaningful information about the entity lives in its facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subject {
  pub subject_id: Uuid,
  pub created_at: DateTime<Utc>,
  pub kind:       SubjectKind,
}
