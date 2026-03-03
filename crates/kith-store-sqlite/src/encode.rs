//! Encoding helpers between Rust domain types and the plain-text
//! representations stored in SQLite columns, plus `FromRow` row structs.
//!
//! All timestamps are stored as RFC 3339 strings. Structured fields
//! (EffectiveDate, Confidence, RecordingContext, tags) are stored as compact
//! JSON. UUIDs are stored as hyphenated lowercase strings.

use chrono::{DateTime, Utc};
use kith_core::{
  fact::{Confidence, EffectiveDate, Fact, FactValue, RecordingContext},
  lifecycle::{FactStatus, ResolvedFact},
  subject::{Subject, SubjectKind},
};
use uuid::Uuid;

use crate::{Error, Result};

// ─── Uuid ─────────────────────────────────────────────────────────────────────

pub fn encode_uuid(id: Uuid) -> String { id.hyphenated().to_string() }

pub fn decode_uuid(s: &str) -> Result<Uuid> { Ok(Uuid::parse_str(s)?) }

// ─── DateTime<Utc> ────────────────────────────────────────────────────────────

pub fn encode_dt(dt: DateTime<Utc>) -> String { dt.to_rfc3339() }

pub fn decode_dt(s: &str) -> Result<DateTime<Utc>> {
  DateTime::parse_from_rfc3339(s)
    .map(|dt| dt.with_timezone(&Utc))
    .map_err(|e| Error::DateParse(e.to_string()))
}

// ─── Row types ────────────────────────────────────────────────────────────────

/// Raw strings read directly from a `facts` row joined with lifecycle tables.
#[derive(sqlx::FromRow)]
pub struct RawResolvedFact {
  // facts columns
  pub fact_id:           String,
  pub subject_id:        String,
  pub fact_type:         String,
  pub value_json:        String,
  pub recorded_at:       String,
  pub effective_at:      Option<String>,
  pub effective_until:   Option<String>,
  pub source:            Option<String>,
  pub confidence:        String,
  pub recording_context: String,
  pub tags:              String,
  // supersessions join
  pub superseded_by:     Option<String>,
  pub superseded_at:     Option<String>,
  // retractions join
  pub retraction_reason: Option<String>,
  pub retracted_at:      Option<String>,
}

impl RawResolvedFact {
  pub fn into_resolved(self) -> Result<ResolvedFact> {
    let fact_id = decode_uuid(&self.fact_id)?;
    let subject_id = decode_uuid(&self.subject_id)?;
    let recorded_at = decode_dt(&self.recorded_at)?;

    let value_json: serde_json::Value = serde_json::from_str(&self.value_json)?;
    let value = FactValue::from_parts(&self.fact_type, value_json)?;

    let effective_at = self
      .effective_at
      .as_deref()
      .map(|s| serde_json::from_str::<EffectiveDate>(s).map_err(Error::from))
      .transpose()?;

    let effective_until = self
      .effective_until
      .as_deref()
      .map(|s| serde_json::from_str::<EffectiveDate>(s).map_err(Error::from))
      .transpose()?;

    let confidence = self
      .confidence
      .parse::<Confidence>()
      .map_err(|e| Error::DateParse(e.to_string()))?;
    let recording_context =
      serde_json::from_str::<RecordingContext>(&self.recording_context)?;
    let tags = serde_json::from_str::<Vec<String>>(&self.tags)?;

    let fact = Fact {
      fact_id,
      subject_id,
      value,
      recorded_at,
      effective_at,
      effective_until,
      source: self.source,
      confidence,
      recording_context,
      tags,
    };

    let status = if let (Some(by_str), Some(at_str)) =
      (self.superseded_by, self.superseded_at)
    {
      FactStatus::Superseded {
        by: decode_uuid(&by_str)?,
        at: decode_dt(&at_str)?,
      }
    } else if let Some(at_str) = self.retracted_at {
      FactStatus::Retracted {
        reason: self.retraction_reason,
        at:     decode_dt(&at_str)?,
      }
    } else {
      FactStatus::Active
    };

    Ok(ResolvedFact { fact, status })
  }
}

/// Raw strings read directly from a `subjects` row.
#[derive(sqlx::FromRow)]
pub struct RawSubject {
  pub subject_id: String,
  pub created_at: String,
  pub kind:       String,
}

impl RawSubject {
  pub fn into_subject(self) -> Result<Subject> {
    Ok(Subject {
      subject_id: decode_uuid(&self.subject_id)?,
      created_at: decode_dt(&self.created_at)?,
      kind:       self
        .kind
        .parse::<SubjectKind>()
        .map_err(|e| Error::DateParse(e.to_string()))?,
    })
  }
}
