//! Fact types — the fundamental unit of the Kith contact store.
//!
//! A fact is an immutable claim about a subject at a point in time. Facts are
//! never updated; lifecycle events (supersession, retraction) are recorded in
//! separate append-only tables.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Result;

// ─── Temporal ────────────────────────────────────────────────────────────────

/// When a fact is (or was) true in the real world — distinct from when it was
/// recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EffectiveDate {
  /// A specific moment in time.
  Instant(DateTime<Utc>),
  /// A calendar date without time component (e.g. a birthday, a hire date).
  DateOnly(NaiveDate),
  /// The fact is known to have been true at some point but the date is
  /// unknown.
  Unknown,
}

// ─── Provenance ──────────────────────────────────────────────────────────────

/// How certain the author is about this fact.
#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
  #[default]
  Certain,
  Probable,
  Rumored,
}

/// How and where this fact entered the store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordingContext {
  /// Typed in by the user directly.
  #[default]
  Manual,
  /// Ingested from an external system (vCard import, CardDAV PUT, etc.).
  Imported {
    /// Human-readable name for the source (e.g. "Google Contacts 2024-01").
    source_name:  String,
    /// The UID from the originating vCard, if available.
    original_uid: Option<String>,
  },
}

// ─── Labels ──────────────────────────────────────────────────────────────────

/// Common label for a contact method; mirrors the vCard `TYPE` parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContactLabel {
  Work,
  Home,
  Other,
  Custom(String),
}

// ─── Identity sub-types ──────────────────────────────────────────────────────

/// A structured name (maps to vCard `N` and `FN`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameValue {
  pub given:      Option<String>,
  pub family:     Option<String>,
  pub additional: Option<String>,
  pub prefix:     Option<String>,
  pub suffix:     Option<String>,
  /// Computed or author-overridden display name (vCard `FN`).
  pub full:       String,
}

/// An alternative or former name (vCard `NICKNAME` or `X-ALIAS`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasValue {
  pub name:    String,
  /// Clarifying context, e.g. "maiden name" or "stage name".
  pub context: Option<String>,
}

/// A profile photo stored on disk; no binary data lives in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoValue {
  /// Path relative to the configured `photo_dir`.
  pub path:         String,
  /// SHA-256 hex digest; used for deduplication and ETag generation.
  pub content_hash: String,
  pub media_type:   String,
}

// ─── Contact-method sub-types ────────────────────────────────────────────────

/// An email address (maps to vCard `EMAIL`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailValue {
  pub address:    String,
  pub label:      ContactLabel,
  /// Preference rank — 1 is most preferred; mirrors vCard `PREF` parameter.
  pub preference: u8,
}

/// The mode of a telephone number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PhoneKind {
  Voice,
  Fax,
  Cell,
  Pager,
  Text,
  Video,
  Other,
}

/// A telephone number (maps to vCard `TEL`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneValue {
  pub number:     String,
  pub label:      ContactLabel,
  pub kind:       PhoneKind,
  pub preference: u8,
}

/// A postal address (maps to vCard `ADR`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressValue {
  pub label:       ContactLabel,
  pub street:      Option<String>,
  /// City or locality.
  pub locality:    Option<String>,
  /// State, province, or region.
  pub region:      Option<String>,
  pub postal_code: Option<String>,
  pub country:     Option<String>,
}

/// The semantic context for a URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UrlContext {
  Homepage,
  LinkedIn,
  GitHub,
  Mastodon,
  Custom(String),
}

/// A URL (maps to vCard `URL`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlValue {
  pub url:     String,
  pub context: UrlContext,
}

/// An instant-messaging handle (maps to vCard `IMPP`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImValue {
  pub handle:  String,
  /// Free-text service name, e.g. "Signal", "Matrix", "XMPP".
  pub service: String,
}

/// A social-media handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialValue {
  pub handle:   String,
  pub platform: String,
}

// ─── Relationship sub-types ──────────────────────────────────────────────────

/// A named directional relationship between two subjects (or a free-text name
/// if the other party is not in the store).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipValue {
  /// Human-readable relation label, e.g. "sister", "manager".
  pub relation:   String,
  /// If the other party is also a subject, their UUID.
  pub other_id:   Option<Uuid>,
  /// Free-text name when the other party is not in the store.
  pub other_name: Option<String>,
}

/// Membership in an organisation, optionally with a title and date range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMembershipValue {
  pub org_name: String,
  /// If the organisation is also a subject, its UUID.
  pub org_id:   Option<Uuid>,
  pub title:    Option<String>,
  pub role:     Option<String>,
}

/// Membership in a user-defined group or list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMembershipValue {
  pub group_name: String,
  /// If the group is also a subject, its UUID.
  pub group_id:   Option<Uuid>,
}

