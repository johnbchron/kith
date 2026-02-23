//! ETag computation for ContactView resources.
//!
//! ETags are SHA-256 hashes over the sorted (fact_id, recorded_at) pairs of
//! all active facts. Ordering is deterministic regardless of insertion order.

use chrono::{DateTime, Utc};
use kith_core::lifecycle::ContactView;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Compute an ETag for the given `ContactView`.
///
/// Stable: same active facts in any order → same ETag.
pub fn compute_etag(view: &ContactView) -> String {
  let mut pairs: Vec<(Uuid, DateTime<Utc>)> = view
    .active_facts
    .iter()
    .map(|rf| (rf.fact.fact_id, rf.fact.recorded_at))
    .collect();
  compute_etag_from_pairs(&mut pairs)
}

/// Compute an ETag directly from (fact_id, recorded_at) pairs.
///
/// The slice is sorted in-place for determinism.
pub fn compute_etag_from_pairs(pairs: &mut [(Uuid, DateTime<Utc>)]) -> String {
  pairs.sort_by_key(|(id, _)| *id);

  let mut hasher = Sha256::new();
  for (id, ts) in pairs.iter() {
    hasher.update(id.as_bytes());
    hasher.update(ts.timestamp_micros().to_le_bytes());
  }
  let hash = hasher.finalize();
  format!("\"{}\"", hex::encode(hash))
}

#[cfg(test)]
mod tests {
  use chrono::{TimeZone, Utc};
  use kith_core::{
    fact::{Confidence, Fact, FactValue, NameValue, RecordingContext},
    lifecycle::{ContactView, FactStatus, ResolvedFact},
    subject::{Subject, SubjectKind},
  };

  use super::*;

  fn make_fact(id: Uuid, ts_secs: i64) -> ResolvedFact {
    let subject_id = Uuid::nil();
    let ts = Utc.timestamp_opt(ts_secs, 0).unwrap();
    ResolvedFact {
      fact:   Fact {
        fact_id: id,
        subject_id,
        value: FactValue::Name(NameValue {
          given:      Some("Test".into()),
          family:     None,
          additional: None,
          prefix:     None,
          suffix:     None,
          full:       "Test".into(),
        }),
        recorded_at: ts,
        effective_at: None,
        effective_until: None,
        source: None,
        confidence: Confidence::Certain,
        recording_context: RecordingContext::Manual,
        tags: vec![],
      },
      status: FactStatus::Active,
    }
  }

  fn make_view(facts: Vec<ResolvedFact>) -> ContactView {
    let ts = Utc.timestamp_opt(0, 0).unwrap();
    ContactView {
      subject:      Subject {
        subject_id: Uuid::nil(),
        created_at: ts,
        kind:       SubjectKind::Person,
      },
      as_of:        ts,
      active_facts: facts,
    }
  }

  #[test]
  fn insertion_order_does_not_matter() {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();
    let ts_a = 1000;
    let ts_b = 2000;

    let view1 = make_view(vec![make_fact(id_a, ts_a), make_fact(id_b, ts_b)]);
    let view2 = make_view(vec![make_fact(id_b, ts_b), make_fact(id_a, ts_a)]);

    assert_eq!(compute_etag(&view1), compute_etag(&view2));
  }

  #[test]
  fn adding_a_fact_changes_etag() {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    let view1 = make_view(vec![make_fact(id_a, 1000)]);
    let view2 = make_view(vec![make_fact(id_a, 1000), make_fact(id_b, 2000)]);

    assert_ne!(compute_etag(&view1), compute_etag(&view2));
  }
}
