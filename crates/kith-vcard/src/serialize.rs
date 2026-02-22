//! vCard 4.0 and 3.0 serializer.
//!
//! Produces CRLF line endings and folds at 75 octets per RFC 6350 §3.2.

use kith_core::{
  fact::{ContactLabel, FactValue, PhoneKind, UrlContext},
  lifecycle::ContactView,
  subject::SubjectKind,
};

use crate::error::Result;

// ─── RFC 6350 line folding ────────────────────────────────────────────────────

/// Emit `s` as one logical line, folding at 75 octets with CRLF + SP continuation.
pub(crate) fn fold_line(s: &str) -> String {
  if s.len() <= 75 {
    return format!("{}\r\n", s);
  }

  let mut result = String::new();
  let total = s.len();
  let mut pos = 0usize;
  let mut first = true;

  while pos < total {
    let limit = if first { 75 } else { 74 };
    let end   = if pos + limit >= total {
      total
    } else {
      // Walk back to the nearest valid UTF-8 char boundary
      let mut e = pos + limit;
      while e > pos && !s.is_char_boundary(e) {
        e -= 1;
      }
      // Guarantee at least one byte per segment
      if e == pos { pos + 1 } else { e }
    };

    if !first {
      result.push(' ');
    }
    result.push_str(&s[pos..end]);
    result.push_str("\r\n");
    pos   = end;
    first = false;
  }

  result
}

// ─── Value escaping ───────────────────────────────────────────────────────────

/// Escape a full property value: `\`, `,`, `;`, `\n`.
fn escape_value(s: &str) -> String {
  s.replace('\\', "\\\\")
   .replace(',', "\\,")
   .replace(';', "\\;")
   .replace('\n', "\\n")
}

/// Escape a semicolon-delimited component (N / ADR field): `\`, `;`, `\n`.
/// Commas are list-separators within a component and are not escaped here.
fn escape_component(s: &str) -> String {
  s.replace('\\', "\\\\")
   .replace(';', "\\;")
   .replace('\n', "\\n")
}

// ─── TYPE / PREF helpers ──────────────────────────────────────────────────────

fn label_type_str(label: &ContactLabel) -> &'static str {
  match label {
    ContactLabel::Work       => "WORK",
    ContactLabel::Home       => "HOME",
    ContactLabel::Other      => "OTHER",
    ContactLabel::Custom(_)  => "OTHER",
  }
}

fn phone_kind_str(kind: PhoneKind) -> &'static str {
  match kind {
    PhoneKind::Voice  => "VOICE",
    PhoneKind::Fax    => "FAX",
    PhoneKind::Cell   => "CELL",
    PhoneKind::Pager  => "PAGER",
    PhoneKind::Text   => "TEXT",
    PhoneKind::Video  => "VIDEO",
    PhoneKind::Other  => "OTHER",
  }
}

fn url_context_type(ctx: &UrlContext) -> String {
  match ctx {
    UrlContext::Homepage    => "HOME".to_string(),
    UrlContext::LinkedIn    => "LINKEDIN".to_string(),
    UrlContext::GitHub      => "GITHUB".to_string(),
    UrlContext::Mastodon    => "MASTODON".to_string(),
    UrlContext::Custom(s)   => s.clone(),
  }
}

fn format_naive_date(d: chrono::NaiveDate) -> String {
  d.format("%Y%m%d").to_string()
}

// ─── IM scheme helpers ─────────────────────────────────────────────────────────

fn service_to_scheme(service: &str) -> &'static str {
  match service.to_lowercase().as_str() {
    "xmpp" | "jabber"  => "xmpp",
    "sip"              => "sip",
    "aim"              => "aim",
    "yahoo"            => "ymsgr",
    "msn"              => "msnim",
    "google talk"      => "gtalk",
    "skype"            => "skype",
    "irc"              => "irc",
    "matrix"           => "matrix",
    _                  => "x-unknown",
  }
}

fn service_to_x_prop(service: &str) -> &'static str {
  match service.to_lowercase().as_str() {
    "xmpp" | "jabber"  => "X-JABBER",
    "aim"              => "X-AIM",
    "yahoo"            => "X-YAHOO",
    "msn"              => "X-MSN",
    "skype"            => "X-SKYPE",
    "icq"              => "X-ICQ",
    "google talk"      => "X-GOOGLE-TALK",
    _                  => "X-IM",
  }
}

// ─── Inner serializer (shared between v3 / v4) ────────────────────────────────

