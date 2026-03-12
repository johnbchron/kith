//! vCard 4.0 and 3.0 serializer backed by [`calcard`].

use calcard::vcard::{
  VCard, VCardEntry, VCardKind, VCardParameter, VCardParameterName, VCardParameterValue,
  VCardProperty, VCardType, VCardValue, VCardVersion,
};
use kith_core::{
  fact::{ContactLabel, FactValue, PhoneKind, UrlContext},
  lifecycle::ContactView,
  subject::SubjectKind,
};

use crate::error::Result;

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Build a TYPE parameter with a VCardType enum value.
fn type_param(t: VCardType) -> VCardParameter {
  VCardParameter::new(VCardParameterName::Type, VCardParameterValue::Type(t))
}

/// Build a PREF parameter.
fn pref_param(pref: u8) -> VCardParameter {
  VCardParameter::new(VCardParameterName::Pref, VCardParameterValue::Integer(pref as u32))
}

/// Build a v3-style TYPE=PREF text parameter.
fn type_pref_param() -> VCardParameter {
  VCardParameter::new(
    VCardParameterName::Type,
    VCardParameterValue::Text("PREF".to_string()),
  )
}

fn format_naive_date(d: chrono::NaiveDate) -> String { d.format("%Y%m%d").to_string() }

fn service_to_scheme(service: &str) -> &'static str {
  match service.to_lowercase().as_str() {
    "xmpp" | "jabber" => "xmpp",
    "sip" => "sip",
    "aim" => "aim",
    "yahoo" => "ymsgr",
    "msn" => "msnim",
    "google talk" => "gtalk",
    "skype" => "skype",
    "irc" => "irc",
    "matrix" => "matrix",
    _ => "x-unknown",
  }
}

fn service_to_x_prop(service: &str) -> &'static str {
  match service.to_lowercase().as_str() {
    "xmpp" | "jabber" => "X-JABBER",
    "aim" => "X-AIM",
    "yahoo" => "X-YAHOO",
    "msn" => "X-MSN",
    "skype" => "X-SKYPE",
    "icq" => "X-ICQ",
    "google talk" => "X-GOOGLE-TALK",
    _ => "X-IM",
  }
}

fn url_context_type_str(ctx: &UrlContext) -> String {
  match ctx {
    UrlContext::Homepage => "HOME".to_string(),
    UrlContext::LinkedIn => "LINKEDIN".to_string(),
    UrlContext::GitHub => "GITHUB".to_string(),
    UrlContext::Mastodon => "MASTODON".to_string(),
    UrlContext::Custom(s) => s.clone(),
  }
}

// ─── Inner serializer (shared between v3 / v4) ────────────────────────────────

