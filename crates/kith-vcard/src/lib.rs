//! vCard 3.0 / 4.0 codec for Kith.
//!
//! Converts between vCard strings and [`kith_core`] domain types. Pure
//! synchronous; no HTTP or database dependencies.
//!
//! # Quick start
//!
//! ```no_run
//! use kith_vcard::{ParsedVcard, parse};
//!
//! let vcard = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice Smith\r\nEND:VCARD\r\n";
//! let parsed: ParsedVcard = parse(vcard, "my-import").unwrap();
//! println!("uid={:?}, {} facts", parsed.uid, parsed.facts.len());
//! ```

pub mod error;
mod parse;
mod serialize;

pub use error::{Error, Result};
use kith_core::{fact::NewFact, lifecycle::ContactView};

// ─── Public types
// ─────────────────────────────────────────────────────────────

/// The result of parsing a single vCard.
///
/// All `facts[*].subject_id` are [`uuid::Uuid::nil()`]; the caller must
/// replace them with the real subject UUID before persisting.
pub struct ParsedVcard {
  /// The `UID` property from the vCard, if present.
  pub uid:   Option<String>,
  /// Facts decoded from the vCard properties.
  /// All use `RecordingContext::Imported { source_name, original_uid: uid }`.
  pub facts: Vec<NewFact>,
}

// ─── Public API
// ───────────────────────────────────────────────────────────────

/// Parse a single vCard from `input`.
///
/// `source_name` is stored in every fact's
/// `RecordingContext::Imported { source_name, … }`.
pub fn parse(input: &str, source_name: &str) -> Result<ParsedVcard> {
  parse::parse_one(input, source_name)
}

/// Parse zero or more vCards from `input`.
///
/// Each `BEGIN:VCARD … END:VCARD` block is parsed independently; a malformed
/// block yields `Err(…)` in the corresponding position without aborting the
/// rest.
pub fn parse_many(input: &str, source_name: &str) -> Vec<Result<ParsedVcard>> {
  let lines = parse::unfold_lines(input);
  let mut results = Vec::new();
  let mut i = 0;

  while i < lines.len() {
    if lines[i].eq_ignore_ascii_case("BEGIN:VCARD") {
      let start = i;
      let rel_end = lines[start + 1..]
        .iter()
        .position(|l| l.eq_ignore_ascii_case("END:VCARD"));

      if let Some(offset) = rel_end {
        let end = start + 1 + offset;
        let card_str = lines[start..=end].join("\r\n") + "\r\n";
        results.push(parse::parse_one(&card_str, source_name));
        i = end + 1;
      } else {
        results.push(Err(Error::MissingEnvelope));
        break;
      }
    } else {
      i += 1;
    }
  }

  results
}

/// Serialize `view` as a vCard 4.0 string (CRLF line endings, folded at 75
/// octets).
pub fn serialize(view: &ContactView) -> Result<String> {
  serialize::serialize(view)
}

/// Serialize `view` as a vCard 3.0 string.
pub fn serialize_v3(view: &ContactView) -> Result<String> {
  serialize::serialize_v3(view)
}

// ─── Round-trip test ─────────────────────────────────────────────────────────

#[cfg(test)]
mod roundtrip_tests {
  use kith_core::fact::{
    AddressValue, ContactLabel, EmailValue, FactValue, NameValue,
    OrgMembershipValue, PhoneKind, PhoneValue, RelationshipValue, SocialValue,
  };
  use uuid::Uuid;

  use super::{test_helpers::make_view, *};

  fn find_fact<F>(parsed: &ParsedVcard, predicate: F) -> Option<&FactValue>
  where
    F: Fn(&FactValue) -> bool,
  {
    parsed
      .facts
      .iter()
      .find(|f| predicate(&f.value))
      .map(|f| &f.value)
  }

