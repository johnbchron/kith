//! The `ContactStore` trait and supporting query types.
//!
//! The trait is implemented by storage backends (e.g. `kith-store-sqlite`).
//! Higher layers (`kith-carddav`, `kith-cli`) depend on this abstraction, not
//! on any concrete backend.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
  fact::{Confidence, NewFact},
  lifecycle::{ContactView, Retraction, ResolvedFact, Supersession},
  subject::{Subject, SubjectKind},
};

// ─── Query type ──────────────────────────────────────────────────────────────

/// Parameters for [`ContactStore::search`].
#[derive(Debug, Clone, Default)]
pub struct FactQuery {
  /// Free-text filter applied over serialised fact values.
  pub text:           Option<String>,
  /// Restrict to subjects of a specific kind.
  pub kind:           Option<SubjectKind>,
  /// Restrict to specific fact type discriminants (e.g. `["email", "phone"]`).
  pub fact_types:     Vec<String>,
  /// All returned subjects must have facts with all of these tags.
  pub tags:           Vec<String>,
  pub confidence:     Option<Confidence>,
  pub recorded_after: Option<DateTime<Utc>>,
  pub recorded_before: Option<DateTime<Utc>>,
  pub limit:          Option<usize>,
  pub offset:         Option<usize>,
}

// ─── Trait ───────────────────────────────────────────────────────────────────

/// Abstraction over a Kith contact store backend.
///
/// All write operations on facts are append-only. Mutations are expressed as
/// lifecycle events (supersession, retraction), which are themselves
/// append-only.
pub trait ContactStore: Send + Sync {
  type Error: std::error::Error + Send + Sync + 'static;

  // ── Subjects ──────────────────────────────────────────────────────────

  /// Create and persist a new subject with the given kind.
  async fn add_subject(&self, kind: SubjectKind) -> Result<Subject, Self::Error>;

  /// Retrieve a subject by UUID. Returns `None` if not found.
  async fn get_subject(&self, id: Uuid) -> Result<Option<Subject>, Self::Error>;

  /// List all subjects, optionally filtered by kind.
  async fn list_subjects(
    &self,
    kind: Option<SubjectKind>,
  ) -> Result<Vec<Subject>, Self::Error>;

  // ── Facts — append-only writes ────────────────────────────────────────

  /// Record a new fact and return the persisted [`Fact`](crate::fact::Fact).
  /// The `recorded_at` timestamp is set by the store.
  async fn record_fact(&self, input: NewFact) -> Result<crate::fact::Fact, Self::Error>;

  // ── Lifecycle events ──────────────────────────────────────────────────

  /// Record that an existing fact is superseded by a new (replacement) fact.
  ///
  /// Returns an error if `old_id` is already superseded or retracted, or if
  /// `old_id == replacement.fact_id` (self-supersession).
  async fn supersede(
    &self,
    old_id:      Uuid,
    replacement: NewFact,
  ) -> Result<(Supersession, crate::fact::Fact), Self::Error>;

  /// Retract a fact entirely (no replacement).
  ///
  /// Returns an error if the fact is already superseded or retracted.
  async fn retract(
    &self,
    fact_id: Uuid,
    reason:  Option<String>,
  ) -> Result<Retraction, Self::Error>;

  // ── Reads ─────────────────────────────────────────────────────────────

  /// Return all facts for a subject, with their lifecycle status resolved.
  ///
  /// - `as_of`: point-in-time filter on `recorded_at`; defaults to now.
  /// - `include_inactive`: if `false`, only `Active` facts are returned.
  async fn get_facts(
    &self,
    subject_id:       Uuid,
    as_of:            Option<DateTime<Utc>>,
    include_inactive: bool,
  ) -> Result<Vec<ResolvedFact>, Self::Error>;

  /// Materialise a [`ContactView`] — the computed, current-state read model
  /// for a subject. Returns `None` if the subject does not exist.
  async fn materialize(
    &self,
    subject_id: Uuid,
    as_of:      Option<DateTime<Utc>>,
  ) -> Result<Option<ContactView>, Self::Error>;

  /// Search for subjects matching `query`. Phase-1 implementation uses SQL
  /// LIKE; phase 2 will use FTS5.
  async fn search(&self, query: &FactQuery) -> Result<Vec<Subject>, Self::Error>;
}
