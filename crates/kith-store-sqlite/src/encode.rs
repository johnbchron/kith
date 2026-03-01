//! Encoding and decoding helpers between Rust domain types and the plain-text
//! representations stored in SQLite columns.
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

// ─── DateTime<Utc>
// ────────────────────────────────────────────────────────────

pub fn encode_dt(dt: DateTime<Utc>) -> String { dt.to_rfc3339() }

pub fn decode_dt(s: &str) -> Result<DateTime<Utc>> {
  DateTime::parse_from_rfc3339(s)
    .map(|dt| dt.with_timezone(&Utc))
    .map_err(|e| Error::DateParse(e.to_string()))
}

// ─── SubjectKind
// ──────────────────────────────────────────────────────────────

pub fn encode_subject_kind(k: SubjectKind) -> &'static str {
  match k {
    SubjectKind::Person => "person",
    SubjectKind::Organization => "organization",
    SubjectKind::Group => "group",
  }
}

pub fn decode_subject_kind(s: &str) -> Result<SubjectKind> {
  match s {
    "person" => Ok(SubjectKind::Person),
    "organization" => Ok(SubjectKind::Organization),
    "group" => Ok(SubjectKind::Group),
    other => Err(Error::DateParse(format!("unknown subject kind: {other:?}"))),
  }
}

// ─── EffectiveDate
// ────────────────────────────────────────────────────────────

pub fn encode_effective_date(d: &EffectiveDate) -> Result<String> {
  Ok(serde_json::to_string(d)?)
}

pub fn decode_effective_date(s: &str) -> Result<EffectiveDate> {
  Ok(serde_json::from_str(s)?)
}

// ─── Confidence
// ───────────────────────────────────────────────────────────────

pub fn encode_confidence(c: Confidence) -> &'static str {
  match c {
    Confidence::Certain => "certain",
    Confidence::Probable => "probable",
    Confidence::Rumored => "rumored",
  }
}

pub fn decode_confidence(s: &str) -> Result<Confidence> {
  match s {
    "certain" => Ok(Confidence::Certain),
    "probable" => Ok(Confidence::Probable),
    "rumored" => Ok(Confidence::Rumored),
    other => Err(Error::DateParse(format!("unknown confidence: {other:?}"))),
  }
}

// ─── RecordingContext
// ─────────────────────────────────────────────────────────

pub fn encode_recording_context(rc: &RecordingContext) -> Result<String> {
  Ok(serde_json::to_string(rc)?)
}

pub fn decode_recording_context(s: &str) -> Result<RecordingContext> {
  Ok(serde_json::from_str(s)?)
}

// ─── Tags ────────────────────────────────────────────────────────────────────

pub fn encode_tags(tags: &[String]) -> Result<String> {
  Ok(serde_json::to_string(tags)?)
}

pub fn decode_tags(s: &str) -> Result<Vec<String>> {
  Ok(serde_json::from_str(s)?)
}

// ─── Row types ───────────────────────────────────────────────────────────────

/// Raw strings read directly from a `facts` row joined with lifecycle tables.
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
      .map(decode_effective_date)
      .transpose()?;

    let effective_until = self
      .effective_until
      .as_deref()
      .map(decode_effective_date)
      .transpose()?;

    let confidence = decode_confidence(&self.confidence)?;
    let recording_context = decode_recording_context(&self.recording_context)?;
    let tags = decode_tags(&self.tags)?;

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
      kind:       decode_subject_kind(&self.kind)?,
    })
  }
}
