//! Lifecycle events and resolved fact types.
//!
//! Facts are immutable. Their lifecycle (supersession and retraction) is
//! tracked in two separate append-only tables. A fact's current status is
//! computed at query time by joining against those tables.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{fact::Fact, subject::Subject};

// ─── Lifecycle event records ─────────────────────────────────────────────────

/// Records that an old fact has been replaced by a newer, corrected version.
/// A fact can be superseded at most once (enforced by a UNIQUE constraint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Supersession {
  pub supersession_id: Uuid,
  pub old_fact_id:     Uuid,
  pub new_fact_id:     Uuid,
  pub recorded_at:     DateTime<Utc>,
}

/// Records that a fact has been withdrawn entirely, with no replacement.
/// A fact can be retracted at most once (enforced by a UNIQUE constraint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retraction {
  pub retraction_id: Uuid,
  pub fact_id:       Uuid,
  pub reason:        Option<String>,
  pub recorded_at:   DateTime<Utc>,
}

// ─── Computed status ─────────────────────────────────────────────────────────

/// The lifecycle status of a fact, computed at query time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FactStatus {
  Active,
  Superseded {
    /// The UUID of the fact that replaced this one.
    by: Uuid,
    at: DateTime<Utc>,
  },
  Retracted {
    reason: Option<String>,
    at:     DateTime<Utc>,
  },
}

impl FactStatus {
  pub fn is_active(&self) -> bool { matches!(self, Self::Active) }
}

/// A fact bundled with its current lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedFact {
  pub fact:   Fact,
  pub status: FactStatus,
}

// ─── Materialised view ───────────────────────────────────────────────────────

/// The computed read model for a subject — never stored, always derived.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactView {
  pub subject:      Subject,
  /// The point in time at which this view was materialised.
  pub as_of:        DateTime<Utc>,
  /// All facts with [`FactStatus::Active`] status as of `as_of`.
  pub active_facts: Vec<ResolvedFact>,
}
