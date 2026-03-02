//! [`SqliteStore`] — the SQLite implementation of [`ContactStore`].

use std::path::Path;

use chrono::Utc;
use kith_core::{
  fact::{Fact, NewFact},
  lifecycle::{ContactView, ResolvedFact, Retraction, Supersession},
  store::{ContactStore, FactQuery},
  subject::{Subject, SubjectKind},
};
use rusqlite::OptionalExtension as _;
use uuid::Uuid;

use crate::{
  Error, Result,
  encode::{
    RawResolvedFact, RawSubject, encode_dt, encode_effective_date,
    encode_recording_context, encode_tags, encode_uuid,
  },
  schema::SCHEMA,
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

  /// Insert a fully-built [`Fact`] into the `facts` table.
  async fn insert_fact(&self, fact: &Fact) -> Result<()> {
    let fact_id_str = encode_uuid(fact.fact_id);
    let subject_id_str = encode_uuid(fact.subject_id);
    let fact_type = fact.value.discriminant().to_owned();
    let value_json_str = fact.value.to_json()?.to_string();
    let recorded_at_str = encode_dt(fact.recorded_at);
    let effective_at_str = fact
      .effective_at
      .as_ref()
      .map(encode_effective_date)
      .transpose()?;
    let effective_until_str = fact
      .effective_until
      .as_ref()
      .map(encode_effective_date)
      .transpose()?;
    let confidence_str = fact.confidence.to_string();
    let recording_ctx_str = encode_recording_context(&fact.recording_context)?;
    let tags_str = encode_tags(&fact.tags)?;
    let source = fact.source.clone();

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
    self.add_subject_with_id(Uuid::new_v4(), kind).await
  }

  async fn add_subject_with_id(
    &self,
    id: Uuid,
    kind: SubjectKind,
  ) -> Result<Subject> {
    let subject = Subject {
      subject_id: id,
      created_at: Utc::now(),
      kind,
    };

    let id_str = encode_uuid(subject.subject_id);
    let at_str = encode_dt(subject.created_at);
    let kind_str = kind.to_string();

    self
      .conn
      .call(move |conn| {
        conn.execute(
          "INSERT INTO subjects (subject_id, created_at, kind) VALUES (?1, \
           ?2, ?3)",
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
        Ok(
          conn
            .query_row(
              "SELECT subject_id, created_at, kind FROM subjects WHERE \
               subject_id = ?1",
              rusqlite::params![id_str],
              |row| {
                Ok(RawSubject {
                  subject_id: row.get(0)?,
                  created_at: row.get(1)?,
                  kind:       row.get(2)?,
                })
              },
            )
            .optional()?,
        )
      })
      .await?;

    raw.map(RawSubject::into_subject).transpose()
  }

  async fn list_subjects(
    &self,
    kind: Option<SubjectKind>,
  ) -> Result<Vec<Subject>> {
    let kind_str = kind.map(|k| k.to_string());

    let raws: Vec<RawSubject> = self
      .conn
      .call(move |conn| {
        let mut stmt = conn.prepare(
          "SELECT subject_id, created_at, kind FROM subjects \
           WHERE (?1 IS NULL OR kind = ?1)",
        )?;
        let rows = stmt
          .query_map(rusqlite::params![kind_str.as_deref()], |row| {
            Ok(RawSubject {
              subject_id: row.get(0)?,
              created_at: row.get(1)?,
              kind:       row.get(2)?,
            })
          })?
          .collect::<rusqlite::Result<Vec<_>>>()?;
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

  // ── Single-fact lookup ────────────────────────────────────────────────────

  async fn get_fact(&self, id: Uuid) -> Result<Option<ResolvedFact>> {
    let id_str = encode_uuid(id);

    let raw: Option<RawResolvedFact> = self
      .conn
      .call(move |conn| {
        Ok(
          conn
            .query_row(
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
               WHERE f.fact_id = ?1",
              rusqlite::params![id_str],
              |row| {
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
              },
            )
            .optional()?,
        )
      })
      .await?;

    raw.map(RawResolvedFact::into_resolved).transpose()
  }

  // ── Lifecycle events ──────────────────────────────────────────────────────

  async fn supersede(
    &self,
    old_id: Uuid,
    replacement: NewFact,
  ) -> Result<(Supersession, Fact)> {
    // Build the replacement fact outside the closure so encoding errors
    // surface here, not inside conn.call.
    let new_fact = Fact {
      fact_id:           Uuid::new_v4(),
      subject_id:        replacement.subject_id,
      value:             replacement.value,
      recorded_at:       Utc::now(),
      effective_at:      replacement.effective_at,
      effective_until:   replacement.effective_until,
      source:            replacement.source,
      confidence:        replacement.confidence,
      recording_context: replacement.recording_context,
      tags:              replacement.tags,
    };

    if new_fact.fact_id == old_id {
      return Err(Error::SelfSupersession);
    }

    // Pre-encode all strings (encoding can fail) before moving into the closure.
    let new_fact_id_str = encode_uuid(new_fact.fact_id);
    let new_subject_id_str = encode_uuid(new_fact.subject_id);
    let fact_type = new_fact.value.discriminant().to_owned();
    let value_json_str = new_fact.value.to_json()?.to_string();
    let new_recorded_at_str = encode_dt(new_fact.recorded_at);
    let new_effective_at_str = new_fact
      .effective_at
      .as_ref()
      .map(encode_effective_date)
      .transpose()?;
    let new_effective_until_str = new_fact
      .effective_until
      .as_ref()
      .map(encode_effective_date)
      .transpose()?;
    let new_confidence_str = new_fact.confidence.to_string();
    let new_recording_ctx_str =
      encode_recording_context(&new_fact.recording_context)?;
    let new_tags_str = encode_tags(&new_fact.tags)?;
    let new_source = new_fact.source.clone();

    let supersession_id = Uuid::new_v4();
    let sup_recorded_at = Utc::now();
    let sup_id_str = encode_uuid(supersession_id);
    let old_id_str = encode_uuid(old_id);
    let sup_at_str = encode_dt(sup_recorded_at);

    enum SupersedeOutcome {
      NotFound,
      AlreadySuperseded,
      AlreadyRetracted,
      Done,
    }

    let outcome = self
      .conn
      .call(move |conn| {
        let tx = conn.transaction()?;

        // Lifecycle check — all three sub-queries inside the same transaction.
        let (exists_flag, sup_str, ret_str): (
          Option<i64>,
          Option<String>,
          Option<String>,
        ) = tx.query_row(
          "SELECT \
             (SELECT 1 FROM facts WHERE fact_id = ?1), \
             (SELECT new_fact_id FROM supersessions WHERE old_fact_id = ?1), \
             (SELECT retraction_id FROM retractions WHERE fact_id = ?1)",
          rusqlite::params![&old_id_str],
          |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;

        if exists_flag.is_none() {
          return Ok(SupersedeOutcome::NotFound);
        }
        if sup_str.is_some() {
          return Ok(SupersedeOutcome::AlreadySuperseded);
        }
        if ret_str.is_some() {
          return Ok(SupersedeOutcome::AlreadyRetracted);
        }

        // Insert the replacement fact inside the transaction.
        tx.execute(
          "INSERT INTO facts (
             fact_id, subject_id, fact_type, value_json, recorded_at,
             effective_at, effective_until, source,
             confidence, recording_context, tags
           ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
          rusqlite::params![
            &new_fact_id_str,
            &new_subject_id_str,
            &fact_type,
            &value_json_str,
            &new_recorded_at_str,
            &new_effective_at_str,
            &new_effective_until_str,
            &new_source,
            &new_confidence_str,
            &new_recording_ctx_str,
            &new_tags_str,
          ],
        )?;

        // Insert the supersession record.  A UNIQUE constraint violation on
        // old_fact_id means a concurrent task already superseded this fact.
        // Rolling back the transaction prevents an orphan replacement fact.
        match tx.execute(
          "INSERT INTO supersessions \
             (supersession_id, old_fact_id, new_fact_id, recorded_at) \
             VALUES (?1, ?2, ?3, ?4)",
          rusqlite::params![
            &sup_id_str,
            &old_id_str,
            &new_fact_id_str,
            &sup_at_str,
          ],
        ) {
          Ok(_) => {}
          Err(rusqlite::Error::SqliteFailure(ref err, _))
            if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
          {
            // tx drops here → rolls back the fact INSERT (no orphan fact).
            return Ok(SupersedeOutcome::AlreadySuperseded);
          }
          Err(e) => return Err(e.into()),
        }

        tx.commit()?;
        Ok(SupersedeOutcome::Done)
      })
      .await?;

    match outcome {
      SupersedeOutcome::NotFound => Err(Error::FactNotFound(old_id)),
      SupersedeOutcome::AlreadySuperseded => {
        Err(Error::AlreadySuperseded(old_id))
      }
      SupersedeOutcome::AlreadyRetracted => {
        Err(Error::AlreadyRetracted(old_id))
      }
      SupersedeOutcome::Done => {
        let supersession = Supersession {
          supersession_id,
          old_fact_id: old_id,
          new_fact_id: new_fact.fact_id,
          recorded_at: sup_recorded_at,
        };
        Ok((supersession, new_fact))
      }
    }
  }

  async fn retract(
    &self,
    fact_id: Uuid,
    reason: Option<String>,
  ) -> Result<Retraction> {
    let retraction_id = Uuid::new_v4();
    let ret_recorded_at = Utc::now();
    let ret_id_str = encode_uuid(retraction_id);
    let fact_id_str = encode_uuid(fact_id);
    let at_str = encode_dt(ret_recorded_at);
    let reason_for_insert = reason.clone();

    enum RetractOutcome {
      NotFound,
      AlreadySuperseded,
      AlreadyRetracted,
      Done,
    }

    let outcome = self
      .conn
      .call(move |conn| {
        let tx = conn.transaction()?;

        let (exists_flag, sup_str, ret_str): (
          Option<i64>,
          Option<String>,
          Option<String>,
        ) = tx.query_row(
          "SELECT \
             (SELECT 1 FROM facts WHERE fact_id = ?1), \
             (SELECT new_fact_id FROM supersessions WHERE old_fact_id = ?1), \
             (SELECT retraction_id FROM retractions WHERE fact_id = ?1)",
          rusqlite::params![&fact_id_str],
          |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;

        if exists_flag.is_none() {
          return Ok(RetractOutcome::NotFound);
        }
        if sup_str.is_some() {
          return Ok(RetractOutcome::AlreadySuperseded);
        }
        if ret_str.is_some() {
          return Ok(RetractOutcome::AlreadyRetracted);
        }

        // A UNIQUE constraint violation on fact_id means a concurrent task
        // already retracted this fact.
        match tx.execute(
          "INSERT INTO retractions \
             (retraction_id, fact_id, reason, recorded_at) \
             VALUES (?1, ?2, ?3, ?4)",
          rusqlite::params![&ret_id_str, &fact_id_str, &reason_for_insert, &at_str],
        ) {
          Ok(_) => {}
          Err(rusqlite::Error::SqliteFailure(ref err, _))
            if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
          {
            return Ok(RetractOutcome::AlreadyRetracted);
          }
          Err(e) => return Err(e.into()),
        }

        tx.commit()?;
        Ok(RetractOutcome::Done)
      })
      .await?;

    match outcome {
      RetractOutcome::NotFound => Err(Error::FactNotFound(fact_id)),
      RetractOutcome::AlreadySuperseded => {
        Err(Error::AlreadySuperseded(fact_id))
      }
      RetractOutcome::AlreadyRetracted => {
        Err(Error::AlreadyRetracted(fact_id))
      }
      RetractOutcome::Done => Ok(Retraction {
        retraction_id,
        fact_id,
        reason,
        recorded_at: ret_recorded_at,
      }),
    }
  }

  // ── Reads ─────────────────────────────────────────────────────────────────

  async fn get_facts(
    &self,
    subject_id: Uuid,
    as_of: Option<chrono::DateTime<Utc>>,
    include_inactive: bool,
  ) -> Result<Vec<ResolvedFact>> {
    let subject_id_str = encode_uuid(subject_id);
    let as_of_str = encode_dt(as_of.unwrap_or_else(Utc::now));

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
    as_of: Option<chrono::DateTime<Utc>>,
  ) -> Result<Option<ContactView>> {
    let subject = match self.get_subject(subject_id).await? {
      Some(s) => s,
      None => return Ok(None),
    };

    let as_of_resolved = as_of.unwrap_or_else(Utc::now);
    let active_facts = self
      .get_facts(subject_id, Some(as_of_resolved), false)
      .await?;

    Ok(Some(ContactView {
      subject,
      as_of: as_of_resolved,
      active_facts,
    }))
  }

  async fn collection_ctag(
    &self,
  ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let raw: Option<Option<String>> = self
      .conn
      .call(|conn| {
        Ok(
          conn
            .query_row(
              "SELECT MAX(ts) FROM (
                 SELECT f.recorded_at AS ts
                 FROM facts f
                 JOIN subjects s ON s.subject_id = f.subject_id
                 WHERE s.kind = 'person'
                 UNION ALL
                 SELECT r.recorded_at AS ts
                 FROM retractions r
                 JOIN facts f ON f.fact_id = r.fact_id
                 JOIN subjects s ON s.subject_id = f.subject_id
                 WHERE s.kind = 'person'
               )",
              [],
              |row| row.get::<_, Option<String>>(0),
            )
            .optional()?,
        )
      })
      .await?;

    raw
      .flatten()
      .map(|s| crate::encode::decode_dt(&s))
      .transpose()
  }

  async fn search(&self, query: &FactQuery) -> Result<Vec<Subject>> {
    // Phase 1: SQL LIKE over value_json + optional subject-kind filter.
    let text_pattern = query.text.as_deref().map(|t| format!("%{t}%"));
    let kind_str = query.kind.map(|k| k.to_string());
    let limit_val = query.limit.unwrap_or(100) as i64;
    let offset_val = query.offset.unwrap_or(0) as i64;

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
