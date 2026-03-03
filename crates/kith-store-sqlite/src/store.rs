//! [`SqliteStore`] — the SQLite implementation of [`ContactStore`].

use std::path::Path;

use chrono::Utc;
use kith_core::{
  fact::{Fact, NewFact},
  lifecycle::{ContactView, ResolvedFact, Retraction, Supersession},
  store::{ContactStore, FactQuery},
  subject::{Subject, SubjectKind},
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use uuid::Uuid;

use crate::{
  Error, Result,
  encode::{
    RawResolvedFact, RawSubject, decode_dt, encode_dt, encode_uuid,
  },
};

// ─── Store ───────────────────────────────────────────────────────────────────

/// A Kith contact store backed by a single SQLite file.
///
/// Cloning is cheap — the inner pool is reference-counted.
#[derive(Clone)]
pub struct SqliteStore {
  pool: sqlx::SqlitePool,
}

impl SqliteStore {
  /// Open (or create) a store at `path` and run schema migrations.
  pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
    let opts = SqliteConnectOptions::new()
      .filename(path.as_ref())
      .journal_mode(SqliteJournalMode::Wal)
      .foreign_keys(true)
      .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(Self { pool })
  }

  /// Open an in-memory store — useful for testing.
  pub async fn open_in_memory() -> Result<Self> {
    let opts = SqliteConnectOptions::new()
      .journal_mode(SqliteJournalMode::Wal)
      .foreign_keys(true)
      .in_memory(true);
    // Pin to one connection so all operations share the same in-memory DB.
    let pool = SqlitePoolOptions::new()
      .max_connections(1)
      .connect_with(opts)
      .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(Self { pool })
  }

  /// Insert a fully-built [`Fact`] into the `facts` table.
  async fn insert_fact(&self, fact: &Fact) -> Result<()> {
    self.insert_fact_with_exec(&self.pool, fact).await
  }

  /// Insert a fact using any sqlx executor (pool or transaction).
  async fn insert_fact_with_exec<'e, E>(
    &self,
    exec: E,
    fact: &Fact,
  ) -> Result<()>
  where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
  {
    sqlx::query(
      "INSERT INTO facts (
         fact_id, subject_id, fact_type, value_json, recorded_at,
         effective_at, effective_until, source,
         confidence, recording_context, tags
       ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(encode_uuid(fact.fact_id))
    .bind(encode_uuid(fact.subject_id))
    .bind(fact.value.discriminant())
    .bind(fact.value.to_json()?.to_string())
    .bind(encode_dt(fact.recorded_at))
    .bind(
      fact
        .effective_at
        .as_ref()
        .map(|d| serde_json::to_string(d))
        .transpose()?,
    )
    .bind(
      fact
        .effective_until
        .as_ref()
        .map(|d| serde_json::to_string(d))
        .transpose()?,
    )
    .bind(&fact.source)
    .bind(fact.confidence.to_string())
    .bind(serde_json::to_string(&fact.recording_context)?)
    .bind(serde_json::to_string(&fact.tags)?)
    .execute(exec)
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

    sqlx::query(
      "INSERT INTO subjects (subject_id, created_at, kind) VALUES (?, ?, ?)",
    )
    .bind(encode_uuid(subject.subject_id))
    .bind(encode_dt(subject.created_at))
    .bind(kind.to_string())
    .execute(&self.pool)
    .await?;

    Ok(subject)
  }

  async fn get_subject(&self, id: Uuid) -> Result<Option<Subject>> {
    let raw: Option<RawSubject> = sqlx::query_as(
      "SELECT subject_id, created_at, kind FROM subjects WHERE subject_id = ?",
    )
    .bind(encode_uuid(id))
    .fetch_optional(&self.pool)
    .await?;

    raw.map(RawSubject::into_subject).transpose()
  }

  async fn list_subjects(
    &self,
    kind: Option<SubjectKind>,
  ) -> Result<Vec<Subject>> {
    let kind_str = kind.map(|k| k.to_string());

    let raws: Vec<RawSubject> = sqlx::query_as(
      "SELECT subject_id, created_at, kind FROM subjects \
       WHERE (? IS NULL OR kind = ?)",
    )
    .bind(kind_str.as_deref())
    .bind(kind_str.as_deref())
    .fetch_all(&self.pool)
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
    let raw: Option<RawResolvedFact> = sqlx::query_as(
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
       WHERE f.fact_id = ?",
    )
    .bind(encode_uuid(id))
    .fetch_optional(&self.pool)
    .await?;

    raw.map(RawResolvedFact::into_resolved).transpose()
  }

  // ── Lifecycle events ──────────────────────────────────────────────────────

  async fn supersede(
    &self,
    old_id: Uuid,
    replacement: NewFact,
  ) -> Result<(Supersession, Fact)> {
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

    let old_id_str = encode_uuid(old_id);
    let supersession_id = Uuid::new_v4();
    let sup_recorded_at = Utc::now();

    let mut tx = self.pool.begin().await?;

    // Lifecycle check in a single round-trip.
    let (exists, sup_id, ret_id): (
      Option<i64>,
      Option<String>,
      Option<String>,
    ) = sqlx::query_as(
      "SELECT \
         (SELECT 1 FROM facts WHERE fact_id = ?), \
         (SELECT new_fact_id FROM supersessions WHERE old_fact_id = ?), \
         (SELECT retraction_id FROM retractions WHERE fact_id = ?)",
    )
    .bind(&old_id_str)
    .bind(&old_id_str)
    .bind(&old_id_str)
    .fetch_one(&mut *tx)
    .await?;

    if exists.is_none() {
      return Err(Error::FactNotFound(old_id));
    }
    if sup_id.is_some() {
      return Err(Error::AlreadySuperseded(old_id));
    }
    if ret_id.is_some() {
      return Err(Error::AlreadyRetracted(old_id));
    }

    // Insert replacement fact inside the transaction.
    self.insert_fact_with_exec(&mut *tx, &new_fact).await?;

    // Insert supersession. UNIQUE violation on old_fact_id → concurrent race.
    match sqlx::query(
      "INSERT INTO supersessions \
         (supersession_id, old_fact_id, new_fact_id, recorded_at) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(encode_uuid(supersession_id))
    .bind(&old_id_str)
    .bind(encode_uuid(new_fact.fact_id))
    .bind(encode_dt(sup_recorded_at))
    .execute(&mut *tx)
    .await
    {
      Ok(_) => {}
      Err(sqlx::Error::Database(e))
        if e.kind() == sqlx::error::ErrorKind::UniqueViolation =>
      {
        // tx drops here → auto-rollback (orphan fact prevention)
        return Err(Error::AlreadySuperseded(old_id));
      }
      Err(e) => return Err(e.into()),
    }

    tx.commit().await?;

    let supersession = Supersession {
      supersession_id,
      old_fact_id: old_id,
      new_fact_id: new_fact.fact_id,
      recorded_at: sup_recorded_at,
    };
    Ok((supersession, new_fact))
  }

  async fn retract(
    &self,
    fact_id: Uuid,
    reason: Option<String>,
  ) -> Result<Retraction> {
    let retraction_id = Uuid::new_v4();
    let ret_recorded_at = Utc::now();
    let fact_id_str = encode_uuid(fact_id);

    let mut tx = self.pool.begin().await?;

    let (exists, sup_id, ret_id): (
      Option<i64>,
      Option<String>,
      Option<String>,
    ) = sqlx::query_as(
      "SELECT \
         (SELECT 1 FROM facts WHERE fact_id = ?), \
         (SELECT new_fact_id FROM supersessions WHERE old_fact_id = ?), \
         (SELECT retraction_id FROM retractions WHERE fact_id = ?)",
    )
    .bind(&fact_id_str)
    .bind(&fact_id_str)
    .bind(&fact_id_str)
    .fetch_one(&mut *tx)
    .await?;

    if exists.is_none() {
      return Err(Error::FactNotFound(fact_id));
    }
    if sup_id.is_some() {
      return Err(Error::AlreadySuperseded(fact_id));
    }
    if ret_id.is_some() {
      return Err(Error::AlreadyRetracted(fact_id));
    }

    match sqlx::query(
      "INSERT INTO retractions \
         (retraction_id, fact_id, reason, recorded_at) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(encode_uuid(retraction_id))
    .bind(&fact_id_str)
    .bind(&reason)
    .bind(encode_dt(ret_recorded_at))
    .execute(&mut *tx)
    .await
    {
      Ok(_) => {}
      Err(sqlx::Error::Database(e))
        if e.kind() == sqlx::error::ErrorKind::UniqueViolation =>
      {
        return Err(Error::AlreadyRetracted(fact_id));
      }
      Err(e) => return Err(e.into()),
    }

    tx.commit().await?;

    Ok(Retraction {
      retraction_id,
      fact_id,
      reason,
      recorded_at: ret_recorded_at,
    })
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

    let raws: Vec<RawResolvedFact> = sqlx::query_as(
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
       WHERE f.subject_id = ?
         AND f.recorded_at <= ?",
    )
    .bind(subject_id_str)
    .bind(as_of_str)
    .fetch_all(&self.pool)
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
    let row: Option<(Option<String>,)> = sqlx::query_as(
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
    )
    .fetch_optional(&self.pool)
    .await?;

    row
      .and_then(|(s,)| s)
      .map(|s| decode_dt(&s))
      .transpose()
  }

  async fn search(&self, query: &FactQuery) -> Result<Vec<Subject>> {
    let text_pattern = query.text.as_deref().map(|t| format!("%{t}%"));
    let kind_str = query.kind.map(|k| k.to_string());
    let limit_val = query.limit.unwrap_or(100) as i64;
    let offset_val = query.offset.unwrap_or(0) as i64;

    // NULL-guard pattern keeps parameters fixed at 6; no dynamic WHERE needed.
    let raws: Vec<RawSubject> = sqlx::query_as(
      "SELECT DISTINCT s.subject_id, s.created_at, s.kind
       FROM subjects s
       LEFT JOIN facts f ON f.subject_id = s.subject_id
       WHERE (? IS NULL OR f.value_json LIKE ?)
         AND (? IS NULL OR s.kind = ?)
       LIMIT ? OFFSET ?",
    )
    .bind(text_pattern.as_deref())
    .bind(text_pattern.as_deref())
    .bind(kind_str.as_deref())
    .bind(kind_str.as_deref())
    .bind(limit_val)
    .bind(offset_val)
    .fetch_all(&self.pool)
    .await?;

    raws.into_iter().map(RawSubject::into_subject).collect()
  }
}
