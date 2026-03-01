//! vCard 3.0 / 4.0 content-line parser.
//!
//! Pipeline:
//!   raw &str
//!     └─ unfold_lines()        → Vec<String>
//!          └─ parse_content_line() → ContentLine
//!               └─ map_property()  → accumulate facts
//!                    └─ flush accumulators → Vec<NewFact>

use chrono::NaiveDate;
use kith_core::fact::{
  AddressValue, AliasValue, ContactLabel, EmailValue, FactValue,
  GroupMembershipValue, ImValue, MeetingValue, NameValue, NewFact,
  OrgMembershipValue, PhoneKind, PhoneValue, RecordingContext,
  RelationshipValue, SocialValue, UrlContext, UrlValue,
};
use uuid::Uuid;

use crate::{
  ParsedVcard,
  error::{Error, Result},
};

// ─── Content-line representation ─────────────────────────────────────────────

struct ContentLine {
  name:   String,
  params: Vec<Param>,
  value:  String,
}

struct Param {
  name:  String,
  value: String,
}

// ─── Low-level helpers
// ────────────────────────────────────────────────────────

/// Join CRLF+SP (or LF+SP / LF+HT) continuation lines (RFC 6350 §3.2).
/// Tolerates bare LF line endings for real-world robustness.
pub(crate) fn unfold_lines(s: &str) -> Vec<String> {
  let mut lines: Vec<String> = Vec::new();
  for raw in s.split('\n') {
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    if line.starts_with(' ') || line.starts_with('\t') {
      if let Some(last) = lines.last_mut() {
        last.push_str(&line[1..]);
      }
      // else: leading continuation with no prior line — discard
    } else {
      lines.push(line.to_string());
    }
  }
  lines.retain(|l| !l.is_empty());
  lines
}

/// Find the first `:` that is not inside double-quoted string.
fn find_unquoted_colon(s: &str) -> Option<usize> {
  let mut in_quotes = false;
  for (i, c) in s.char_indices() {
    match c {
      '"' => in_quotes = !in_quotes,
      ':' if !in_quotes => return Some(i),
      _ => {}
    }
  }
  None
}

/// Split on `;` while respecting double-quoted strings.
fn split_semicolons_respecting_quotes(s: &str) -> Vec<&str> {
  let mut result = Vec::new();
  let mut start = 0usize;
  let mut in_quotes = false;
  for (i, c) in s.char_indices() {
    match c {
      '"' => in_quotes = !in_quotes,
      ';' if !in_quotes => {
        result.push(&s[start..i]);
        start = i + 1;
      }
      _ => {}
    }
  }
  result.push(&s[start..]);
  result
}

/// Collect all TYPE= values, handling `TYPE=A,B` and multiple `TYPE=` params.
fn type_values(params: &[Param]) -> Vec<String> {
  let mut types = Vec::new();
  for p in params {
    if p.name.eq_ignore_ascii_case("TYPE") {
      for t in p.value.split(',') {
        let t = t.trim().to_uppercase();
        if !t.is_empty() {
          types.push(t);
        }
      }
    }
  }
  types
}

/// Return a preference value (1..=255).
/// vCard 4.0: `PREF=N` param; vCard 3.0: `TYPE=PREF`.
fn pref_from_params(params: &[Param], types: &[String]) -> u8 {
  // v4 PREF=N
  for p in params {
    if p.name.eq_ignore_ascii_case("PREF")
      && let Ok(n) = p.value.parse::<u8>()
    {
      return n;
    }
  }
  // v3 TYPE=PREF
  if types.iter().any(|t| t == "PREF") {
    return 1;
  }
  255
}

/// Map TYPE values to a ContactLabel.
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

/// Parse vCard date formats `YYYYMMDD` and `YYYY-MM-DD`.
/// Returns `Err` for year-omitted `--MMDD` (caller silently skips).
fn parse_vcard_date(property: &str, value: &str) -> Result<NaiveDate> {
  if value.starts_with("--") {
    return Err(Error::InvalidDate {
      property: property.to_string(),
      value:    value.to_string(),
    });
  }
  if let Ok(d) = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d") {
    return Ok(d);
  }
  if let Ok(d) = NaiveDate::parse_from_str(value.trim(), "%Y%m%d") {
    return Ok(d);
  }
  Err(Error::InvalidDate {
    property: property.to_string(),
    value:    value.to_string(),
  })
}