  #[test]
  fn full_round_trip() {
    let other_id = Uuid::new_v4();

    let input_facts = vec![
      FactValue::Name(NameValue {
        given:      Some("Alice".to_string()),
        family:     Some("Smith".to_string()),
        additional: None,
        prefix:     None,
        suffix:     None,
        full:       "Alice Smith".to_string(),
      }),
      FactValue::Email(EmailValue {
        address:    "alice@example.com".to_string(),
        label:      ContactLabel::Work,
        preference: 1,
      }),
      FactValue::Phone(PhoneValue {
        number:     "+15555551234".to_string(),
        label:      ContactLabel::Home,
        kind:       PhoneKind::Cell,
        preference: 2,
      }),
      FactValue::Address(AddressValue {
        label:       ContactLabel::Work,
        street:      Some("123 Main St".to_string()),
        locality:    Some("Springfield".to_string()),
        region:      Some("IL".to_string()),
        postal_code: Some("62701".to_string()),
        country:     Some("USA".to_string()),
      }),
      FactValue::OrgMembership(OrgMembershipValue {
        org_name: "Acme Corp".to_string(),
        org_id:   None,
        title:    Some("Engineer".to_string()),
        role:     Some("IC".to_string()),
      }),
      FactValue::Note("First met at conference.".to_string()),
      FactValue::Social(SocialValue {
        handle:   "@alice".to_string(),
        platform: "Twitter".to_string(),
      }),
      FactValue::Relationship(RelationshipValue {
        relation:   "colleague".to_string(),
        other_id:   Some(other_id),
        other_name: Some("Bob".to_string()),
      }),
    ];

    let view = make_view(input_facts);
    let vcard = serialize(&view).expect("serialization failed");
    let parsed = parse(&vcard, "roundtrip").expect("parse failed");

    // Name
    let FactValue::Name(n) =
      find_fact(&parsed, |f| matches!(f, FactValue::Name(_))).unwrap()
    else {
      panic!("no Name fact")
    };
    assert_eq!(n.full, "Alice Smith");
    assert_eq!(n.family, Some("Smith".to_string()));
    assert_eq!(n.given, Some("Alice".to_string()));

    // Email
    let FactValue::Email(e) =
      find_fact(&parsed, |f| matches!(f, FactValue::Email(_))).unwrap()
    else {
      panic!("no Email fact")
    };
    assert_eq!(e.address, "alice@example.com");
    assert_eq!(e.preference, 1);

    // Phone
    let FactValue::Phone(p) =
      find_fact(&parsed, |f| matches!(f, FactValue::Phone(_))).unwrap()
    else {
      panic!("no Phone fact")
    };
    assert_eq!(p.number, "+15555551234");
    assert_eq!(p.kind, PhoneKind::Cell);

    // Address
    let FactValue::Address(a) =
      find_fact(&parsed, |f| matches!(f, FactValue::Address(_))).unwrap()
    else {
      panic!("no Address fact")
    };
    assert_eq!(a.street, Some("123 Main St".to_string()));
    assert_eq!(a.locality, Some("Springfield".to_string()));

    // OrgMembership
    let FactValue::OrgMembership(o) =
      find_fact(&parsed, |f| matches!(f, FactValue::OrgMembership(_))).unwrap()
    else {
      panic!("no OrgMembership fact")
    };
    assert_eq!(o.org_name, "Acme Corp");
    assert_eq!(o.title, Some("Engineer".to_string()));
    assert_eq!(o.role, Some("IC".to_string()));

    // Note
    let FactValue::Note(note) =
      find_fact(&parsed, |f| matches!(f, FactValue::Note(_))).unwrap()
    else {
      panic!("no Note fact")
    };
    assert_eq!(note, "First met at conference.");

    // Social
    let FactValue::Social(s) =
      find_fact(&parsed, |f| matches!(f, FactValue::Social(_))).unwrap()
    else {
      panic!("no Social fact")
    };
    assert_eq!(s.platform, "Twitter");
    assert_eq!(s.handle, "@alice");

    // Relationship
    let FactValue::Relationship(r) =
      find_fact(&parsed, |f| matches!(f, FactValue::Relationship(_))).unwrap()
    else {
      panic!("no Relationship fact")
    };
    assert_eq!(r.relation, "colleague");
    assert_eq!(r.other_id, Some(other_id));
  }
}

// ─── Shared test helpers ──────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_helpers {
  use chrono::{TimeZone, Utc};
  use kith_core::{
    fact::{Confidence, Fact, FactValue, RecordingContext},
    lifecycle::{ContactView, FactStatus, ResolvedFact},
    subject::{Subject, SubjectKind},
  };
  use uuid::Uuid;

  /// Build a [`ContactView`] from a list of fact values for use in tests.
  pub(crate) fn make_view(facts: Vec<FactValue>) -> ContactView {
    let subject_id = Uuid::new_v4();
    let as_of = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
    let subject = Subject {
      subject_id,
      created_at: as_of,
      kind: SubjectKind::Person,
    };
    let active_facts = facts
      .into_iter()
      .map(|v| {
        let fact = Fact {
          fact_id: Uuid::new_v4(),
          subject_id,
          value: v,
          recorded_at: as_of,
          effective_at: None,
          effective_until: None,
          source: None,
          confidence: Confidence::Certain,
          recording_context: RecordingContext::Manual,
          tags: vec![],
        };
        ResolvedFact {
          fact,
          status: FactStatus::Active,
        }
      })
      .collect();
    ContactView {
      subject,
      as_of,
      active_facts,
    }
  }
}
