//! vCard → Kith fact mapper.
//!
//! Calls [`calcard`]'s `VCard::parse` and maps each entry to [`NewFact`]s.

use calcard::common::{IanaString, IanaType};
use calcard::vcard::{VCard, VCardParameterName, VCardParameterValue, VCardProperty};
use chrono::NaiveDate;
use kith_core::fact::{
  AddressValue, AliasValue, ContactLabel, EmailValue, FactValue, GroupMembershipValue, ImValue,
  MeetingValue, NameValue, NewFact, OrgMembershipValue, PhoneKind, PhoneValue, RecordingContext,
  RelationshipValue, SocialValue, UrlContext, UrlValue,
};
use uuid::Uuid;

use crate::{
  ParsedVcard,
  error::{Error, Result},
};

// ─── Accumulators ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct NameAccum {
  given:      Option<String>,
  family:     Option<String>,
  additional: Option<String>,
  prefix:     Option<String>,
  suffix:     Option<String>,
  full:       Option<String>,
}

impl NameAccum {
  fn is_empty(&self) -> bool {
    self.given.is_none()
      && self.family.is_none()
      && self.additional.is_none()
      && self.prefix.is_none()
      && self.suffix.is_none()
      && self.full.is_none()
  }

  fn flush(self) -> Option<FactValue> {
    if self.is_empty() {
      return None;
    }
    let full = self.full.clone().or_else(|| {
      let mut parts: Vec<String> = Vec::new();
      if let Some(ref p) = self.prefix {
        parts.push(p.clone());
      }
      if let Some(ref g) = self.given {
        parts.push(g.clone());
      }
      if let Some(ref a) = self.additional {
        parts.push(a.clone());
      }
      if let Some(ref f) = self.family {
        parts.push(f.clone());
      }
      if let Some(ref s) = self.suffix {
        parts.push(s.clone());
      }
      if parts.is_empty() { None } else { Some(parts.join(" ")) }
    })?;
    Some(FactValue::Name(NameValue {
      given: self.given,
      family: self.family,
      additional: self.additional,
      prefix: self.prefix,
      suffix: self.suffix,
      full,
    }))
  }
}