fn build_vcard(view: &ContactView, v4: bool) -> Result<VCard> {
  let mut vcard = VCard { entries: vec![] };
  let facts: Vec<&FactValue> = view.active_facts.iter().map(|rf| &rf.fact.value).collect();

  // Collect OrgMembership facts for group-prefix logic
  let org_memberships: Vec<&kith_core::fact::OrgMembershipValue> = facts
    .iter()
    .filter_map(|f| if let FactValue::OrgMembership(o) = f { Some(o) } else { None })
    .collect();
  let multi_org = org_memberships.len() > 1;

  // ── Header entries ────────────────────────────────────────────────────────
  vcard.entries.push(
    VCardEntry::new(VCardProperty::Uid)
      .with_value(VCardValue::Text(view.subject.subject_id.to_string())),
  );
  vcard.entries.push(
    VCardEntry::new(VCardProperty::Prodid)
      .with_value(VCardValue::Text("-//Kith//Kith vCard//EN".to_string())),
  );
  let rev = view.as_of.format("%Y%m%dT%H%M%SZ").to_string();
  vcard.entries.push(
    VCardEntry::new(VCardProperty::Rev).with_value(VCardValue::Text(rev)),
  );
  if v4 {
    let kind = match view.subject.kind {
      SubjectKind::Person => VCardKind::Individual,
      SubjectKind::Organization => VCardKind::Org,
      SubjectKind::Group => VCardKind::Group,
    };
    vcard.entries.push(
      VCardEntry::new(VCardProperty::Kind).with_value(VCardValue::Kind(kind)),
    );
  }

  // v3 requires FN + N; emit blanks if no Name fact present
  if !v4 && !facts.iter().any(|f| matches!(f, FactValue::Name(_))) {
    vcard.entries.push(
      VCardEntry::new(VCardProperty::Fn).with_value(VCardValue::Text(String::new())),
    );
    vcard.entries.push(
      VCardEntry::new(VCardProperty::N)
        .with_value(VCardValue::Text(String::new()))
        .with_value(VCardValue::Text(String::new()))
        .with_value(VCardValue::Text(String::new()))
        .with_value(VCardValue::Text(String::new()))
        .with_value(VCardValue::Text(String::new())),
    );
  }

  // ── Fact entries ──────────────────────────────────────────────────────────
  for fact in &facts {
    match fact {
      FactValue::Name(n) => {
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Fn)
            .with_value(VCardValue::Text(n.full.clone())),
        );
        vcard.entries.push(
          VCardEntry::new(VCardProperty::N)
            .with_value(VCardValue::Text(n.family.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(n.given.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(n.additional.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(n.prefix.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(n.suffix.clone().unwrap_or_default())),
        );
      }

      FactValue::Alias(a) => {
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Nickname)
            .with_value(VCardValue::Text(a.name.clone())),
        );
      }

      FactValue::Photo(p) => {
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Photo)
            .with_value(VCardValue::Text(p.path.clone())),
        );
      }

      FactValue::Birthday(d) => {
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Bday)
            .with_value(VCardValue::Text(format_naive_date(*d))),
        );
      }

      FactValue::Anniversary(d) => {
        let prop = if v4 {
          VCardProperty::Anniversary
        } else {
          VCardProperty::Other("X-ANNIVERSARY".to_string())
        };
        vcard.entries.push(
          VCardEntry::new(prop).with_value(VCardValue::Text(format_naive_date(*d))),
        );
      }

      FactValue::Gender(g) => {
        if v4 {
          vcard.entries.push(
            VCardEntry::new(VCardProperty::Gender)
              .with_value(VCardValue::Text(g.clone())),
          );
        }
        // v3: omitted
      }

      FactValue::Email(e) => {
        let label_type = match e.label {
          ContactLabel::Work => VCardType::Work,
          ContactLabel::Home => VCardType::Home,
          _ => VCardType::Work, // default
        };
        let mut entry = VCardEntry::new(VCardProperty::Email)
          .with_value(VCardValue::Text(e.address.clone()))
          .with_param(type_param(label_type));

        if e.preference < 255 {
          if v4 {
            entry = entry.with_param(pref_param(e.preference));
          } else {
            // v3: TYPE=PREF as an additional TYPE param
            entry = entry.with_param(type_pref_param());
          }
        }
        vcard.entries.push(entry);
      }

      FactValue::Phone(p) => {
        let label_type = match p.label {
          ContactLabel::Work => VCardType::Work,
          ContactLabel::Home => VCardType::Home,
          _ => VCardType::Home, // default to HOME for phones
        };
        let kind_type = match p.kind {
          PhoneKind::Voice => VCardType::Voice,
          PhoneKind::Fax => VCardType::Fax,
          PhoneKind::Cell => VCardType::Cell,
          PhoneKind::Pager => VCardType::Pager,
          PhoneKind::Text => VCardType::Text,
          PhoneKind::Video => VCardType::Video,
          PhoneKind::Other => VCardType::Voice, // fallback
        };
        let mut entry = VCardEntry::new(VCardProperty::Tel)
          .with_value(VCardValue::Text(p.number.clone()))
          .with_param(type_param(label_type))
          .with_param(type_param(kind_type));

        if p.preference < 255 {
          if v4 {
            entry = entry.with_param(pref_param(p.preference));
          } else {
            entry = entry.with_param(type_pref_param());
          }
        }
        vcard.entries.push(entry);
      }

      FactValue::Address(a) => {
        let label_type = match a.label {
          ContactLabel::Work => VCardType::Work,
          ContactLabel::Home => VCardType::Home,
          _ => VCardType::Work,
        };
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Adr)
            .with_param(type_param(label_type))
            // pobox, ext, street, city, region, postal, country
            .with_value(VCardValue::Text(String::new())) // pobox
            .with_value(VCardValue::Text(String::new())) // ext
            .with_value(VCardValue::Text(a.street.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(a.locality.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(a.region.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(a.postal_code.clone().unwrap_or_default()))
            .with_value(VCardValue::Text(a.country.clone().unwrap_or_default())),
        );
      }

      FactValue::Url(u) => {
        let ctx_str = url_context_type_str(&u.context);
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Url)
            .with_param(VCardParameter::new(
              VCardParameterName::Type,
              VCardParameterValue::Text(ctx_str),
            ))
            .with_value(VCardValue::Text(u.url.clone())),
        );
      }

      FactValue::Im(im) => {
        if v4 {
          let scheme = service_to_scheme(&im.service);
          vcard.entries.push(
            VCardEntry::new(VCardProperty::Impp)
              .with_value(VCardValue::Text(format!("{}:{}", scheme, im.handle))),
          );
        } else {
          let prop = service_to_x_prop(&im.service);
          vcard.entries.push(
            VCardEntry::new(VCardProperty::Other(prop.to_string()))
              .with_value(VCardValue::Text(im.handle.clone())),
          );
        }
      }

      FactValue::Social(s) => {
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Other("X-KITH-SOCIAL".to_string()))
            .with_param(VCardParameter::new(
              VCardParameterName::Other("PLATFORM".to_string()),
              VCardParameterValue::Text(s.platform.clone()),
            ))
            .with_value(VCardValue::Text(s.handle.clone())),
        );
      }

      FactValue::Relationship(r) => {
        let mut entry =
          VCardEntry::new(VCardProperty::Other("X-KITH-RELATION".to_string()))
            .with_param(VCardParameter::new(
              VCardParameterName::Other("RELATION".to_string()),
              VCardParameterValue::Text(r.relation.clone()),
            ));
        if let Some(oid) = r.other_id {
          entry = entry.with_param(VCardParameter::new(
            VCardParameterName::Other("OTHER-ID".to_string()),
            VCardParameterValue::Text(oid.to_string()),
          ));
        }
        entry = entry.with_value(VCardValue::Text(
          r.other_name.clone().unwrap_or_default(),
        ));
        vcard.entries.push(entry);
      }

      FactValue::GroupMembership(g) => {
        let mut entry =
          VCardEntry::new(VCardProperty::Other("X-KITH-GROUP".to_string()));
        if let Some(gid) = g.group_id {
          entry = entry.with_param(VCardParameter::new(
            VCardParameterName::Other("GROUP-ID".to_string()),
            VCardParameterValue::Text(gid.to_string()),
          ));
        }
        entry = entry.with_value(VCardValue::Text(g.group_name.clone()));
        vcard.entries.push(entry);
      }

      FactValue::Note(n) => {
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Note)
            .with_value(VCardValue::Text(n.clone())),
        );
      }

      FactValue::Meeting(m) => {
        let mut entry =
          VCardEntry::new(VCardProperty::Other("X-KITH-MEETING".to_string()));
        if let Some(ref loc) = m.location {
          entry = entry.with_param(VCardParameter::new(
            VCardParameterName::Other("LOCATION".to_string()),
            VCardParameterValue::Text(loc.clone()),
          ));
        }
        entry = entry.with_value(VCardValue::Text(m.summary.clone()));
        vcard.entries.push(entry);
      }

      FactValue::Introduction(s) => {
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Other("X-KITH-INTRODUCTION".to_string()))
            .with_value(VCardValue::Text(s.clone())),
        );
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
        vcard.entries.push(
          VCardEntry::new(VCardProperty::Other(prop_name))
            .with_value(VCardValue::Text(val_str)),
        );
      }

      // Handled below with group-prefix logic
      FactValue::OrgMembership(_) => {}
    }
  }

  // ── OrgMembership with optional group prefix ───────────────────────────────
  for (idx, org) in org_memberships.iter().enumerate() {
    let group_name = if multi_org { Some(format!("ORG{}", idx + 1)) } else { None };

    let org_entry = VCardEntry::new(VCardProperty::Org)
      .with_group(group_name.clone())
      .with_value(VCardValue::Text(org.org_name.clone()));
    vcard.entries.push(org_entry);

    if let Some(ref title) = org.title {
      vcard.entries.push(
        VCardEntry::new(VCardProperty::Title)
          .with_group(group_name.clone())
          .with_value(VCardValue::Text(title.clone())),
      );
    }
    if let Some(ref role) = org.role {
      vcard.entries.push(
        VCardEntry::new(VCardProperty::Role)
          .with_group(group_name.clone())
          .with_value(VCardValue::Text(role.clone())),
      );
    }
  }

  Ok(vcard)
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Serialize `view` as a vCard 4.0 string.
pub fn serialize(view: &ContactView) -> Result<String> {
  let vcard = build_vcard(view, true)?;
  let mut out = String::new();
  vcard.write_to(&mut out, VCardVersion::V4_0).map_err(|e| {
    crate::error::Error::MalformedContentLine(format!("write error: {e}"))
  })?;
  Ok(out)
}

