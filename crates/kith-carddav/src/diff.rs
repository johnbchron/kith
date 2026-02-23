//! vCard diff pipeline: incoming vCard → minimal store operations.
//!
//! Computes the set of new facts, supersessions, and retractions needed to
//! transition the current contact state to match an incoming vCard.

use kith_core::{
  fact::{Confidence, FactValue, NewFact, RecordingContext},
  lifecycle::ContactView,
};
use uuid::Uuid;

/// The result of diffing an incoming vCard against the current store state.
pub struct DiffResult {
  pub new_facts:     Vec<NewFact>,
  pub supersessions: Vec<(Uuid /* old_fact_id */, NewFact)>,
  pub retractions:   Vec<Uuid>,
}

/// Compute the minimal set of store operations that transitions `current_view`
/// to match `incoming_vcard`.
///
/// When `current_view` is `None` (new contact), all parsed facts are new.
pub fn diff(
  incoming_vcard: &str,
  subject_id: Uuid,
  source_name: &str,
  current_view: Option<&ContactView>,
) -> Result<DiffResult, kith_vcard::Error> {
  let parsed = kith_vcard::parse(incoming_vcard, source_name)?;
  let uid = parsed.uid.clone();

  // Build incoming facts with the real subject_id and correct context.
  let incoming: Vec<NewFact> = parsed
    .facts
    .into_iter()
    .map(|mut f| {
      f.subject_id = subject_id;
      f.confidence = Confidence::Certain;
      f.recording_context = RecordingContext::Imported {
        source_name:  source_name.to_string(),
        original_uid: uid.clone(),
      };
      f
    })
    .collect();

  let Some(view) = current_view else {
    // No existing contact — all incoming facts are new.
    return Ok(DiffResult {
      new_facts:     incoming,
      supersessions: vec![],
      retractions:   vec![],
    });
  };

  let active: Vec<&kith_core::lifecycle::ResolvedFact> =
    view.active_facts.iter().collect();

  let mut new_facts: Vec<NewFact> = vec![];
  let mut supersessions: Vec<(Uuid, NewFact)> = vec![];
  // Track which active fact IDs were matched.
  let mut matched_active: std::collections::HashSet<Uuid> =
    std::collections::HashSet::new();

  for incoming_fact in incoming {
    // Try to find a matching active fact by type + key fields.
    let match_result = find_match(&incoming_fact.value, &active);

    match match_result {
      Some((old_id, old_value)) => {
        matched_active.insert(old_id);
        if values_identical(&incoming_fact.value, old_value) {
          // Unchanged — no-op.
        } else {
          // Value changed — supersession.
          supersessions.push((old_id, incoming_fact));
        }
      }
      None => {
        new_facts.push(incoming_fact);
      }
    }
  }

  // Any active fact not matched by incoming → retraction.
  let retractions: Vec<Uuid> = active
    .iter()
    .filter(|rf| !matched_active.contains(&rf.fact.fact_id))
    .map(|rf| rf.fact.fact_id)
    .collect();

  Ok(DiffResult {
    new_facts,
    supersessions,
    retractions,
  })
}

/// Find a matching active fact for the given incoming value.
///
/// Returns `(fact_id, &FactValue)` if a match is found.
fn find_match<'a>(
  incoming: &FactValue,
  active: &[&'a kith_core::lifecycle::ResolvedFact],
) -> Option<(Uuid, &'a FactValue)> {
  for rf in active {
    if fact_matches(incoming, &rf.fact.value) {
      return Some((rf.fact.fact_id, &rf.fact.value));
    }
  }
  None
}

