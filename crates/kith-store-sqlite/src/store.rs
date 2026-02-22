//! [`SqliteStore`] — the SQLite implementation of [`ContactStore`].

use std::path::Path;

use chrono::Utc;
use rusqlite::OptionalExtension as _;
use uuid::Uuid;

use kith_core::{
  fact::{Fact, NewFact},
  lifecycle::{ContactView, Retraction, ResolvedFact, Supersession},
  store::{ContactStore, FactQuery},
  subject::{Subject, SubjectKind},
};

use crate::{
  encode::{
    encode_confidence, encode_dt, encode_effective_date, encode_recording_context,
    encode_subject_kind, encode_tags, encode_uuid, RawResolvedFact, RawSubject,
  },
  schema::SCHEMA,
  Error, Result,
};

// ─── Store ───────────────────────────────────────────────────────────────────

/// A Kith contact store backed by a single SQLite file.
///
/// Cloning is cheap — the inner connection is reference-counted.
#[derive(Clone)]
pub struct SqliteStore {
  conn: tokio_rusqlite::Connection,
}

impl SqliteStore {
  /// Open (or create) a store at `path` and run schema initialisation.
  pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
    let conn = tokio_rusqlite::Connection::open(path).await?;
    let store = Self { conn };
    store.init_schema().await?;
    Ok(store)
  }

  /// Open an in-memory store — useful for testing.
  pub async fn open_in_memory() -> Result<Self> {
    let conn = tokio_rusqlite::Connection::open_in_memory().await?;
    let store = Self { conn };
    store.init_schema().await?;
    Ok(store)
  }

  async fn init_schema(&self) -> Result<()> {
    self
      .conn
      .call(|conn| {
        conn.execute_batch(SCHEMA)?;
        Ok(())
      })
      .await?;
    Ok(())
  }

  /// Check that a fact exists and is not already in a lifecycle event table.
  ///
  /// Returns `(exists, superseded_by_uuid, retracted_id_uuid)`.
  async fn fact_lifecycle_check(
    &self,
    fact_id: Uuid,
  ) -> Result<(bool, Option<Uuid>, Option<Uuid>)> {
    let id_str = encode_uuid(fact_id);

    let (exists, sup_str, ret_str): (bool, Option<String>, Option<String>) = self
      .conn
      .call(move |conn| {
        let exists: bool = conn
          .query_row(
            "SELECT 1 FROM facts WHERE fact_id = ?1",
            rusqlite::params![id_str],
            |_| Ok(true),
          )
          .optional()?
          .unwrap_or(false);

        if !exists {
          return Ok((false, None, None));
        }

        let sup: Option<String> = conn
          .query_row(
            "SELECT new_fact_id FROM supersessions WHERE old_fact_id = ?1",
            rusqlite::params![id_str],
            |r| r.get(0),
          )
          .optional()?;

        let ret: Option<String> = conn
          .query_row(
            "SELECT retraction_id FROM retractions WHERE fact_id = ?1",
            rusqlite::params![id_str],
            |r| r.get(0),
          )
          .optional()?;

        Ok((true, sup, ret))
      })
      .await?;

    let superseded_by = sup_str
      .map(|s| Uuid::parse_str(&s))
      .transpose()
      .map_err(Error::Uuid)?;

    let retracted_id = ret_str
      .map(|s| Uuid::parse_str(&s))
      .transpose()
      .map_err(Error::Uuid)?;

    Ok((exists, superseded_by, retracted_id))
  }

  /// Insert a fully-built [`Fact`] into the `facts` table.
  async fn insert_fact(&self, fact: &Fact) -> Result<()> {
    let fact_id_str         = encode_uuid(fact.fact_id);
    let subject_id_str      = encode_uuid(fact.subject_id);
    let fact_type           = fact.value.discriminant().to_owned();
    let value_json_str      = fact.value.to_json()?.to_string();
    let recorded_at_str     = encode_dt(fact.recorded_at);
    let effective_at_str    = fact
      .effective_at
      .as_ref()
      .map(encode_effective_date)
      .transpose()?;
    let effective_until_str = fact
      .effective_until
      .as_ref()
      .map(encode_effective_date)
      .transpose()?;
    let confidence_str      = encode_confidence(fact.confidence).to_owned();
    let recording_ctx_str   = encode_recording_context(&fact.recording_context)?;
    let tags_str            = encode_tags(&fact.tags)?;
    let source              = fact.source.clone();

    self
      .conn
      .call(move |conn| {
        conn.execute(
          "INSERT INTO facts (
             fact_id, subject_id, fact_type, value_json, recorded_at,
             effective_at, effective_until, source,
             confidence, recording_context, tags
           ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
          rusqlite::params![
            fact_id_str,
            subject_id_str,
            fact_type,
            value_json_str,
            recorded_at_str,
            effective_at_str,
            effective_until_str,
            source,
            confidence_str,
            recording_ctx_str,
            tags_str,
          ],
        )?;
        Ok(())
      })
      .await?;
    Ok(())
  }
}

