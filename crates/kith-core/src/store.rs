//! The `ContactStore` trait and supporting query types.
//!
//! The trait is implemented by storage backends (e.g. `kith-store-sqlite`).
//! Higher layers (`kith-carddav`, `kith-cli`) depend on this abstraction, not
//! on any concrete backend.

use std::future::Future;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
  fact::{Confidence, NewFact},
  lifecycle::{ContactView, ResolvedFact, Retraction, Supersession},
  subject::{Subject, SubjectKind},
};

// ─── Query type ──────────────────────────────────────────────────────────────

/// Parameters for [`ContactStore::search`].
#[derive(Debug, Clone, Default)]
pub struct FactQuery {
  /// Free-text filter applied over serialised fact values.
  pub text:            Option<String>,
  /// Restrict to subjects of a specific kind.
  pub kind:            Option<SubjectKind>,
  /// Restrict to specific fact type discriminants (e.g. `["email", "phone"]`).
  pub fact_types:      Vec<String>,
  /// All returned subjects must have facts with all of these tags.
  pub tags:            Vec<String>,
  pub confidence:      Option<Confidence>,
  pub recorded_after:  Option<DateTime<Utc>>,
  pub recorded_before: Option<DateTime<Utc>>,
  pub limit:           Option<usize>,
  pub offset:          Option<usize>,
}

// ─── Trait ───────────────────────────────────────────────────────────────────

/// Abstraction over a Kith contact store backend.
///
/// All write operations on facts are append-only. Mutations are expressed as
/// lifecycle events (supersession, retraction), which are themselves
/// append-only.
///
/// All methods return `Send` futures so the trait can be used in multi-threaded
/// async runtimes (e.g. tokio with `axum`).
pub trait ContactStore: Send + Sync {
  type Error: std::error::Error + Send + Sync + 'static;

  // ── Subjects ──────────────────────────────────────────────────────────

  /// Create and persist a new subject with the given kind.
  fn add_subject(
    &self,
    kind: SubjectKind,
  ) -> impl Future<Output = Result<Subject, Self::Error>> + Send + '_;

  /// Create and persist a subject with a caller-supplied UUID.
  ///
  /// Used by the CardDAV PUT handler to ensure the subject UUID matches the
  /// URL path. Returns an error if the UUID is already taken.
  fn add_subject_with_id(
    &self,
    id: Uuid,
    kind: SubjectKind,
  ) -> impl Future<Output = Result<Subject, Self::Error>> + Send + '_;

  /// Retrieve a subject by UUID. Returns `None` if not found.
  fn get_subject(
    &self,
    id: Uuid,
  ) -> impl Future<Output = Result<Option<Subject>, Self::Error>> + Send + '_;

  /// List all subjects, optionally filtered by kind.
  fn list_subjects(
    &self,
    kind: Option<SubjectKind>,
  ) -> impl Future<Output = Result<Vec<Subject>, Self::Error>> + Send + '_;

  // ── Facts — append-only writes ────────────────────────────────────────

  /// Record a new fact and return the persisted [`Fact`](crate::fact::Fact).
  /// The `recorded_at` timestamp is set by the store.
  fn record_fact(
    &self,
    input: NewFact,
  ) -> impl Future<Output = Result<crate::fact::Fact, Self::Error>> + Send + '_;

  // ── Lifecycle events ──────────────────────────────────────────────────

  /// Record that an existing fact is superseded by a new (replacement) fact.
  ///
  /// Returns an error if `old_id` is already superseded or retracted, or if
  /// `old_id == replacement.fact_id` (self-supersession).
  fn supersede(
    &self,
    old_id: Uuid,
    replacement: NewFact,
  ) -> impl Future<Output = Result<(Supersession, crate::fact::Fact), Self::Error>>
  + Send
  + '_;

  /// Retract a fact entirely (no replacement).
  ///
  /// Returns an error if the fact is already superseded or retracted.
  fn retract(
    &self,
    fact_id: Uuid,
    reason: Option<String>,
  ) -> impl Future<Output = Result<Retraction, Self::Error>> + Send + '_;

  // ── Reads ─────────────────────────────────────────────────────────────

  /// Return all facts for a subject, with their lifecycle status resolved.
  ///
  /// - `as_of`: point-in-time filter on `recorded_at`; defaults to now.
  /// - `include_inactive`: if `false`, only `Active` facts are returned.
  fn get_facts(
    &self,
    subject_id: Uuid,
    as_of: Option<DateTime<Utc>>,
    include_inactive: bool,
  ) -> impl Future<Output = Result<Vec<ResolvedFact>, Self::Error>> + Send + '_;

  /// Materialise a [`ContactView`] — the computed, current-state read model
  /// for a subject. Returns `None` if the subject does not exist.
  fn materialize(
    &self,
    subject_id: Uuid,
    as_of: Option<DateTime<Utc>>,
  ) -> impl Future<Output = Result<Option<ContactView>, Self::Error>> + Send + '_;

  /// Search for subjects matching `query`. Phase-1 implementation uses SQL
  /// LIKE; phase 2 will use FTS5.
  fn search<'a>(
    &'a self,
    query: &'a FactQuery,
  ) -> impl Future<Output = Result<Vec<Subject>, Self::Error>> + Send + 'a;
}