/// Minimal quoted-printable decoder for vCard 3.0 `ENCODING=QUOTED-PRINTABLE`.
fn decode_quoted_printable(s: &str) -> String {
  let bytes = s.as_bytes();
  let mut result: Vec<u8> = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'=' && i + 2 < bytes.len() {
      let hi = bytes[i + 1];
      let lo = bytes[i + 2];
      if hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit() {
        let hi = (hi as char).to_digit(16).unwrap() as u8;
        let lo = (lo as char).to_digit(16).unwrap() as u8;
        result.push((hi << 4) | lo);
        i += 3;
        continue;
      }
    }
    result.push(bytes[i]);
    i += 1;
  }
  String::from_utf8_lossy(&result).into_owned()
}

// ─── Content-line parser
// ──────────────────────────────────────────────────────

fn parse_content_line(line: &str) -> Result<ContentLine> {
  let colon_pos = find_unquoted_colon(line)
    .ok_or_else(|| Error::MalformedContentLine(line.to_string()))?;

  let name_part = &line[..colon_pos];
  let value = line[colon_pos + 1..].to_string();

  let tokens = split_semicolons_respecting_quotes(name_part);
  if tokens.is_empty() {
    return Err(Error::MalformedContentLine(line.to_string()));
  }

  // Strip group prefix (e.g. "ORG1.ORG" → "ORG")
  let name_raw = tokens[0];
  let name = if let Some(dot_pos) = name_raw.find('.') {
    name_raw[dot_pos + 1..].to_uppercase()
  } else {
    name_raw.to_uppercase()
  };

  let mut params = Vec::new();
  for token in &tokens[1..] {
    if let Some(eq_pos) = token.find('=') {
      let param_name = token[..eq_pos].trim().to_uppercase();
      let param_val = token[eq_pos + 1..].trim().trim_matches('"').to_string();
      params.push(Param {
        name:  param_name,
        value: param_val,
      });
    } else {
      // Bare token — treat as TYPE=value (vCard 3.0 compat)
      let t = token.trim();
      if !t.is_empty() {
        params.push(Param {
          name:  "TYPE".to_string(),
          value: t.to_uppercase(),
        });
      }
    }
  }

  Ok(ContentLine {
    name,
    params,
    value,
  })
}

// ─── Accumulators
// ─────────────────────────────────────────────────────────────

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
      if parts.is_empty() {
        None
      } else {
        Some(parts.join(" "))
      }
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

// ─── Value helpers
// ────────────────────────────────────────────────────────────

fn unescape_value(s: &str) -> String {
  let mut result = String::with_capacity(s.len());
  let mut chars = s.chars().peekable();
  while let Some(c) = chars.next() {
    if c == '\\' {
      match chars.next() {
        Some('n') | Some('N') => result.push('\n'),
        Some('\\') => result.push('\\'),
        Some(',') => result.push(','),
        Some(';') => result.push(';'),
        Some(other) => {
          result.push('\\');
          result.push(other);
        }
        None => result.push('\\'),
      }
    } else {
      result.push(c);
    }
  }
  result
}