// ─── ContactStore impl ───────────────────────────────────────────────────────

impl ContactStore for SqliteStore {
  type Error = Error;

  // ── Subjects ──────────────────────────────────────────────────────────────

  async fn add_subject(&self, kind: SubjectKind) -> Result<Subject> {
    let subject = Subject {
      subject_id: Uuid::new_v4(),
      created_at: Utc::now(),
      kind,
    };

    let id_str   = encode_uuid(subject.subject_id);
    let at_str   = encode_dt(subject.created_at);
    let kind_str = encode_subject_kind(kind).to_owned();

    self
      .conn
      .call(move |conn| {
        conn.execute(
          "INSERT INTO subjects (subject_id, created_at, kind) VALUES (?1, ?2, ?3)",
          rusqlite::params![id_str, at_str, kind_str],
        )?;
        Ok(())
      })
      .await?;

    Ok(subject)
  }

  async fn get_subject(&self, id: Uuid) -> Result<Option<Subject>> {
    let id_str = encode_uuid(id);

    let raw: Option<RawSubject> = self
      .conn
      .call(move |conn| {
        Ok(conn
          .query_row(
            "SELECT subject_id, created_at, kind FROM subjects WHERE subject_id = ?1",
            rusqlite::params![id_str],
            |row| {
              Ok(RawSubject {
                subject_id: row.get(0)?,
                created_at: row.get(1)?,
                kind:       row.get(2)?,
              })
            },
          )
          .optional()?)
      })
      .await?;

    raw.map(RawSubject::into_subject).transpose()
  }