// ─── Contextual sub-types ────────────────────────────────────────────────────

/// A logged interaction with the subject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingValue {
  pub summary:  String,
  pub location: Option<String>,
}

// ─── FactValue ───────────────────────────────────────────────────────────────

/// The typed payload of a fact. The variant name serves as the `fact_type`
/// discriminant stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FactValue {
  // ── Identity ────────────────────────────────────────────────────────────
  Name(NameValue),
  Alias(AliasValue),
  Photo(PhotoValue),
  Birthday(NaiveDate),
  Anniversary(NaiveDate),
  Gender(String),

  // ── Contact methods ─────────────────────────────────────────────────────
  Email(EmailValue),
  Phone(PhoneValue),
  Address(AddressValue),
  Url(UrlValue),
  Im(ImValue),
  Social(SocialValue),

  // ── Relationships ────────────────────────────────────────────────────────
  Relationship(RelationshipValue),
  OrgMembership(OrgMembershipValue),
  GroupMembership(GroupMembershipValue),

  // ── Context ──────────────────────────────────────────────────────────────
  Note(String),
  Meeting(MeetingValue),
  Introduction(String),

  /// Escape hatch for facts that don't fit the taxonomy.
  Custom {
    key:   String,
    value: serde_json::Value,
  },
}

impl FactValue {
  /// The discriminant string stored in the `fact_type` column.
  /// Must match the `rename_all = "snake_case"` serde tags above.
  pub fn discriminant(&self) -> &'static str {
    match self {
      Self::Name(_) => "name",
      Self::Alias(_) => "alias",
      Self::Photo(_) => "photo",
      Self::Birthday(_) => "birthday",
      Self::Anniversary(_) => "anniversary",
      Self::Gender(_) => "gender",
      Self::Email(_) => "email",
      Self::Phone(_) => "phone",
      Self::Address(_) => "address",
      Self::Url(_) => "url",
      Self::Im(_) => "im",
      Self::Social(_) => "social",
      Self::Relationship(_) => "relationship",
      Self::OrgMembership(_) => "org_membership",
      Self::GroupMembership(_) => "group_membership",
      Self::Note(_) => "note",
      Self::Meeting(_) => "meeting",
      Self::Introduction(_) => "introduction",
      Self::Custom { .. } => "custom",
    }
  }

  /// Serialise the inner payload (without the type tag) for the `value_json`
  /// database column.
  pub fn to_json(&self) -> Result<serde_json::Value> {
    // The full serialised form is `{"type": "...", "data": <payload>}`.
    // We want only the payload.
    let full = serde_json::to_value(self)?;
    Ok(full.get("data").cloned().unwrap_or(serde_json::Value::Null))
  }

  /// Deserialise from the discriminant string and JSON payload stored in the
  /// database.
  pub fn from_parts(
    discriminant: &str,
    data: serde_json::Value,
  ) -> Result<Self> {
    let wrapped = serde_json::json!({ "type": discriminant, "data": data });
    Ok(serde_json::from_value(wrapped)?)
  }
}

// ─── Fact ────────────────────────────────────────────────────────────────────

/// An immutable claim about a subject. Once written, no field is ever updated.
/// Lifecycle events live in separate tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
  pub fact_id:           Uuid,
  pub subject_id:        Uuid,
  pub value:             FactValue,
  /// Server-assigned timestamp; never changes after creation.
  pub recorded_at:       DateTime<Utc>,
  pub effective_at:      Option<EffectiveDate>,
  pub effective_until:   Option<EffectiveDate>,
  pub source:            Option<String>,
  pub confidence:        Confidence,
  pub recording_context: RecordingContext,
  pub tags:              Vec<String>,
}

// ─── NewFact ─────────────────────────────────────────────────────────────────

/// Input to [`crate::store::ContactStore::record_fact`].
/// `recorded_at` is always set by the store; it is not accepted from callers.
#[derive(Debug, Clone)]
pub struct NewFact {
  pub subject_id:        Uuid,
  pub value:             FactValue,
  pub effective_at:      Option<EffectiveDate>,
  pub effective_until:   Option<EffectiveDate>,
  pub source:            Option<String>,
  pub confidence:        Confidence,
  pub recording_context: RecordingContext,
  pub tags:              Vec<String>,
}

impl NewFact {
  /// Convenience constructor with all optional fields set to their defaults.
  pub fn new(subject_id: Uuid, value: FactValue) -> Self {
    Self {
      subject_id,
      value,
      effective_at: None,
      effective_until: None,
      source: None,
      confidence: Confidence::default(),
      recording_context: RecordingContext::default(),
      tags: Vec::new(),
    }
  }
}