fn serialize_body(view: &ContactView, v4: bool) -> Result<String> {
  let facts: Vec<&FactValue> =
    view.active_facts.iter().map(|rf| &rf.fact.value).collect();

  // Collect OrgMembership facts separately for group-prefix logic
  let org_memberships: Vec<&kith_core::fact::OrgMembershipValue> = facts
    .iter()
    .filter_map(|f| {
      if let FactValue::OrgMembership(o) = f { Some(o) } else { None }
    })
    .collect();
  let multi_org = org_memberships.len() > 1;

  let mut lines: Vec<String> = Vec::new();

  // v3 requires FN + N; emit blanks if no Name fact present
  if !v4 && !facts.iter().any(|f| matches!(f, FactValue::Name(_))) {
    lines.push(fold_line("FN:"));
    lines.push(fold_line("N:;;;;"));
  }

  for fact in &facts {
    match fact {
      FactValue::Name(n) => {
        lines.push(fold_line(&format!("FN:{}", escape_value(&n.full))));
        let family     = n.family    .as_deref().map(escape_component).unwrap_or_default();
        let given      = n.given     .as_deref().map(escape_component).unwrap_or_default();
        let additional = n.additional.as_deref().map(escape_component).unwrap_or_default();
        let prefix     = n.prefix    .as_deref().map(escape_component).unwrap_or_default();
        let suffix     = n.suffix    .as_deref().map(escape_component).unwrap_or_default();
        lines.push(fold_line(&format!("N:{};{};{};{};{}", family, given, additional, prefix, suffix)));
      }

      FactValue::Alias(a) => {
        lines.push(fold_line(&format!("NICKNAME:{}", escape_value(&a.name))));
      }

      FactValue::Photo(p) => {
        lines.push(fold_line(&format!("PHOTO;VALUE=URI:{}", p.path)));
      }

      FactValue::Birthday(d) => {
        lines.push(fold_line(&format!("BDAY:{}", format_naive_date(*d))));
      }

      FactValue::Anniversary(d) => {
        let prop = if v4 { "ANNIVERSARY" } else { "X-ANNIVERSARY" };
        lines.push(fold_line(&format!("{}:{}", prop, format_naive_date(*d))));
      }

      FactValue::Gender(g) => {
        if v4 {
          lines.push(fold_line(&format!("GENDER:{}", escape_value(g))));
        }
        // v3: omitted
      }

      FactValue::Email(e) => {
        let type_str = label_type_str(&e.label);
        let line = if v4 {
          if e.preference < 255 {
            format!("EMAIL;TYPE={};PREF={}:{}", type_str, e.preference, e.address)
          } else {
            format!("EMAIL;TYPE={}:{}", type_str, e.address)
          }
        } else {
          if e.preference < 255 {
            format!("EMAIL;TYPE={},PREF:{}", type_str, e.address)
          } else {
            format!("EMAIL;TYPE={}:{}", type_str, e.address)
          }
        };
        lines.push(fold_line(&line));
      }

      FactValue::Phone(p) => {
        let type_str = label_type_str(&p.label);
        let kind_str = phone_kind_str(p.kind);
        let line = if v4 {
          if p.preference < 255 {
            format!("TEL;TYPE={},{};PREF={}:{}", type_str, kind_str, p.preference, p.number)
          } else {
            format!("TEL;TYPE={},{}:{}", type_str, kind_str, p.number)
          }
        } else {
          if p.preference < 255 {
            format!("TEL;TYPE={},{},PREF:{}", type_str, kind_str, p.number)
          } else {
            format!("TEL;TYPE={},{}:{}", type_str, kind_str, p.number)
          }
        };
        lines.push(fold_line(&line));
      }

      FactValue::Address(a) => {
        let type_str    = label_type_str(&a.label);
        let street      = a.street     .as_deref().map(escape_component).unwrap_or_default();
        let locality    = a.locality   .as_deref().map(escape_component).unwrap_or_default();
        let region      = a.region     .as_deref().map(escape_component).unwrap_or_default();
        let postal_code = a.postal_code.as_deref().map(escape_component).unwrap_or_default();
        let country     = a.country    .as_deref().map(escape_component).unwrap_or_default();
        lines.push(fold_line(&format!(
          "ADR;TYPE={}:;;{};{};{};{};{}",
          type_str, street, locality, region, postal_code, country
        )));
      }

      FactValue::Url(u) => {
        let ctx_str = url_context_type(&u.context);
        lines.push(fold_line(&format!("URL;TYPE={}:{}", ctx_str, u.url)));
      }

      FactValue::Im(im) => {
        if v4 {
          let scheme = service_to_scheme(&im.service);
          lines.push(fold_line(&format!("IMPP:{}:{}", scheme, im.handle)));
        } else {
          let prop = service_to_x_prop(&im.service);
          lines.push(fold_line(&format!("{}:{}", prop, escape_value(&im.handle))));
        }
      }

      FactValue::Social(s) => {
        lines.push(fold_line(&format!(
          "X-KITH-SOCIAL;PLATFORM={}:{}",
          s.platform,
          escape_value(&s.handle)
        )));
      }

      FactValue::Relationship(r) => {
        let mut prop = format!("X-KITH-RELATION;RELATION={}", r.relation);
        if let Some(oid) = r.other_id {
          prop.push_str(&format!(";OTHER-ID={}", oid));
        }
        let other_name = r.other_name.as_deref().map(escape_value).unwrap_or_default();
        lines.push(fold_line(&format!("{}:{}", prop, other_name)));
      }

      FactValue::GroupMembership(g) => {
        let mut prop = "X-KITH-GROUP".to_string();
        if let Some(gid) = g.group_id {
          prop.push_str(&format!(";GROUP-ID={}", gid));
        }
        lines.push(fold_line(&format!("{}:{}", prop, escape_value(&g.group_name))));
      }

      FactValue::Note(n) => {
        lines.push(fold_line(&format!("NOTE:{}", escape_value(n))));
      }

      FactValue::Meeting(m) => {
        let mut prop = "X-KITH-MEETING".to_string();
        if let Some(ref loc) = m.location {
          prop.push_str(&format!(";LOCATION={}", loc));
        }
        lines.push(fold_line(&format!("{}:{}", prop, escape_value(&m.summary))));
      }

      FactValue::Introduction(s) => {
        lines.push(fold_line(&format!("X-KITH-INTRODUCTION:{}", escape_value(s))));
      }

      FactValue::Custom { key, value } => {
        let val_str = match value {
          serde_json::Value::String(s) => s.clone(),
          other => other.to_string(),
        };
        let prop_name = if key.to_uppercase().starts_with("X-") {
          key.to_uppercase()
        } else {
          format!("X-{}", key.to_uppercase())
        };
        lines.push(fold_line(&format!("{}:{}", prop_name, escape_value(&val_str))));
      }

      // Handled below with group-prefix logic
      FactValue::OrgMembership(_) => {}
    }
  }

  // ── OrgMembership with optional group prefix ──────────────────────────────
  for (idx, org) in org_memberships.iter().enumerate() {
    let prefix = if multi_org { format!("ORG{}.", idx + 1) } else { String::new() };
    lines.push(fold_line(&format!("{}ORG:{}", prefix, escape_value(&org.org_name))));
    if let Some(ref title) = org.title {
      lines.push(fold_line(&format!("{}TITLE:{}", prefix, escape_value(title))));
    }
    if let Some(ref role) = org.role {
      lines.push(fold_line(&format!("{}ROLE:{}", prefix, escape_value(role))));
    }
  }

  Ok(lines.join(""))
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Serialize `view` as a vCard 4.0 string.
pub fn serialize(view: &ContactView) -> Result<String> {
  let kind_str = match view.subject.kind {
    SubjectKind::Person       => "individual",
    SubjectKind::Organization => "org",
    SubjectKind::Group        => "group",
  };
  let rev = view.as_of.format("%Y%m%dT%H%M%SZ").to_string();

  let mut out = String::new();
  out.push_str("BEGIN:VCARD\r\n");
  out.push_str("VERSION:4.0\r\n");
  out.push_str(&fold_line(&format!("UID:{}", view.subject.subject_id)));
  out.push_str("PRODID:-//Kith//Kith vCard//EN\r\n");
  out.push_str(&fold_line(&format!("REV:{}", rev)));
  out.push_str(&fold_line(&format!("KIND:{}", kind_str)));
  out.push_str(&serialize_body(view, true)?);
  out.push_str("END:VCARD\r\n");
  Ok(out)
}

/// Serialize `view` as a vCard 3.0 string.
pub fn serialize_v3(view: &ContactView) -> Result<String> {
  let rev = view.as_of.format("%Y%m%dT%H%M%SZ").to_string();

  let mut out = String::new();
  out.push_str("BEGIN:VCARD\r\n");
  out.push_str("VERSION:3.0\r\n");
  out.push_str(&fold_line(&format!("UID:{}", view.subject.subject_id)));
  out.push_str("PRODID:-//Kith//Kith vCard//EN\r\n");
  out.push_str(&fold_line(&format!("REV:{}", rev)));
  // KIND is omitted in vCard 3.0
  out.push_str(&serialize_body(view, false)?);
  out.push_str("END:VCARD\r\n");
  Ok(out)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::{NaiveDate, TimeZone, Utc};
  use kith_core::{
    fact::{
      AddressValue, ContactLabel, EmailValue, FactValue, NameValue, OrgMembershipValue,
      PhoneKind, PhoneValue, RecordingContext, SocialValue,
    },
    lifecycle::{ContactView, FactStatus, ResolvedFact},
    subject::{Subject, SubjectKind},
  };
  use uuid::Uuid;

  fn make_view(facts: Vec<FactValue>) -> ContactView {
    let subject_id = Uuid::new_v4();
    let subject    = Subject {
      subject_id,
      created_at: Utc::now(),
      kind:       SubjectKind::Person,
    };
    let as_of      = Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap();
    let active_facts = facts.into_iter().map(|v| {
      let fact = kith_core::fact::Fact {
        fact_id:           Uuid::new_v4(),
        subject_id,
        value:             v,
        recorded_at:       as_of,
        effective_at:      None,
        effective_until:   None,
        source:            None,
        confidence:        kith_core::fact::Confidence::Certain,
        recording_context: RecordingContext::Manual,
        tags:              vec![],
      };
      ResolvedFact { fact, status: FactStatus::Active }
    }).collect();
    ContactView { subject, as_of, active_facts }
  }

  // ── Envelope ────────────────────────────────────────────────────────────────

  #[test]
  fn envelope_contains_required_lines() {
    let view = make_view(vec![]);
    let out  = serialize(&view).unwrap();
    assert!(out.contains("BEGIN:VCARD\r\n"));
    assert!(out.contains("VERSION:4.0\r\n"));
    assert!(out.contains("UID:"));
    assert!(out.contains("END:VCARD\r\n"));
  }

  // ── Name ────────────────────────────────────────────────────────────────────

  #[test]
  fn name_emits_fn_and_n() {
    let name = FactValue::Name(NameValue {
      given:      Some("Alice".to_string()),
      family:     Some("Smith".to_string()),
      additional: None,
      prefix:     None,
      suffix:     None,
      full:       "Alice Smith".to_string(),
    });
    let out = serialize(&make_view(vec![name])).unwrap();
    assert!(out.contains("FN:Alice Smith\r\n"), "missing FN in:\n{out}");
    assert!(out.contains("N:Smith;Alice;;;\r\n"), "missing N in:\n{out}");
  }

  // ── Email ────────────────────────────────────────────────────────────────────

  #[test]
  fn email_with_type_and_pref() {
    let email = FactValue::Email(EmailValue {
      address:    "alice@example.com".to_string(),
      label:      ContactLabel::Work,
      preference: 1,
    });
    let out = serialize(&make_view(vec![email])).unwrap();
    assert!(out.contains("EMAIL;TYPE=WORK;PREF=1:alice@example.com\r\n"), "got:\n{out}");
  }

  #[test]
  fn email_without_pref_when_preference_255() {
    let email = FactValue::Email(EmailValue {
      address:    "alice@example.com".to_string(),
      label:      ContactLabel::Work,
      preference: 255,
    });
    let out = serialize(&make_view(vec![email])).unwrap();
    assert!(!out.contains("PREF"), "unexpected PREF in:\n{out}");
    assert!(out.contains("EMAIL;TYPE=WORK:alice@example.com\r\n"));
  }

  // ── Phone ────────────────────────────────────────────────────────────────────

  #[test]
  fn phone_without_pref_when_preference_255() {
    let phone = FactValue::Phone(PhoneValue {
      number:     "+15555551234".to_string(),
      label:      ContactLabel::Home,
      kind:       PhoneKind::Voice,
      preference: 255,
    });
    let out = serialize(&make_view(vec![phone])).unwrap();
    assert!(!out.contains("PREF"), "unexpected PREF in:\n{out}");
    assert!(out.contains("TEL;TYPE=HOME,VOICE:+15555551234\r\n"));
  }

  // ── Line folding ─────────────────────────────────────────────────────────────

  #[test]
  fn long_note_is_folded() {
    let note = FactValue::Note("A".repeat(200));
    let out  = serialize(&make_view(vec![note])).unwrap();
    for physical_line in out.split("\r\n").filter(|l| !l.is_empty()) {
      assert!(
        physical_line.len() <= 75,
        "physical line too long ({} bytes): {:?}",
        physical_line.len(), physical_line
      );
    }
  }

  // ── Address escaping ─────────────────────────────────────────────────────────

  #[test]
  fn semicolons_in_address_are_escaped() {
    let addr = FactValue::Address(AddressValue {
      label:       ContactLabel::Work,
      street:      Some("123 Main; Suite 4".to_string()),
      locality:    None,
      region:      None,
      postal_code: None,
      country:     None,
    });
    let out = serialize(&make_view(vec![addr])).unwrap();
    assert!(out.contains("123 Main\\; Suite 4"), "missing escape in:\n{out}");
  }

  // ── Multiple OrgMembership → group prefixes ───────────────────────────────────

  #[test]
  fn two_org_memberships_get_prefixes() {
    let o1 = FactValue::OrgMembership(OrgMembershipValue {
      org_name: "Acme Corp".to_string(),
      org_id:   None,
      title:    Some("Engineer".to_string()),
      role:     None,
    });
    let o2 = FactValue::OrgMembership(OrgMembershipValue {
      org_name: "OSF".to_string(),
      org_id:   None,
      title:    Some("Board Member".to_string()),
      role:     None,
    });
    let out = serialize(&make_view(vec![o1, o2])).unwrap();
    assert!(out.contains("ORG1.ORG:Acme Corp\r\n"), "missing ORG1.ORG in:\n{out}");
    assert!(out.contains("ORG1.TITLE:Engineer\r\n"));
    assert!(out.contains("ORG2.ORG:OSF\r\n"));
    assert!(out.contains("ORG2.TITLE:Board Member\r\n"));
  }

  #[test]
  fn single_org_has_no_prefix() {
    let o = FactValue::OrgMembership(OrgMembershipValue {
      org_name: "Acme".to_string(),
      org_id:   None,
      title:    None,
      role:     None,
    });
    let out = serialize(&make_view(vec![o])).unwrap();
    assert!(out.contains("ORG:Acme\r\n"), "got:\n{out}");
    assert!(!out.contains("ORG1."), "unexpected prefix in:\n{out}");
  }

  // ── X-KITH-SOCIAL ────────────────────────────────────────────────────────────

  #[test]
  fn social_emitted_correctly() {
    let s = FactValue::Social(SocialValue {
      handle:   "@alice".to_string(),
      platform: "Twitter".to_string(),
    });
    let out = serialize(&make_view(vec![s])).unwrap();
    assert!(out.contains("X-KITH-SOCIAL;PLATFORM=Twitter:@alice\r\n"), "got:\n{out}");
  }

  // ── v3 differences ───────────────────────────────────────────────────────────

  #[test]
  fn v3_anniversary_becomes_x_anniversary() {
    let ann = FactValue::Anniversary(NaiveDate::from_ymd_opt(2020, 6, 15).unwrap());
    let out = serialize_v3(&make_view(vec![ann])).unwrap();
    assert!(out.contains("X-ANNIVERSARY:20200615\r\n"), "got:\n{out}");
    // Ensure the bare RFC 6350 "ANNIVERSARY:" line is absent (not just any substring)
    assert!(!out.contains("\r\nANNIVERSARY:"), "bare ANNIVERSARY present in v3:\n{out}");
  }

  #[test]
  fn v3_kind_omitted() {
    let out = serialize_v3(&make_view(vec![])).unwrap();
    assert!(!out.contains("KIND:"), "unexpected KIND in v3:\n{out}");
  }

  #[test]
  fn v3_pref_in_type_list() {
    let email = FactValue::Email(EmailValue {
      address:    "a@b.com".to_string(),
      label:      ContactLabel::Work,
      preference: 1,
    });
    let out = serialize_v3(&make_view(vec![email])).unwrap();
    assert!(out.contains("EMAIL;TYPE=WORK,PREF:a@b.com\r\n"), "got:\n{out}");
  }

  #[test]
  fn v3_gender_omitted() {
    let g = FactValue::Gender("M".to_string());
    let out = serialize_v3(&make_view(vec![g])).unwrap();
    assert!(!out.contains("GENDER:"), "unexpected GENDER in v3:\n{out}");
  }
}