  async fn list_subjects(&self, kind: Option<SubjectKind>) -> Result<Vec<Subject>> {
    let kind_str = kind.map(encode_subject_kind).map(str::to_owned);

    let raws: Vec<RawSubject> = self
      .conn
      .call(move |conn| {
        let rows = if let Some(k) = kind_str {
          let mut stmt = conn
            .prepare("SELECT subject_id, created_at, kind FROM subjects WHERE kind = ?1")?;
          stmt
            .query_map(rusqlite::params![k], |row| {
              Ok(RawSubject {
                subject_id: row.get(0)?,
                created_at: row.get(1)?,
                kind:       row.get(2)?,
              })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
          let mut stmt = conn
            .prepare("SELECT subject_id, created_at, kind FROM subjects")?;
          stmt
            .query_map([], |row| {
              Ok(RawSubject {
                subject_id: row.get(0)?,
                created_at: row.get(1)?,
                kind:       row.get(2)?,
              })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
      })
      .await?;

    raws.into_iter().map(RawSubject::into_subject).collect()
  }

  // ── Facts — append-only writes ────────────────────────────────────────────

  async fn record_fact(&self, input: NewFact) -> Result<Fact> {
    let fact = Fact {
      fact_id:           Uuid::new_v4(),
      subject_id:        input.subject_id,
      value:             input.value,
      recorded_at:       Utc::now(),
      effective_at:      input.effective_at,
      effective_until:   input.effective_until,
      source:            input.source,
      confidence:        input.confidence,
      recording_context: input.recording_context,
      tags:              input.tags,
    };

    self.insert_fact(&fact).await?;
    Ok(fact)
  }

  // ── Lifecycle events ──────────────────────────────────────────────────────

  async fn supersede(
    &self,
    old_id:      Uuid,
    replacement: NewFact,
  ) -> Result<(Supersession, Fact)> {
    let (exists, superseded_by, retracted_id) =
      self.fact_lifecycle_check(old_id).await?;

    if !exists {
      return Err(Error::FactNotFound(old_id));
    }
    if superseded_by.is_some() {
      return Err(Error::AlreadySuperseded(old_id));
    }
    if retracted_id.is_some() {
      return Err(Error::AlreadyRetracted(old_id));
    }

    let new_fact = self.record_fact(replacement).await?;

    if new_fact.fact_id == old_id {
      return Err(Error::SelfSupersession);
    }

    let supersession = Supersession {
      supersession_id: Uuid::new_v4(),
      old_fact_id:     old_id,
      new_fact_id:     new_fact.fact_id,
      recorded_at:     Utc::now(),
    };

    let sup_id_str = encode_uuid(supersession.supersession_id);
    let old_id_str = encode_uuid(old_id);
    let new_id_str = encode_uuid(new_fact.fact_id);
    let at_str     = encode_dt(supersession.recorded_at);

    self
      .conn
      .call(move |conn| {
        conn.execute(
          "INSERT INTO supersessions (supersession_id, old_fact_id, new_fact_id, recorded_at)
           VALUES (?1, ?2, ?3, ?4)",
          rusqlite::params![sup_id_str, old_id_str, new_id_str, at_str],
        )?;
        Ok(())
      })
      .await?;

    Ok((supersession, new_fact))
  }

  async fn retract(&self, fact_id: Uuid, reason: Option<String>) -> Result<Retraction> {
    let (exists, superseded_by, retracted_id) =
      self.fact_lifecycle_check(fact_id).await?;

    if !exists {
      return Err(Error::FactNotFound(fact_id));
    }
    if superseded_by.is_some() {
      return Err(Error::AlreadySuperseded(fact_id));
    }
    if retracted_id.is_some() {
      return Err(Error::AlreadyRetracted(fact_id));
    }

    let retraction = Retraction {
      retraction_id: Uuid::new_v4(),
      fact_id,
      reason:        reason.clone(),
      recorded_at:   Utc::now(),
    };

    let ret_id_str  = encode_uuid(retraction.retraction_id);
    let fact_id_str = encode_uuid(fact_id);
    let at_str      = encode_dt(retraction.recorded_at);

    self
      .conn
      .call(move |conn| {
        conn.execute(
          "INSERT INTO retractions (retraction_id, fact_id, reason, recorded_at)
           VALUES (?1, ?2, ?3, ?4)",
          rusqlite::params![ret_id_str, fact_id_str, reason, at_str],
        )?;
        Ok(())
      })
      .await?;

    Ok(retraction)
  }

  // ── Reads ─────────────────────────────────────────────────────────────────

  async fn get_facts(
    &self,
    subject_id:       Uuid,
    as_of:            Option<chrono::DateTime<Utc>>,
    include_inactive: bool,
  ) -> Result<Vec<ResolvedFact>> {
    let subject_id_str = encode_uuid(subject_id);
    let as_of_str      = encode_dt(as_of.unwrap_or_else(Utc::now));

    let raws: Vec<RawResolvedFact> = self
      .conn
      .call(move |conn| {
        let mut stmt = conn.prepare(
          "SELECT
             f.fact_id, f.subject_id, f.fact_type, f.value_json,
             f.recorded_at, f.effective_at, f.effective_until,
             f.source, f.confidence, f.recording_context, f.tags,
             s.new_fact_id   AS superseded_by,
             s.recorded_at   AS superseded_at,
             r.reason        AS retraction_reason,
             r.recorded_at   AS retracted_at
           FROM facts f
           LEFT JOIN supersessions s ON s.old_fact_id = f.fact_id
           LEFT JOIN retractions   r ON r.fact_id     = f.fact_id
           WHERE f.subject_id = ?1
             AND f.recorded_at <= ?2",
        )?;

        let rows = stmt
          .query_map(rusqlite::params![subject_id_str, as_of_str], |row| {
            Ok(RawResolvedFact {
              fact_id:           row.get(0)?,
              subject_id:        row.get(1)?,
              fact_type:         row.get(2)?,
              value_json:        row.get(3)?,
              recorded_at:       row.get(4)?,
              effective_at:      row.get(5)?,
              effective_until:   row.get(6)?,
              source:            row.get(7)?,
              confidence:        row.get(8)?,
              recording_context: row.get(9)?,
              tags:              row.get(10)?,
              superseded_by:     row.get(11)?,
              superseded_at:     row.get(12)?,
              retraction_reason: row.get(13)?,
              retracted_at:      row.get(14)?,
            })
          })?
          .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
      })
      .await?;

    let mut facts: Vec<ResolvedFact> = raws
      .into_iter()
      .map(RawResolvedFact::into_resolved)
      .collect::<Result<_>>()?;

    if !include_inactive {
      facts.retain(|rf| rf.status.is_active());
    }

    Ok(facts)
  }

  async fn materialize(
    &self,
    subject_id: Uuid,
    as_of:      Option<chrono::DateTime<Utc>>,
  ) -> Result<Option<ContactView>> {
    let subject = match self.get_subject(subject_id).await? {
      Some(s) => s,
      None    => return Ok(None),
    };

    let as_of_resolved = as_of.unwrap_or_else(Utc::now);
    let active_facts   = self.get_facts(subject_id, Some(as_of_resolved), false).await?;

    Ok(Some(ContactView { subject, as_of: as_of_resolved, active_facts }))
  }

  async fn search(&self, query: &FactQuery) -> Result<Vec<Subject>> {
    // Phase 1: SQL LIKE over value_json + optional subject-kind filter.
    let text_pattern = query.text.as_deref().map(|t| format!("%{t}%"));
    let kind_str     = query.kind.map(encode_subject_kind).map(str::to_owned);
    let limit_val    = query.limit.unwrap_or(100) as i64;
    let offset_val   = query.offset.unwrap_or(0) as i64;

    let raws: Vec<RawSubject> = self
      .conn
      .call(move |conn| {
        // Build WHERE clause dynamically.
        let mut conds: Vec<&'static str> = vec![];
        if text_pattern.is_some() {
          conds.push("f.value_json LIKE ?1");
        }
        if kind_str.is_some() {
          conds.push("s.kind = ?2");
        }

        let where_clause = if conds.is_empty() {
          String::new()
        } else {
          format!("WHERE {}", conds.join(" AND "))
        };

        let sql = format!(
          "SELECT DISTINCT s.subject_id, s.created_at, s.kind
           FROM subjects s
           LEFT JOIN facts f ON f.subject_id = s.subject_id
           {where_clause}
           LIMIT ?3 OFFSET ?4"
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
          .query_map(
            rusqlite::params![
              text_pattern.as_deref(),
              kind_str.as_deref(),
              limit_val,
              offset_val,
            ],
            |row| {
              Ok(RawSubject {
                subject_id: row.get(0)?,
                created_at: row.get(1)?,
                kind:       row.get(2)?,
              })
            },
          )?
          .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
      })
      .await?;

    raws.into_iter().map(RawSubject::into_subject).collect()
  }
}