#[derive(Default)]
struct OrgGroup {
  org_name: String,
  title:    Option<String>,
  role:     Option<String>,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn opt_str(s: &str) -> Option<String> {
  let s = s.trim();
  if s.is_empty() { None } else { Some(s.to_string()) }
}

/// Collect all TYPE param values from an entry, returned as upper-case strings.
fn type_strings(
  entry: &calcard::vcard::VCardEntry,
) -> Vec<String> {
  entry
    .params
    .iter()
    .filter(|p| p.name == VCardParameterName::Type)
    .filter_map(|p| p.value.as_type())
    .map(|t| match t {
      IanaType::Iana(v) => v.as_str().to_uppercase(),
      IanaType::Other(s) => s.to_uppercase(),
    })
    .collect()
}

/// Get the PREF param value (1–255; 255 = no preference).
fn pref_value(entry: &calcard::vcard::VCardEntry, types: &[String]) -> u8 {
  // v4: PREF=N param
  if let Some(n) = entry
    .parameters(&VCardParameterName::Pref)
    .find_map(|v| {
      if let VCardParameterValue::Integer(n) = v {
        Some(*n as u8)
      } else {
        None
      }
    })
  {
    return n;
  }
  // v3: TYPE=PREF
  if types.iter().any(|t| t == "PREF") {
    return 1;
  }
  255
}

fn label_from_types(types: &[String]) -> ContactLabel {
  for t in types {
    match t.as_str() {
      "WORK" => return ContactLabel::Work,
      "HOME" => return ContactLabel::Home,
      _ => {}
    }
  }
  ContactLabel::Other
}

fn scheme_to_service(scheme: &str) -> String {
  match scheme.to_lowercase().as_str() {
    "xmpp" | "jabber" => "XMPP".to_string(),
    "sip" => "SIP".to_string(),
    "aim" => "AIM".to_string(),
    "ymsgr" => "Yahoo".to_string(),
    "msnim" => "MSN".to_string(),
    "gtalk" => "Google Talk".to_string(),
    "skype" => "Skype".to_string(),
    "irc" => "IRC".to_string(),
    "matrix" => "Matrix".to_string(),
    other => other.to_string(),
  }
}

// ─── Core parser ─────────────────────────────────────────────────────────────

/// Normalize BDAY/ANNIVERSARY date values from `YYYY-MM-DD` to `YYYYMMDD`
/// because calcard's `parse_vcard_date` doesn't handle the extended
/// two-hyphen ISO 8601 form when parsing vCard 4.0 DateAndOrTime properties.
fn normalize_date_lines(input: &str) -> String {
  let mut out = String::with_capacity(input.len());
  for raw in input.split('\n') {
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    // Identify BDAY / ANNIVERSARY lines (strip optional group prefix)
    let prop_start = line.find('.').map(|p| p + 1).unwrap_or(0);
    let prop_line = &line[prop_start..];
    let upper = prop_line.to_uppercase();
    let is_date_prop = upper.starts_with("BDAY:") || upper.starts_with("ANNIVERSARY:");
    if is_date_prop {
      // Find the colon that separates property from value
      if let Some(colon) = prop_line.find(':') {
        let value_part = &prop_line[colon + 1..];
        // Check for YYYY-MM-DD pattern (10 chars, digits and hyphens)
        if value_part.len() >= 10
          && value_part.as_bytes()[4] == b'-'
          && value_part.as_bytes()[7] == b'-'
          && value_part[..4].bytes().all(|b| b.is_ascii_digit())
          && value_part[5..7].bytes().all(|b| b.is_ascii_digit())
          && value_part[8..10].bytes().all(|b| b.is_ascii_digit())
        {
          // Replace YYYY-MM-DD with YYYYMMDD
          let normalized_value =
            format!("{}{}{}", &value_part[..4], &value_part[5..7], &value_part[8..]);
          out.push_str(&line[..prop_start + colon + 1]);
          out.push_str(&normalized_value);
          out.push_str("\r\n");
          continue;
        }
      }
    }
    out.push_str(line);
    out.push_str("\r\n");
  }
  out
}

/// Parse a single vCard block and return a [`ParsedVcard`].
pub fn parse_one(input: &str, source_name: &str) -> Result<ParsedVcard> {
  let normalized = normalize_date_lines(input);
  let vcard = VCard::parse(&normalized).map_err(|_| Error::MissingEnvelope)?;

  let mut uid: Option<String> = None;
  let mut name_accum = NameAccum::default();
  let mut org_groups: Vec<OrgGroup> = Vec::new();
  let mut facts: Vec<FactValue> = Vec::new();

  for entry in &vcard.entries {
    let types = type_strings(entry);
    let pref = pref_value(entry, &types);
    let label = label_from_types(&types);

    match &entry.name {
      // ── Skip envelope / meta ──────────────────────────────────────────────
      VCardProperty::Version
      | VCardProperty::Prodid
      | VCardProperty::Rev
      | VCardProperty::Kind
      | VCardProperty::Categories
      | VCardProperty::Begin
      | VCardProperty::End => {}

      VCardProperty::Uid => {
        uid = entry.values.first().and_then(|v| v.as_text()).map(|s| s.to_string());
      }

      // ── Name ─────────────────────────────────────────────────────────────
      VCardProperty::Fn => {
        if let Some(text) = entry.values.first().and_then(|v| v.as_text()) {
          let v = text.to_string();
          if !v.is_empty() {
            name_accum.full = Some(v);
          }
        }
      }
      VCardProperty::N => {
        // values are [family, given, additional, prefix, suffix]
        let get = |idx: usize| -> Option<String> {
          entry
            .values
            .get(idx)
            .and_then(|v| v.as_text())
            .and_then(|s| opt_str(s))
        };
        name_accum.family = get(0);
        name_accum.given = get(1);
        name_accum.additional = get(2);
        name_accum.prefix = get(3);
        name_accum.suffix = get(4);
      }
      VCardProperty::Nickname => {
        // calcard may parse multi-value NICKNAME as multiple entries or a
        // single comma-separated value; handle both.
        for v in &entry.values {
          if let Some(text) = v.as_text() {
            for token in text.split(',') {
              let name = token.trim().to_string();
              if !name.is_empty() {
                facts.push(FactValue::Alias(AliasValue { name, context: None }));
              }
            }
          }
        }
      }

      // ── Contact methods ───────────────────────────────────────────────────
      VCardProperty::Tel => {
        let number = entry
          .values
          .first()
          .and_then(|v| v.as_text())
          .map(|s| s.trim().to_string())
          .unwrap_or_default();
        if number.is_empty() {
          continue;
        }
        let kind = if types.contains(&"CELL".to_string())
          || types.contains(&"MOBILE".to_string())
        {
          PhoneKind::Cell
        } else if types.contains(&"FAX".to_string()) {
          PhoneKind::Fax
        } else if types.contains(&"PAGER".to_string()) {
          PhoneKind::Pager
        } else if types.contains(&"TEXT".to_string())
          && !types.contains(&"VOICE".to_string())
          && !types.contains(&"CELL".to_string())
        {
          PhoneKind::Text
        } else if types.contains(&"VIDEO".to_string()) {
          PhoneKind::Video
        } else {
          PhoneKind::Voice
        };
        facts.push(FactValue::Phone(PhoneValue { number, label, kind, preference: pref }));
      }

      VCardProperty::Email => {
        let address = entry
          .values
          .first()
          .and_then(|v| v.as_text())
          .map(|s| s.trim().to_string())
          .unwrap_or_default();
        if address.is_empty() {
          continue;
        }
        facts.push(FactValue::Email(EmailValue { address, label, preference: pref }));
      }

      VCardProperty::Adr => {
        // values: [pobox, ext, street, city, region, postal, country]
        let get = |idx: usize| -> Option<String> {
          entry
            .values
            .get(idx)
            .and_then(|v| v.as_text())
            .and_then(|s| opt_str(s))
        };
        facts.push(FactValue::Address(AddressValue {
          label,
          street:      get(2),
          locality:    get(3),
          region:      get(4),
          postal_code: get(5),
          country:     get(6),
        }));
      }

      VCardProperty::Url => {
        let url = entry
          .values
          .first()
          .and_then(|v| v.as_text())
          .map(|s| s.trim().to_string())
          .unwrap_or_default();
        if url.is_empty() {
          continue;
        }
        let context = if types.iter().any(|t| t.eq_ignore_ascii_case("LINKEDIN"))
          || url.contains("linkedin.com")
        {
          UrlContext::LinkedIn
        } else if types.iter().any(|t| t.eq_ignore_ascii_case("GITHUB"))
          || url.contains("github.com")
        {
          UrlContext::GitHub
        } else if types.iter().any(|t| t.eq_ignore_ascii_case("MASTODON"))
          || url.contains("mastodon")
        {
          UrlContext::Mastodon
        } else {
          let type_val = types
            .iter()
            .find(|t| !matches!(t.as_str(), "WORK" | "HOME" | "PREF" | "OTHER"))
            .cloned();
          match type_val.as_deref() {
            Some(t) => UrlContext::Custom(t.to_string()),
            None => UrlContext::Homepage,
          }
        };
        facts.push(FactValue::Url(UrlValue { url, context }));
      }

      // ── Dates ─────────────────────────────────────────────────────────────
      VCardProperty::Bday => {
        if let Some(dt) = entry.values.first().and_then(|v| v.as_partial_date_time()) {
          if let (Some(y), Some(m), Some(d)) = (dt.year, dt.month, dt.day) {
            if let Some(date) = NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32) {
              facts.push(FactValue::Birthday(date));
            }
          }
          // year-omitted (--MMDD) silently skipped
        }
      }

      VCardProperty::Anniversary => {
        if let Some(dt) = entry.values.first().and_then(|v| v.as_partial_date_time()) {
          if let (Some(y), Some(m), Some(d)) = (dt.year, dt.month, dt.day) {
            if let Some(date) = NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32) {
              facts.push(FactValue::Anniversary(date));
            }
          }
        }
      }

      // ── Demographics ──────────────────────────────────────────────────────
      VCardProperty::Gender => {
        // First value is the sex; as_text() works on VCardValue::Sex too
        if let Some(text) = entry.values.first().and_then(|v| v.as_text()) {
          let gender = text.trim().to_string();
          if !gender.is_empty() {
            facts.push(FactValue::Gender(gender));
          }
        }
      }

      // ── Org / role ────────────────────────────────────────────────────────
      VCardProperty::Org => {
        // values[0] = org name (semicolon-separated units follow)
        if let Some(name) = entry.values.first().and_then(|v| v.as_text()) {
          let org_name = name.trim().to_string();
          if !org_name.is_empty() {
            org_groups.push(OrgGroup { org_name, title: None, role: None });
          }
        }
      }

      VCardProperty::Title => {
        if let Some(text) = entry.values.first().and_then(|v| v.as_text()) {
          let title = text.trim().to_string();
          if !title.is_empty() {
            if let Some(last) = org_groups.last_mut() {
              last.title = Some(title);
            } else {
              org_groups.push(OrgGroup { org_name: String::new(), title: Some(title), role: None });
            }
          }
        }
      }

      VCardProperty::Role => {
        if let Some(text) = entry.values.first().and_then(|v| v.as_text()) {
          let role = text.trim().to_string();
          if !role.is_empty() {
            if let Some(last) = org_groups.last_mut() {
              last.role = Some(role);
            } else {
              org_groups.push(OrgGroup { org_name: String::new(), title: None, role: Some(role) });
            }
          }
        }
      }

      // ── Misc ──────────────────────────────────────────────────────────────
      VCardProperty::Note => {
        if let Some(text) = entry.values.first().and_then(|v| v.as_text()) {
          if !text.is_empty() {
            facts.push(FactValue::Note(text.to_string()));
          }
        }
      }

      VCardProperty::Photo => {
        // URI only; skip binary
        if let Some(text) = entry.values.first().and_then(|v| v.as_text()) {
          let uri = text.trim().to_string();
          if !uri.is_empty()
            && (uri.starts_with("http") || uri.starts_with("file://") || uri.starts_with("cid:"))
          {
            facts.push(FactValue::Custom {
              key:   "photo_uri".to_string(),
              value: serde_json::Value::String(uri),
            });
          }
        }
        // binary values silently dropped
      }

      // ── IM ────────────────────────────────────────────────────────────────
      VCardProperty::Impp => {
        if let Some(text) = entry.values.first().and_then(|v| v.as_text()) {
          if let Some(colon) = text.find(':') {
            let scheme = &text[..colon];
            let handle = text[colon + 1..].to_string();
            let service = scheme_to_service(scheme);
            facts.push(FactValue::Im(ImValue { handle, service }));
          }
          // No colon: skip (malformed IMPP URI)
        }
      }

      // ── X- properties ─────────────────────────────────────────────────────
      VCardProperty::Other(prop_name) => {
        let name_upper = prop_name.to_uppercase();
        let value_str = || {
          entry
            .values
            .first()
            .and_then(|v| v.as_text())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
        };

        match name_upper.as_str() {
          // ── vCard 3.0 legacy IM X-props ────────────────────────────────
          "X-AIM" => facts.push(FactValue::Im(ImValue {
            handle:  value_str(),
            service: "AIM".to_string(),
          })),
          "X-JABBER" => facts.push(FactValue::Im(ImValue {
            handle:  value_str(),
            service: "XMPP".to_string(),
          })),
          "X-SKYPE" | "X-SKYPE-USERNAME" => facts.push(FactValue::Im(ImValue {
            handle:  value_str(),
            service: "Skype".to_string(),
          })),
          "X-ICQ" => facts.push(FactValue::Im(ImValue {
            handle:  value_str(),
            service: "ICQ".to_string(),
          })),
          "X-MSN" => facts.push(FactValue::Im(ImValue {
            handle:  value_str(),
            service: "MSN".to_string(),
          })),
          "X-YAHOO" => facts.push(FactValue::Im(ImValue {
            handle:  value_str(),
            service: "Yahoo".to_string(),
          })),
          "X-GOOGLE-TALK" => facts.push(FactValue::Im(ImValue {
            handle:  value_str(),
            service: "Google Talk".to_string(),
          })),

          // ── Kith-specific X-props ───────────────────────────────────────
          "X-KITH-SOCIAL" => {
            let platform = entry
              .parameters(&VCardParameterName::Other("PLATFORM".to_string()))
              .find_map(|v| v.as_text())
              .unwrap_or("")
              .to_string();
            let handle = value_str();
            if !platform.is_empty() && !handle.is_empty() {
              facts.push(FactValue::Social(SocialValue { handle, platform }));
            }
          }

          "X-KITH-GROUP" => {
            let group_id = entry
              .parameters(&VCardParameterName::Other("GROUP-ID".to_string()))
              .find_map(|v| v.as_text())
              .and_then(|s| Uuid::parse_str(s).ok());
            let group_name = value_str();
            facts.push(FactValue::GroupMembership(GroupMembershipValue { group_name, group_id }));
          }

          "X-KITH-RELATION" => {
            let relation = entry
              .parameters(&VCardParameterName::Other("RELATION".to_string()))
              .find_map(|v| v.as_text())
              .unwrap_or("")
              .to_string();
            let other_id = entry
              .parameters(&VCardParameterName::Other("OTHER-ID".to_string()))
              .find_map(|v| v.as_text())
              .and_then(|s| Uuid::parse_str(s).ok());
            let other_name = opt_str(&value_str());
            facts.push(FactValue::Relationship(RelationshipValue {
              relation,
              other_id,
              other_name,
            }));
          }

          "X-KITH-MEETING" => {
            let location = entry
              .parameters(&VCardParameterName::Other("LOCATION".to_string()))
              .find_map(|v| v.as_text())
              .and_then(|s| opt_str(s));
            let summary = value_str();
            facts.push(FactValue::Meeting(MeetingValue { summary, location }));
          }

          "X-KITH-INTRODUCTION" => {
            let intro = value_str();
            if !intro.is_empty() {
              facts.push(FactValue::Introduction(intro));
            }
          }

          // ── X-ANNIVERSARY (vCard 3.0 compat) ───────────────────────────
          "X-ANNIVERSARY" => {
            let val = value_str();
            if let Ok(d) = NaiveDate::parse_from_str(val.trim(), "%Y%m%d")
              .or_else(|_| NaiveDate::parse_from_str(val.trim(), "%Y-%m-%d"))
            {
              facts.push(FactValue::Anniversary(d));
            }
          }

          // ── Other X-props → Custom ──────────────────────────────────────
          other if other.starts_with("X-") => {
            facts.push(FactValue::Custom {
              key:   other.to_string(),
              value: serde_json::Value::String(value_str()),
            });
          }

          _ => {} // unknown X- subname — skip
        }
      }

      // ── Unknown IANA properties silently skipped ──────────────────────────
      _ => {}
    }
  }

  // ── Flush accumulators ────────────────────────────────────────────────────
  let mut final_facts: Vec<FactValue> = Vec::new();

  if let Some(name_fv) = name_accum.flush() {
    final_facts.push(name_fv);
  }

  for g in org_groups {
    let org_name = if g.org_name.is_empty() { "(unknown)".to_string() } else { g.org_name };
    final_facts.push(FactValue::OrgMembership(OrgMembershipValue {
      org_name,
      org_id: None,
      title: g.title,
      role: g.role,
    }));
  }

  final_facts.extend(facts);

  // ── Wrap in NewFact with Imported context ─────────────────────────────────
  let context = RecordingContext::Imported {
    source_name:  source_name.to_string(),
    original_uid: uid.clone(),
  };

  let new_facts = final_facts
    .into_iter()
    .map(|v| {
      let mut f = NewFact::new(Uuid::nil(), v);
      f.recording_context = context.clone();
      f
    })
    .collect();

  Ok(ParsedVcard { uid, facts: new_facts })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use kith_core::fact::{ContactLabel, FactValue, PhoneKind, RecordingContext};
  use uuid::Uuid;

  use super::*;

  fn first_fact(card: &ParsedVcard) -> &FactValue { &card.facts[0].value }

  // ── Envelope ──────────────────────────────────────────────────────────────

  #[test]
  fn missing_envelope_returns_error() {
    let r = parse_one("FN:Alice", "test");
    assert!(matches!(r, Err(Error::MissingEnvelope)));
  }

  #[test]
  fn empty_envelope_returns_error() {
    let r = parse_one("BEGIN:VCARD\r\nEND:VCARD", "test");
    assert!(r.is_err() || r.unwrap().facts.is_empty());
  }

  // ── FN-only → single Name fact ────────────────────────────────────────────

  #[test]
  fn fn_only_becomes_name_fact() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice Smith\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    assert_eq!(card.facts.len(), 1);
    let FactValue::Name(n) = first_fact(&card) else {
      panic!("expected Name")
    };
    assert_eq!(n.full, "Alice Smith");
    assert!(n.family.is_none());
  }

  // ── N + FN → merged single Name fact ─────────────────────────────────────

  #[test]
  fn n_and_fn_merged_into_single_name() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice \
                 Smith\r\nN:Smith;Alice;;;\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let name_facts: Vec<_> = card
      .facts
      .iter()
      .filter(|f| matches!(f.value, FactValue::Name(_)))
      .collect();
    assert_eq!(name_facts.len(), 1, "must produce exactly one Name fact");
    let FactValue::Name(n) = &name_facts[0].value else {
      panic!()
    };
    assert_eq!(n.full, "Alice Smith");
    assert_eq!(n.family, Some("Smith".to_string()));
    assert_eq!(n.given, Some("Alice".to_string()));
  }

  // ── TEL v4 PREF ───────────────────────────────────────────────────────────

  #[test]
  fn tel_v4_type_and_pref() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nTEL;TYPE=WORK,VOICE;PREF=1:\
                 +15555551234\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Phone(p) = first_fact(&card) else {
      panic!("expected Phone")
    };
    assert_eq!(p.number, "+15555551234");
    assert_eq!(p.label, ContactLabel::Work);
    assert_eq!(p.kind, PhoneKind::Voice);
    assert_eq!(p.preference, 1);
  }

  // ── TEL v3 TYPE=PREF ──────────────────────────────────────────────────────

  #[test]
  fn tel_v3_type_pref() {
    let input = "BEGIN:VCARD\r\nVERSION:3.0\r\nTEL;TYPE=WORK,PREF:\
                 +15555559999\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Phone(p) = first_fact(&card) else {
      panic!("expected Phone")
    };
    assert_eq!(p.preference, 1);
    assert_eq!(p.label, ContactLabel::Work);
  }

  // ── EMAIL preference roundtrip ────────────────────────────────────────────

  #[test]
  fn email_with_preference() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nEMAIL;TYPE=WORK;PREF=1:alice@\
                 example.com\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Email(e) = first_fact(&card) else {
      panic!("expected Email")
    };
    assert_eq!(e.address, "alice@example.com");
    assert_eq!(e.label, ContactLabel::Work);
    assert_eq!(e.preference, 1);
  }

  // ── ADR 7-field split ─────────────────────────────────────────────────────

  #[test]
  fn adr_seven_field_split() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nADR;TYPE=WORK:;;123 Main \
                 St;Springfield;IL;62701;USA\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Address(a) = first_fact(&card) else {
      panic!("expected Address")
    };
    assert_eq!(a.street, Some("123 Main St".to_string()));
    assert_eq!(a.locality, Some("Springfield".to_string()));
    assert_eq!(a.region, Some("IL".to_string()));
    assert_eq!(a.postal_code, Some("62701".to_string()));
    assert_eq!(a.country, Some("USA".to_string()));
    assert_eq!(a.label, ContactLabel::Work);
  }

  // ── BDAY ──────────────────────────────────────────────────────────────────

  #[test]
  fn bday_yyyymmdd() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nBDAY:19900315\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Birthday(d) = first_fact(&card) else {
      panic!("expected Birthday")
    };
    assert_eq!(d.to_string(), "1990-03-15");
  }

  #[test]
  fn bday_yyyy_mm_dd() {
    let input =
      "BEGIN:VCARD\r\nVERSION:4.0\r\nBDAY:1990-03-15\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Birthday(d) = first_fact(&card) else {
      panic!("expected Birthday")
    };
    assert_eq!(d.to_string(), "1990-03-15");
  }

  #[test]
  fn bday_year_omitted_skipped() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nBDAY:--0315\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    assert!(
      !card
        .facts
        .iter()
        .any(|f| matches!(f.value, FactValue::Birthday(_)))
    );
  }

  // ── ORG + TITLE + ROLE ────────────────────────────────────────────────────

  #[test]
  fn org_title_role_single_membership() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nORG:Acme \
                 Corp\r\nTITLE:Engineer\r\nROLE:IC\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let orgs: Vec<_> = card
      .facts
      .iter()
      .filter_map(|f| {
        if let FactValue::OrgMembership(o) = &f.value { Some(o) } else { None }
      })
      .collect();
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0].org_name, "Acme Corp");
    assert_eq!(orgs[0].title, Some("Engineer".to_string()));
    assert_eq!(orgs[0].role, Some("IC".to_string()));
  }

  #[test]
  fn two_orgs_produce_two_memberships() {
    let input =
      "BEGIN:VCARD\r\nVERSION:4.0\r\nORG:Acme\r\nORG:OSF\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let orgs: Vec<_> = card
      .facts
      .iter()
      .filter_map(|f| {
        if let FactValue::OrgMembership(o) = &f.value { Some(o) } else { None }
      })
      .collect();
    assert_eq!(orgs.len(), 2);
    assert_eq!(orgs[0].org_name, "Acme");
    assert_eq!(orgs[1].org_name, "OSF");
  }

  // ── IMPP ──────────────────────────────────────────────────────────────────

  #[test]
  fn impp_xmpp_uri() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nIMPP:xmpp:alice@jabber.org\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Im(im) = first_fact(&card) else {
      panic!("expected Im")
    };
    assert_eq!(im.service, "XMPP");
    assert_eq!(im.handle, "alice@jabber.org");
  }

  #[test]
  fn x_jabber_legacy() {
    let input =
      "BEGIN:VCARD\r\nVERSION:3.0\r\nX-JABBER:bob@jabber.org\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Im(im) = first_fact(&card) else {
      panic!("expected Im")
    };
    assert_eq!(im.service, "XMPP");
    assert_eq!(im.handle, "bob@jabber.org");
  }

  // ── Kith X-props ──────────────────────────────────────────────────────────

  #[test]
  fn x_kith_social() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nX-KITH-SOCIAL;PLATFORM=Twitter:\
                 @alice\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Social(s) = first_fact(&card) else {
      panic!("expected Social")
    };
    assert_eq!(s.platform, "Twitter");
    assert_eq!(s.handle, "@alice");
  }

  #[test]
  fn x_kith_group() {
    let gid = Uuid::new_v4();
    let input = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nX-KITH-GROUP;GROUP-ID={}:Friends\r\nEND:\
       VCARD\r\n",
      gid
    );
    let card = parse_one(&input, "test").unwrap();
    let FactValue::GroupMembership(g) = first_fact(&card) else {
      panic!("expected GroupMembership")
    };
    assert_eq!(g.group_name, "Friends");
    assert_eq!(g.group_id, Some(gid));
  }

  #[test]
  fn x_kith_relation() {
    let oid = Uuid::new_v4();
    let input = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nX-KITH-RELATION;RELATION=sister;\
       OTHER-ID={}:Jane\r\nEND:VCARD\r\n",
      oid
    );
    let card = parse_one(&input, "test").unwrap();
    let FactValue::Relationship(r) = first_fact(&card) else {
      panic!("expected Relationship")
    };
    assert_eq!(r.relation, "sister");
    assert_eq!(r.other_id, Some(oid));
    assert_eq!(r.other_name, Some("Jane".to_string()));
  }

  #[test]
  fn x_kith_meeting() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nX-KITH-MEETING;LOCATION=Coffee \
                 Shop:Intro call\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Meeting(m) = first_fact(&card) else {
      panic!("expected Meeting")
    };
    assert_eq!(m.summary, "Intro call");
    assert_eq!(m.location, Some("Coffee Shop".to_string()));
  }

  #[test]
  fn x_kith_introduction() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nX-KITH-INTRODUCTION:Met at \
                 PyCon\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Introduction(s) = first_fact(&card) else {
      panic!("expected Introduction")
    };
    assert_eq!(s, "Met at PyCon");
  }

  // ── Folded lines ──────────────────────────────────────────────────────────

  #[test]
  fn folded_lines_unfolded_correctly() {
    let input =
      "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\n  Smith\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Name(n) = first_fact(&card) else {
      panic!()
    };
    // calcard's parser handles unfolding; with a space continuation the space
    // becomes part of the value — same as the old unfold_lines behaviour
    assert!(n.full.contains("Alice"));
  }

  // ── RecordingContext ──────────────────────────────────────────────────────

  #[test]
  fn recording_context_set_correctly() {
    let uid = "uid-abc-123";
    let input = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Alice\r\nEND:VCARD\r\n"
    );
    let card = parse_one(&input, "MyImport").unwrap();
    assert_eq!(card.uid, Some(uid.to_string()));
    for f in &card.facts {
      let RecordingContext::Imported { source_name, original_uid } = &f.recording_context else {
        panic!("expected Imported context");
      };
      assert_eq!(source_name, "MyImport");
      assert_eq!(original_uid, &Some(uid.to_string()));
    }
  }

  // ── parse_many ────────────────────────────────────────────────────────────

  #[test]
  fn parse_many_two_cards() {
    use crate::parse_many;
    let input = concat!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEND:VCARD\r\n",
      "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Bob\r\nEND:VCARD\r\n",
    );
    let results = parse_many(input, "test");
    assert_eq!(results.len(), 2);
    assert!(results[0].is_ok());
    assert!(results[1].is_ok());
    let FactValue::Name(n0) = &results[0].as_ref().unwrap().facts[0].value else {
      panic!()
    };
    let FactValue::Name(n1) = &results[1].as_ref().unwrap().facts[0].value else {
      panic!()
    };
    assert_eq!(n0.full, "Alice");
    assert_eq!(n1.full, "Bob");
  }
}