/// Returns true if `incoming` and `existing` are the same logical piece of
/// information (same type + key fields).
fn fact_matches(incoming: &FactValue, existing: &FactValue) -> bool {
  use FactValue::*;
  match (incoming, existing) {
    // Singleton facts (only one per contact).
    (Name(_), Name(_)) => true,
    (Birthday(_), Birthday(_)) => true,
    (Anniversary(_), Anniversary(_)) => true,
    (Gender(_), Gender(_)) => true,

    // Key: address (normalized to lowercase).
    (Email(a), Email(b)) => {
      a.address.to_lowercase() == b.address.to_lowercase()
    }

    // Key: number (stripped of whitespace/dashes).
    (Phone(a), Phone(b)) => {
      normalize_phone(&a.number) == normalize_phone(&b.number)
    }

    // Key: (street, locality, postal_code).
    (Address(a), Address(b)) => {
      normalize_opt(&a.street) == normalize_opt(&b.street)
        && normalize_opt(&a.locality) == normalize_opt(&b.locality)
        && normalize_opt(&a.postal_code) == normalize_opt(&b.postal_code)
    }

    // Key: org_name (case-insensitive).
    (OrgMembership(a), OrgMembership(b)) => {
      a.org_name.to_lowercase() == b.org_name.to_lowercase()
    }

    // Key: alias name.
    (Alias(a), Alias(b)) => a.name == b.name,

    // Key: url.
    (Url(a), Url(b)) => a.url == b.url,

    // Key: (service, handle).
    (Im(a), Im(b)) => {
      a.service.to_lowercase() == b.service.to_lowercase()
        && a.handle == b.handle
    }

    // Key: (platform, handle).
    (Social(a), Social(b)) => {
      a.platform.to_lowercase() == b.platform.to_lowercase()
        && a.handle == b.handle
    }

    // Key: exact content.
    (Note(a), Note(b)) => a == b,

    // Key: group_id if present, else group_name.
    (GroupMembership(a), GroupMembership(b)) => {
      match (&a.group_id, &b.group_id) {
        (Some(ga), Some(gb)) => ga == gb,
        _ => a.group_name.to_lowercase() == b.group_name.to_lowercase(),
      }
    }

    // Key: (relation, other_id).
    (Relationship(a), Relationship(b)) => {
      a.relation == b.relation && a.other_id == b.other_id
    }

    // Key: (summary, effective_at) — Meeting has no effective_at on NewFact;
    // match on summary + location.
    (Meeting(a), Meeting(b)) => a.summary == b.summary,

    // Key: exact content.
    (Introduction(a), Introduction(b)) => a == b,

    // Key: custom key.
    (Custom { key: ka, .. }, Custom { key: kb, .. }) => ka == kb,

    // Photo: match by path.
    (Photo(a), Photo(b)) => a.path == b.path,

    _ => false,
  }
}

/// Returns true if the two values are structurally identical.
fn values_identical(a: &FactValue, b: &FactValue) -> bool {
  // Serialize both to JSON and compare; avoids re-implementing equality.
  let ja = serde_json::to_value(a).unwrap_or_default();
  let jb = serde_json::to_value(b).unwrap_or_default();
  ja == jb
}

fn normalize_phone(s: &str) -> String {
  s.chars()
    .filter(|c| !c.is_whitespace() && *c != '-')
    .collect()
}