/// Serialize `view` as a vCard 3.0 string.
pub fn serialize_v3(view: &ContactView) -> Result<String> {
  let vcard = build_vcard(view, false)?;
  let mut out = String::new();
  vcard.write_to(&mut out, VCardVersion::V3_0).map_err(|e| {
    crate::error::Error::MalformedContentLine(format!("write error: {e}"))
  })?;
  Ok(out)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use chrono::NaiveDate;
  use kith_core::fact::{
    AddressValue, ContactLabel, EmailValue, FactValue, NameValue, OrgMembershipValue, PhoneKind,
    PhoneValue, SocialValue,
  };

  use crate::test_helpers::make_view;

  use super::*;

  // ── Envelope ──────────────────────────────────────────────────────────────

  #[test]
  fn envelope_contains_required_lines() {
    let view = make_view(vec![]);
    let out = serialize(&view).unwrap();
    assert!(out.contains("BEGIN:VCARD\r\n"));
    assert!(out.contains("VERSION:4.0\r\n"));
    assert!(out.contains("UID:"));
    assert!(out.contains("END:VCARD\r\n"));
  }

  // ── Name ──────────────────────────────────────────────────────────────────

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

  // ── Email ─────────────────────────────────────────────────────────────────

  #[test]
  fn email_with_type_and_pref() {
    let email = FactValue::Email(EmailValue {
      address:    "alice@example.com".to_string(),
      label:      ContactLabel::Work,
      preference: 1,
    });
    let out = serialize(&make_view(vec![email])).unwrap();
    // Check all required parts are present (order-flexible)
    assert!(out.contains("alice@example.com"), "missing address in:\n{out}");
    assert!(out.contains("TYPE=WORK"), "missing TYPE=WORK in:\n{out}");
    assert!(out.contains("PREF=1"), "missing PREF=1 in:\n{out}");
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

  // ── Phone ─────────────────────────────────────────────────────────────────

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
    assert!(out.contains("+15555551234"), "missing number in:\n{out}");
    assert!(out.contains("TYPE=HOME"), "missing HOME in:\n{out}");
    assert!(out.contains("TYPE=HOME,VOICE") || out.contains("TYPE=VOICE"), "missing VOICE in:\n{out}");
  }

  // ── Line folding ──────────────────────────────────────────────────────────

  #[test]
  fn long_note_is_folded() {
    let note = FactValue::Note("A".repeat(200));
    let out = serialize(&make_view(vec![note])).unwrap();
    // calcard folds at 75 content chars but doesn't count the property-name
    // colon in the line length, so the first physical segment can be up to
    // 76 bytes.  Continuation lines (starting with a space) are up to 75.
    let lines: Vec<&str> = out.split("\r\n").filter(|l| !l.is_empty()).collect();
    for line in &lines {
      let max = if line.starts_with(' ') { 75 } else { 76 };
      assert!(
        line.len() <= max,
        "physical line too long ({} bytes): {:?}",
        line.len(),
        line
      );
    }
    // Must actually be folded (more than 1 physical line for the NOTE)
    assert!(lines.len() > 2, "long note should produce multiple physical lines");
  }

  // ── Address escaping ──────────────────────────────────────────────────────

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
    assert!(
      out.contains("123 Main\\; Suite 4"),
      "missing escape in:\n{out}"
    );
  }

  // ── Multiple OrgMembership → group prefixes ───────────────────────────────

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
    assert!(
      out.contains("ORG1.ORG:Acme Corp\r\n"),
      "missing ORG1.ORG in:\n{out}"
    );
    assert!(out.contains("ORG1.TITLE:Engineer\r\n"), "got:\n{out}");
    assert!(out.contains("ORG2.ORG:OSF\r\n"), "got:\n{out}");
    assert!(out.contains("ORG2.TITLE:Board Member\r\n"), "got:\n{out}");
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

  // ── X-KITH-SOCIAL ─────────────────────────────────────────────────────────

  #[test]
  fn social_emitted_correctly() {
    let s = FactValue::Social(SocialValue {
      handle:   "@alice".to_string(),
      platform: "Twitter".to_string(),
    });
    let out = serialize(&make_view(vec![s])).unwrap();
    assert!(
      out.contains("X-KITH-SOCIAL;PLATFORM=Twitter:@alice\r\n"),
      "got:\n{out}"
    );
  }

  // ── v3 differences ────────────────────────────────────────────────────────

  #[test]
  fn v3_anniversary_becomes_x_anniversary() {
    let ann =
      FactValue::Anniversary(NaiveDate::from_ymd_opt(2020, 6, 15).unwrap());
    let out = serialize_v3(&make_view(vec![ann])).unwrap();
    assert!(out.contains("X-ANNIVERSARY:20200615\r\n"), "got:\n{out}");
    assert!(
      !out.contains("\r\nANNIVERSARY:"),
      "bare ANNIVERSARY present in v3:\n{out}"
    );
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
    // calcard writes consecutive TYPE params as comma-separated
    assert!(
      out.contains("EMAIL;TYPE=WORK,PREF:a@b.com\r\n"),
      "got:\n{out}"
    );
  }

  #[test]
  fn v3_gender_omitted() {
    let g = FactValue::Gender("M".to_string());
    let out = serialize_v3(&make_view(vec![g])).unwrap();
    assert!(!out.contains("GENDER:"), "unexpected GENDER in v3:\n{out}");
  }
}