/// Return `Some(trimmed)` when non-empty, `None` otherwise.
fn opt_str(s: &str) -> Option<String> {
  let s = s.trim();
  if s.is_empty() {
    None
  } else {
    Some(s.to_string())
  }
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

/// Parse a single vCard from `input`.
///
/// All returned [`NewFact`]s have `subject_id = Uuid::nil()`; the caller must
/// replace this with the real subject UUID before persisting.
pub fn parse_one(input: &str, source_name: &str) -> Result<ParsedVcard> {
  let lines = unfold_lines(input);

  let start = lines
    .iter()
    .position(|l| l.eq_ignore_ascii_case("BEGIN:VCARD"))
    .ok_or(Error::MissingEnvelope)?;
  let end = lines
    .iter()
    .rposition(|l| l.eq_ignore_ascii_case("END:VCARD"))
    .ok_or(Error::MissingEnvelope)?;
  if end <= start {
    return Err(Error::MissingEnvelope);
  }

  let mut uid: Option<String> = None;
  let mut name_accum = NameAccum::default();
  let mut org_groups: Vec<OrgGroup> = Vec::new();
  let mut facts: Vec<FactValue> = Vec::new();

  for line in &lines[start + 1..end] {
    let cl = match parse_content_line(line) {
      Ok(cl) => cl,
      Err(_) => continue, // skip malformed lines
    };

    // Apply ENCODING=QUOTED-PRINTABLE if present
    let value = {
      let is_qp = cl.params.iter().any(|p| {
        p.name.eq_ignore_ascii_case("ENCODING")
          && p.value.eq_ignore_ascii_case("QUOTED-PRINTABLE")
      });
      if is_qp {
        decode_quoted_printable(&cl.value)
      } else {
        cl.value.clone()
      }
    };

    let types = type_values(&cl.params);
    let pref = pref_from_params(&cl.params, &types);
    let label = label_from_types(&types);

    match cl.name.as_str() {
      // ── Skip envelope / meta ──────────────────────────────────────────────
      "VERSION" | "PRODID" | "REV" | "KIND" | "CATEGORIES" => {}

      "UID" => uid = opt_str(&value),

      // ── Name ─────────────────────────────────────────────────────────────
      "FN" => {
        let v = unescape_value(&value);
        if !v.is_empty() {
          name_accum.full = Some(v);
        }
      }
      "N" => {
        // family;given;additional;prefix;suffix
        let parts: Vec<&str> = value.split(';').collect();
        name_accum.family = parts
          .first()
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        name_accum.given = parts
          .get(1)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        name_accum.additional = parts
          .get(2)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        name_accum.prefix = parts
          .get(3)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        name_accum.suffix = parts
          .get(4)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
      }
      "NICKNAME" => {
        for token in value.split(',') {
          let name = unescape_value(token.trim());
          if !name.is_empty() {
            facts.push(FactValue::Alias(AliasValue {
              name,
              context: None,
            }));
          }
        }
      }

      // ── Contact methods ───────────────────────────────────────────────────
      "TEL" => {
        let number = unescape_value(value.trim());
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
        } else if types.contains(&"TEXT".to_string()) {
          PhoneKind::Text
        } else if types.contains(&"VIDEO".to_string()) {
          PhoneKind::Video
        } else {
          PhoneKind::Voice
        };
        facts.push(FactValue::Phone(PhoneValue {
          number,
          label,
          kind,
          preference: pref,
        }));
      }
      "EMAIL" => {
        let address = unescape_value(value.trim());
        if address.is_empty() {
          continue;
        }
        facts.push(FactValue::Email(EmailValue {
          address,
          label,
          preference: pref,
        }));
      }
      "ADR" => {
        // pobox;ext;street;city;region;postal;country
        let parts: Vec<&str> = value.split(';').collect();
        // fields 0 (pobox) and 1 (ext) are discarded per the spec mapping
        let street = parts
          .get(2)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        let locality = parts
          .get(3)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        let region = parts
          .get(4)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        let postal_code = parts
          .get(5)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        let country = parts
          .get(6)
          .and_then(|s| opt_str(s))
          .map(|s| unescape_value(&s));
        facts.push(FactValue::Address(AddressValue {
          label,
          street,
          locality,
          region,
          postal_code,
          country,
        }));
      }
      "URL" => {
        let url = value.trim().to_string();
        if url.is_empty() {
          continue;
        }
        let context = if types
          .iter()
          .any(|t| t.eq_ignore_ascii_case("LINKEDIN"))
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
      "BDAY" => {
        match parse_vcard_date("BDAY", &value) {
          Ok(d) => facts.push(FactValue::Birthday(d)),
          Err(Error::InvalidDate { ref value, .. })
            if value.starts_with("--") => {} // year-omitted
          Err(_) => {} // other parse errors skipped silently
        }
      }
      "ANNIVERSARY" => match parse_vcard_date("ANNIVERSARY", &value) {
        Ok(d) => facts.push(FactValue::Anniversary(d)),
        Err(Error::InvalidDate { ref value, .. })
          if value.starts_with("--") => {}
        Err(_) => {}
      },

      // ── Demographics ──────────────────────────────────────────────────────
      "GENDER" => {
        let gender = value.split(';').next().unwrap_or("").trim().to_string();
        if !gender.is_empty() {
          facts.push(FactValue::Gender(gender));
        }
      }

      // ── Org / role ────────────────────────────────────────────────────────
      "ORG" => {
        let org_name =
          unescape_value(value.split(';').next().unwrap_or(&value).trim());
        if !org_name.is_empty() {
          org_groups.push(OrgGroup {
            org_name,
            title: None,
            role: None,
          });
        }
      }
      "TITLE" => {
        let title = unescape_value(value.trim());
        if !title.is_empty() {
          if let Some(last) = org_groups.last_mut() {
            last.title = Some(title);
          } else {
            org_groups.push(OrgGroup {
              org_name: String::new(),
              title:    Some(title),
              role:     None,
            });
          }
        }
      }
      "ROLE" => {
        let role = unescape_value(value.trim());
        if !role.is_empty() {
          if let Some(last) = org_groups.last_mut() {
            last.role = Some(role);
          } else {
            org_groups.push(OrgGroup {
              org_name: String::new(),
              title:    None,
              role:     Some(role),
            });
          }
        }
      }

      // ── Misc ─────────────────────────────────────────────────────────────
      "NOTE" => {
        let note = unescape_value(&value);
        if !note.is_empty() {
          facts.push(FactValue::Note(note));
        }
      }
      "PHOTO" => {
        let is_base64 = cl.params.iter().any(|p| {
          p.name.eq_ignore_ascii_case("ENCODING")
            && (p.value.eq_ignore_ascii_case("BASE64")
              || p.value.eq_ignore_ascii_case("b"))
        });
        if !is_base64
          && (value.starts_with("http")
            || value.starts_with("file://")
            || value.starts_with("cid:"))
        {
          let uri = value.trim().to_string();
          if !uri.is_empty() {
            facts.push(FactValue::Custom {
              key:   "photo_uri".to_string(),
              value: serde_json::Value::String(uri),
            });
          }
        }
        // base64 photos silently dropped
      }

      // ── IM ────────────────────────────────────────────────────────────────
      "IMPP" => {
        if let Some(colon) = value.find(':') {
          let scheme = &value[..colon];
          let handle = value[colon + 1..].to_string();
          let service = scheme_to_service(scheme);
          facts.push(FactValue::Im(ImValue { handle, service }));
        } else {
          return Err(Error::InvalidImppUri(value.clone()));
        }
      }

      // ── vCard 3.0 legacy IM X-props ───────────────────────────────────────
      "X-AIM" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "AIM".to_string(),
      })),
      "X-JABBER" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "XMPP".to_string(),
      })),
      "X-SKYPE" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "Skype".to_string(),
      })),
      "X-SKYPE-USERNAME" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "Skype".to_string(),
      })),
      "X-ICQ" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "ICQ".to_string(),
      })),
      "X-MSN" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "MSN".to_string(),
      })),
      "X-YAHOO" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "Yahoo".to_string(),
      })),
      "X-GOOGLE-TALK" => facts.push(FactValue::Im(ImValue {
        handle:  value.trim().to_string(),
        service: "Google Talk".to_string(),
      })),

      // ── Kith-specific X-props ─────────────────────────────────────────────
      "X-KITH-SOCIAL" => {
        let platform = cl
          .params
          .iter()
          .find(|p| p.name.eq_ignore_ascii_case("PLATFORM"))
          .map(|p| p.value.clone())
          .unwrap_or_default();
        let handle = unescape_value(value.trim());
        if !platform.is_empty() && !handle.is_empty() {
          facts.push(FactValue::Social(SocialValue { handle, platform }));
        }
      }
      "X-KITH-GROUP" => {
        let group_id = cl
          .params
          .iter()
          .find(|p| p.name.eq_ignore_ascii_case("GROUP-ID"))
          .and_then(|p| Uuid::parse_str(&p.value).ok());
        let group_name = unescape_value(value.trim());
        facts.push(FactValue::GroupMembership(GroupMembershipValue {
          group_name,
          group_id,
        }));
      }
      "X-KITH-RELATION" => {
        let relation = cl
          .params
          .iter()
          .find(|p| p.name.eq_ignore_ascii_case("RELATION"))
          .map(|p| p.value.clone())
          .unwrap_or_default();
        let other_id = cl
          .params
          .iter()
          .find(|p| p.name.eq_ignore_ascii_case("OTHER-ID"))
          .and_then(|p| Uuid::parse_str(&p.value).ok());
        let other_name = opt_str(value.trim());
        facts.push(FactValue::Relationship(RelationshipValue {
          relation,
          other_id,
          other_name,
        }));
      }
      "X-KITH-MEETING" => {
        let location = cl
          .params
          .iter()
          .find(|p| p.name.eq_ignore_ascii_case("LOCATION"))
          .and_then(|p| opt_str(&p.value));
        let summary = unescape_value(value.trim());
        facts.push(FactValue::Meeting(MeetingValue { summary, location }));
      }
      "X-KITH-INTRODUCTION" => {
        let intro = unescape_value(value.trim());
        if !intro.is_empty() {
          facts.push(FactValue::Introduction(intro));
        }
      }

      // ── Other X-props → Custom ────────────────────────────────────────────
      other if other.starts_with("X-") => {
        let val = serde_json::Value::String(unescape_value(&value));
        facts.push(FactValue::Custom {
          key:   other.to_string(),
          value: val,
        });
      }

      // ── Unknown IANA properties silently skipped ──────────────────────────
      _ => {}
    }
  }

  // ── Flush accumulators
  // ───────────────────────────────────────────────────────
  let mut final_facts: Vec<FactValue> = Vec::new();

  if let Some(name_fv) = name_accum.flush() {
    final_facts.push(name_fv);
  }

  for g in org_groups {
    let org_name = if g.org_name.is_empty() {
      "(unknown)".to_string()
    } else {
      g.org_name
    };
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

  Ok(ParsedVcard {
    uid,
    facts: new_facts,
  })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use kith_core::fact::{FactValue, RecordingContext};

  use super::*;

  fn first_fact(card: &ParsedVcard) -> &FactValue { &card.facts[0].value }

  // ── Envelope
  // ────────────────────────────────────────────────────────────────

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

  // ── FN-only → single Name fact
  // ───────────────────────────────────────────────

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

  // ── N + FN → merged single Name fact
  // ────────────────────────────────────────

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

  // ── TEL v4 PREF
  // ─────────────────────────────────────────────────────────────

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

  // ── TEL v3 TYPE=PREF
  // ────────────────────────────────────────────────────────

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

  // ── EMAIL preference roundtrip
  // ────────────────────────────────────────────────

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

  // ── ADR 7-field split
  // ────────────────────────────────────────────────────────

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

  // ── BDAY ────────────────────────────────────────────────────────────────────

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
    // --MMDD should be silently skipped; no Birthday fact produced
    assert!(
      !card
        .facts
        .iter()
        .any(|f| matches!(f.value, FactValue::Birthday(_)))
    );
  }

  // ── ORG + TITLE + ROLE
  // ───────────────────────────────────────────────────────

  #[test]
  fn org_title_role_single_membership() {
    let input = "BEGIN:VCARD\r\nVERSION:4.0\r\nORG:Acme \
                 Corp\r\nTITLE:Engineer\r\nROLE:IC\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let orgs: Vec<_> = card
      .facts
      .iter()
      .filter_map(|f| {
        if let FactValue::OrgMembership(o) = &f.value {
          Some(o)
        } else {
          None
        }
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
        if let FactValue::OrgMembership(o) = &f.value {
          Some(o)
        } else {
          None
        }
      })
      .collect();
    assert_eq!(orgs.len(), 2);
    assert_eq!(orgs[0].org_name, "Acme");
    assert_eq!(orgs[1].org_name, "OSF");
  }

  // ── IMPP ────────────────────────────────────────────────────────────────────

  #[test]
  fn impp_xmpp_uri() {
    #[rustfmt::skip]
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

  // ── Kith X-props
  // ────────────────────────────────────────────────────────────

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

  // ── Folded lines
  // ─────────────────────────────────────────────────────────────

  #[test]
  fn folded_lines_unfolded_correctly() {
    let input =
      "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\n  Smith\r\nEND:VCARD\r\n";
    let card = parse_one(input, "test").unwrap();
    let FactValue::Name(n) = first_fact(&card) else {
      panic!()
    };
    assert_eq!(n.full, "Alice Smith");
  }

  // ── RecordingContext
  // ─────────────────────────────────────────────────────────

  #[test]
  fn recording_context_set_correctly() {
    let uid = "uid-abc-123";
    let input = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Alice\r\nEND:VCARD\r\n"
    );
    let card = parse_one(&input, "MyImport").unwrap();
    assert_eq!(card.uid, Some(uid.to_string()));
    for f in &card.facts {
      let RecordingContext::Imported {
        source_name,
        original_uid,
      } = &f.recording_context
      else {
        panic!("expected Imported context");
      };
      assert_eq!(source_name, "MyImport");
      assert_eq!(original_uid, &Some(uid.to_string()));
    }
  }

  // ── parse_many
  // ───────────────────────────────────────────────────────────────

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
    let FactValue::Name(n0) = &results[0].as_ref().unwrap().facts[0].value
    else {
      panic!()
    };
    let FactValue::Name(n1) = &results[1].as_ref().unwrap().facts[0].value
    else {
      panic!()
    };
    assert_eq!(n0.full, "Alice");
    assert_eq!(n1.full, "Bob");
  }
}