fn normalize_opt(s: &Option<String>) -> String {
  s.as_deref().unwrap_or("").to_lowercase()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use chrono::{TimeZone, Utc};
  use kith_core::{
    fact::{Confidence, Fact, FactValue, RecordingContext},
    lifecycle::{ContactView, FactStatus, ResolvedFact},
    subject::{Subject, SubjectKind},
  };

  use super::*;

  const SRC: &str = "test";

  #[test]
  fn none_view_all_new() {
    let vcard = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEMAIL:alice@\
                 example.com\r\nEND:VCARD\r\n";
    let result = diff(vcard, Uuid::new_v4(), SRC, None).unwrap();
    assert!(!result.new_facts.is_empty());
    assert!(result.supersessions.is_empty());
    assert!(result.retractions.is_empty());
  }

  #[test]
  fn unchanged_contact_empty_result() {
    let vcard = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEMAIL:alice@\
                 example.com\r\nEND:VCARD\r\n";
    let id = Uuid::new_v4();
    // First diff to get the initial facts.
    let r1 = diff(vcard, id, SRC, None).unwrap();

    // Build a fake view with those facts.
    let ts = Utc.timestamp_opt(1_000_000, 0).unwrap();
    let active_facts: Vec<ResolvedFact> = r1
      .new_facts
      .into_iter()
      .map(|f| ResolvedFact {
        fact:   Fact {
          fact_id:           Uuid::new_v4(),
          subject_id:        id,
          value:             f.value,
          recorded_at:       ts,
          effective_at:      None,
          effective_until:   None,
          source:            None,
          confidence:        Confidence::Certain,
          recording_context: RecordingContext::Manual,
          tags:              vec![],
        },
        status: FactStatus::Active,
      })
      .collect();
    let view = ContactView {
      subject: Subject {
        subject_id: id,
        created_at: ts,
        kind:       SubjectKind::Person,
      },
      as_of: ts,
      active_facts,
    };

    let r2 = diff(vcard, id, SRC, Some(&view)).unwrap();
    assert!(
      r2.new_facts.is_empty(),
      "unexpected new facts: {:?}",
      r2.new_facts.len()
    );
    assert!(r2.supersessions.is_empty(), "unexpected supersessions");
    assert!(r2.retractions.is_empty(), "unexpected retractions");
  }

  /// Build a view by diffing a vCard against None (first import).
  fn initial_view(vcard: &str, id: Uuid) -> ContactView {
    let ts = Utc.timestamp_opt(1_000_000, 0).unwrap();
    let r = diff(vcard, id, SRC, None).unwrap();
    let active_facts = r
      .new_facts
      .into_iter()
      .map(|f| ResolvedFact {
        fact:   Fact {
          fact_id:           Uuid::new_v4(),
          subject_id:        id,
          value:             f.value,
          recorded_at:       ts,
          effective_at:      None,
          effective_until:   None,
          source:            None,
          confidence:        Confidence::Certain,
          recording_context: RecordingContext::Manual,
          tags:              vec![],
        },
        status: FactStatus::Active,
      })
      .collect();
    ContactView {
      subject: Subject {
        subject_id: id,
        created_at: ts,
        kind:       SubjectKind::Person,
      },
      as_of: ts,
      active_facts,
    }
  }

  #[test]
  fn email_change_is_supersession() {
    let id = Uuid::new_v4();
    // Establish initial state: FN + Email(WORK)
    let initial = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEMAIL;TYPE=WORK:\
                   alice@example.com\r\nEND:VCARD\r\n";
    let view = initial_view(initial, id);

    // Update: same address, different label (HOME).
    let updated = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEMAIL;TYPE=HOME:\
                   alice@example.com\r\nEND:VCARD\r\n";
    let result = diff(updated, id, SRC, Some(&view)).unwrap();

    // Email address is the same key → match; label differs → supersession.
    assert_eq!(result.supersessions.len(), 1, "expected one supersession");
    assert!(
      result.new_facts.is_empty(),
      "unexpected new_facts: {}",
      result.new_facts.len()
    );
    assert!(
      result.retractions.is_empty(),
      "unexpected retractions: {}",
      result.retractions.len()
    );
  }

  #[test]
  fn new_phone_is_new_fact() {
    let id = Uuid::new_v4();
    // Establish initial state: FN + Email
    let initial = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEMAIL;TYPE=WORK:\
                   alice@example.com\r\nEND:VCARD\r\n";
    let view = initial_view(initial, id);

    // Add a phone; email and name unchanged.
    let updated = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEMAIL;TYPE=WORK:\
                   alice@example.com\r\nTEL;TYPE=CELL:+15555551234\r\nEND:\
                   VCARD\r\n";
    let result = diff(updated, id, SRC, Some(&view)).unwrap();

    let phones: Vec<_> = result
      .new_facts
      .iter()
      .filter(|f| matches!(f.value, FactValue::Phone(_)))
      .collect();
    assert_eq!(phones.len(), 1, "expected one new phone");
    assert!(
      result.supersessions.is_empty(),
      "unexpected supersessions: {}",
      result.supersessions.len()
    );
    assert!(
      result.retractions.is_empty(),
      "unexpected retractions: {}",
      result.retractions.len()
    );
  }

  #[test]
  fn email_removed_is_retraction() {
    let id = Uuid::new_v4();
    // Establish initial state: FN + Email
    let initial = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEMAIL;TYPE=WORK:\
                   alice@example.com\r\nEND:VCARD\r\n";
    let view = initial_view(initial, id);

    // Remove the email, keep the name.
    let updated = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEND:VCARD\r\n";
    let result = diff(updated, id, SRC, Some(&view)).unwrap();

    assert_eq!(result.retractions.len(), 1, "expected one retraction");
    assert!(
      result.new_facts.is_empty(),
      "unexpected new_facts: {}",
      result.new_facts.len()
    );
    assert!(
      result.supersessions.is_empty(),
      "unexpected supersessions: {}",
      result.supersessions.len()
    );
  }

  #[test]
  fn full_contact_round_trip() {
    let id = Uuid::new_v4();
    let vcard = concat!(
      "BEGIN:VCARD\r\n",
      "VERSION:4.0\r\n",
      "FN:Alice Smith\r\n",
      "N:Smith;Alice;;;\r\n",
      "EMAIL;TYPE=WORK:alice@example.com\r\n",
      "TEL;TYPE=CELL:+15555551234\r\n",
      "ORG:Acme Corp\r\n",
      "NOTE:First met at conference.\r\n",
      "END:VCARD\r\n",
    );

    let r1 = diff(vcard, id, SRC, None).unwrap();
    assert!(!r1.new_facts.is_empty());

    // Build view from those facts.
    let ts = Utc.timestamp_opt(1_000_000, 0).unwrap();
    let active_facts: Vec<ResolvedFact> = r1
      .new_facts
      .into_iter()
      .map(|f| ResolvedFact {
        fact:   Fact {
          fact_id:           Uuid::new_v4(),
          subject_id:        id,
          value:             f.value,
          recorded_at:       ts,
          effective_at:      None,
          effective_until:   None,
          source:            None,
          confidence:        Confidence::Certain,
          recording_context: RecordingContext::Manual,
          tags:              vec![],
        },
        status: FactStatus::Active,
      })
      .collect();
    let view = ContactView {
      subject: Subject {
        subject_id: id,
        created_at: ts,
        kind:       SubjectKind::Person,
      },
      as_of: ts,
      active_facts,
    };

    // Diff again — should be empty.
    let r2 = diff(vcard, id, SRC, Some(&view)).unwrap();
    assert!(r2.new_facts.is_empty(), "new={}", r2.new_facts.len());
    assert!(
      r2.supersessions.is_empty(),
      "sup={}",
      r2.supersessions.len()
    );
    assert!(r2.retractions.is_empty(), "ret={}", r2.retractions.len());
  }
}
