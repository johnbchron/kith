# Kith: Planning Document

## Vision

A Rust-based personal contact store that treats contact information as a stream of timestamped facts rather than a single mutable record. Each piece of information about a person — a phone number, an email, a relationship, a note — is its own document, recorded with both a *recording date* (when you wrote it down) and an *effective date* (when it became or was true in the real world). The system speaks CardDAV so it integrates naturally with standard calendar and contact clients.

---

## Core Concepts

### The Fact Document

The fundamental unit of the store is not a "contact record" but a **fact** — a single claim about a person at a point in time. Facts are immutable once written. Changing someone's phone number doesn't overwrite the old one; it adds a new fact with a newer effective date.

Every fact carries:

- `fact_id` — a UUID identifying this specific fact document
- `subject_id` — the UUID of the person this fact is about
- `value` — a typed `FactValue` enum variant; the variant itself encodes the fact type
- `recorded_at` — a UTC timestamp of when this fact was written into the store (set by the server, immutable)
- `effective_at` — the date/time when this fact became or was true in the world (may be a past date, a date range, or null for timeless facts like a birthdate)
- `effective_until` — optional end of validity; null means "still true as far as we know"
- `source` — free-text or structured reference to where this information came from
- `confidence` — a `Confidence` enum: `Certain | Probable | Rumored`
- `recording_context` — a `RecordingContext` enum distinguishing manually entered facts from imported ones
- `tags` — arbitrary user-defined labels

Facts are **truly immutable** — no field on a stored fact row is ever updated. Lifecycle events (supersession and retraction) are recorded as separate rows in their own tables. A fact's current status is a computed property, determined at query time by joining against those tables.

### Fact Taxonomy

Facts are organized into typed categories. The type determines how `value` is parsed and how CardDAV mapping works.

**Identity facts**
- `name` — full name, with subfields for given/family/prefix/suffix/nickname
- `alias` — an alternative name or former name
- `photo` — a profile image
- `birthday`
- `anniversary`
- `gender`

**Contact method facts**
- `email` — address plus label (work, personal, etc.) and preference rank
- `phone` — number, type, and label
- `address` — structured postal address with label
- `url` — website or profile URL, with context (homepage, LinkedIn, GitHub, etc.)
- `im` — instant messaging handle and service name
- `social` — social media handle and platform

**Relationship facts**
- `relationship` — a named directional relationship between two subjects in the store (e.g., `{from: alice, relation: "sister", to: bob}`)
- `organization_membership` — the subject's role at an org, with start/end dates
- `group_membership` — membership in a user-defined group/list

**Contextual facts**
- `note` — free-text observation
- `meeting` — a logged interaction with the person
- `introduction` — how you met
- `custom` — an arbitrary key/value pair with a user-defined schema

### The Subject Record

A **subject** is a thin envelope that aggregates facts. It holds only:

- `subject_id` — UUID
- `created_at` — when this subject was first added to the store
- `kind` — `person | organization | group`

The "current view" of a contact is computed on read by collecting all non-superseded facts for a subject that are effective as of a requested point in time. This is the event-sourcing read model.

---

## Data Model (Rust Types)

### Facts

```rust
/// An immutable claim about a subject. Once written, no field changes.
/// Lifecycle (supersession, retraction) lives in separate tables.
pub struct Fact {
    pub fact_id:            Uuid,
    pub subject_id:         Uuid,
    pub value:              FactValue,       // variant encodes the fact type
    pub recorded_at:        DateTime<Utc>,   // server-assigned, never updated
    pub effective_at:       Option<EffectiveDate>,
    pub effective_until:    Option<EffectiveDate>,
    pub source:             Option<String>,
    pub confidence:         Confidence,
    pub recording_context:  RecordingContext,
    pub tags:               Vec<String>,
}

/// Where and how this fact entered the store.
pub enum RecordingContext {
    /// Typed in by the user directly.
    Manual,
    /// Ingested from an external source (e.g. a .vcf file or CardDAV PUT).
    Imported {
        source_name:  String,           // e.g. "Google Contacts export 2024-01"
        original_uid: Option<String>,   // UID from the originating vCard, if any
    },
}

/// When a fact is true in the real world (distinct from when it was recorded).
pub enum EffectiveDate {
    /// A specific moment in time.
    Instant(DateTime<Utc>),
    /// A calendar date without time (e.g. a birthday, a start-of-employment date).
    DateOnly(NaiveDate),
    /// The fact is known to have been true at some point but the date is not known.
    Unknown,
}

pub enum Confidence {
    Certain,
    Probable,
    Rumored,
}
```

### Fact Values

Rather than `serde_json::Value` with a parallel `fact_type: String`, the value and its type are unified into a single enum. The DB still stores a discriminant string and a JSON payload for indexing, but the application layer always works with typed variants.

```rust
pub enum FactValue {
    // ── Identity ─────────────────────────────────────────────────────────
    Name(NameValue),
    Alias(AliasValue),
    Photo(PhotoValue),
    Birthday(NaiveDate),
    Anniversary(NaiveDate),
    Gender(String),

    // ── Contact methods ───────────────────────────────────────────────────
    Email(EmailValue),
    Phone(PhoneValue),
    Address(AddressValue),
    Url(UrlValue),
    Im(ImValue),

    // ── Relationships ─────────────────────────────────────────────────────
    Relationship(RelationshipValue),
    OrgMembership(OrgMembershipValue),

    // ── Context ───────────────────────────────────────────────────────────
    Note(String),
    Meeting(MeetingValue),
    Introduction(String),

    /// Escape hatch for facts that don't fit the taxonomy.
    Custom { key: String, value: serde_json::Value },
}

pub struct NameValue {
    pub given:      Option<String>,
    pub family:     Option<String>,
    pub additional: Option<String>,
    pub prefix:     Option<String>,
    pub suffix:     Option<String>,
    pub full:       String,   // FN in vCard; computed or overridden
}

pub struct EmailValue {
    pub address:    String,
    pub label:      ContactLabel,
    pub preference: u8,   // 1 = most preferred; mirrors vCard PREF param
}

pub struct PhoneValue {
    pub number:     String,
    pub label:      ContactLabel,
    pub kind:       PhoneKind,
    pub preference: u8,
}

pub enum PhoneKind { Voice, Fax, Cell, Pager, Text, Video, Other }

/// Common label for contact methods (mirrors vCard TYPE param).
pub enum ContactLabel {
    Work,
    Home,
    Other,
    Custom(String),
}

pub struct AddressValue {
    pub label:       ContactLabel,
    pub street:      Option<String>,
    pub locality:    Option<String>,   // city
    pub region:      Option<String>,   // state/province
    pub postal_code: Option<String>,
    pub country:     Option<String>,
}

pub struct UrlValue {
    pub url:     String,
    pub context: UrlContext,
}

pub enum UrlContext {
    Homepage, LinkedIn, GitHub, Mastodon, Custom(String),
}

pub struct ImValue {
    pub handle:  String,
    pub service: String,   // "Signal", "Matrix", etc.
}

pub struct RelationshipValue {
    pub relation:   String,     // "sister", "manager", "introduced by"
    pub other_id:   Option<Uuid>,  // if the other party is also a subject
    pub other_name: Option<String>, // free text if not in the store
}

pub struct OrgMembershipValue {
    pub org_name: String,
    pub org_id:   Option<Uuid>,   // if the org is also a subject
    pub title:    Option<String>,
    pub role:     Option<String>,
}

pub struct PhotoValue {
    pub path:        std::path::PathBuf,  // file on disk relative to photo_dir
    pub content_hash: String,             // SHA-256 hex; used for dedup and ETag
    pub media_type:  String,              // "image/jpeg", "image/png"
}

pub struct MeetingValue {
    pub summary: String,
    pub location: Option<String>,
}

pub struct AliasValue {
    pub name:    String,
    pub context: Option<String>,   // e.g. "maiden name", "stage name"
}
```

### Lifecycle Events

Facts never change. Their lifecycle is tracked in two separate, append-only event tables.

```rust
/// A fact that has been replaced by a newer fact with corrected or updated information.
pub struct Supersession {
    pub supersession_id: Uuid,
    pub old_fact_id:     Uuid,
    pub new_fact_id:     Uuid,
    pub recorded_at:     DateTime<Utc>,
}

/// A fact that has been withdrawn entirely, with no replacement.
pub struct Retraction {
    pub retraction_id: Uuid,
    pub fact_id:       Uuid,
    pub reason:        Option<String>,
    pub recorded_at:   DateTime<Utc>,
}
```

The active status of a fact is computed by checking whether its `fact_id` appears in either table:

```rust
/// Computed at query time by joining facts with the lifecycle tables.
pub enum FactStatus {
    Active,
    Superseded { by: Uuid, at: DateTime<Utc> },
    Retracted  { reason: Option<String>, at: DateTime<Utc> },
}

/// A fact bundled with its current lifecycle status.
pub struct ResolvedFact {
    pub fact:   Fact,
    pub status: FactStatus,
}
```

This means `get_facts(..., as_of)` returns `Vec<ResolvedFact>` — the caller can trivially filter to `Active` facts for the current view, or inspect the full history including superseded and retracted entries. The materialized `ContactView` only includes facts whose status is `Active`.

### Subject and View

```rust
pub struct Subject {
    pub subject_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub kind:       SubjectKind,
}

pub enum SubjectKind { Person, Organization, Group }

/// Materialized read model — computed, never stored.
pub struct ContactView {
    pub subject:     Subject,
    pub as_of:       DateTime<Utc>,
    pub active_facts: Vec<ResolvedFact>,   // all Active-status facts as of `as_of`
}
```

---

## Storage Layer

### Backend Choice

**SQLite via `rusqlite`** for the initial implementation. Rationale: single-file, zero-infrastructure, easily embedded, and sufficient for a personal contact store of tens of thousands of facts. The schema is simple enough that migrating to PostgreSQL later is straightforward.

A future `postgres` feature flag should be kept in mind from the start. Abstract the store behind a `ContactStore` trait so the backend is swappable.

### Schema

```sql
CREATE TABLE subjects (
    subject_id  TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL,
    kind        TEXT NOT NULL   -- 'person' | 'organization' | 'group'
);

-- Facts are append-only. No UPDATE or DELETE is ever issued against this table.
CREATE TABLE facts (
    fact_id           TEXT PRIMARY KEY,
    subject_id        TEXT NOT NULL REFERENCES subjects(subject_id),
    fact_type         TEXT NOT NULL,    -- discriminant of FactValue variant, for indexing
    value_json        TEXT NOT NULL,    -- JSON serialization of the typed FactValue
    recorded_at       TEXT NOT NULL,    -- ISO 8601 UTC; server-assigned
    effective_at      TEXT,             -- ISO 8601 or null
    effective_until   TEXT,
    source            TEXT,
    confidence        TEXT NOT NULL DEFAULT 'certain',
    recording_context TEXT NOT NULL DEFAULT 'manual',  -- 'manual' or JSON import metadata
    tags              TEXT NOT NULL DEFAULT '[]'       -- JSON array of strings
);

-- A fact replaced by a newer corrected/updated version.
CREATE TABLE supersessions (
    supersession_id TEXT PRIMARY KEY,
    old_fact_id     TEXT NOT NULL REFERENCES facts(fact_id),
    new_fact_id     TEXT NOT NULL REFERENCES facts(fact_id),
    recorded_at     TEXT NOT NULL,
    UNIQUE (old_fact_id),   -- a fact can only be superseded once
    CHECK  (old_fact_id != new_fact_id)
);

-- A fact withdrawn with no replacement.
CREATE TABLE retractions (
    retraction_id TEXT PRIMARY KEY,
    fact_id       TEXT NOT NULL REFERENCES facts(fact_id),
    reason        TEXT,
    recorded_at   TEXT NOT NULL,
    UNIQUE (fact_id)   -- a fact can only be retracted once
);

-- A fact_id cannot appear in both tables.
-- Enforced at the application layer (the trait returns an error if you try).

-- Photo blobs live on disk; the facts table stores only metadata.
-- Photos are in {photo_dir}/{subject_id}/{content_hash}.{ext}

CREATE INDEX facts_subject_idx  ON facts(subject_id);
CREATE INDEX facts_type_idx     ON facts(fact_type);
CREATE INDEX facts_recorded_idx ON facts(recorded_at);
```

The active-status query becomes a standard anti-join:

```sql
SELECT f.*, NULL as superseded_by, NULL as retracted_reason
FROM facts f
WHERE f.subject_id = ?
  AND f.fact_id NOT IN (SELECT old_fact_id FROM supersessions)
  AND f.fact_id NOT IN (SELECT fact_id      FROM retractions)
  -- point-in-time filter:
  AND f.recorded_at <= ?
```

### Trait Interface

```rust
#[async_trait]
pub trait ContactStore: Send + Sync {
    // Subjects
    async fn add_subject(&self, kind: SubjectKind) -> Result<Subject>;
    async fn get_subject(&self, id: Uuid) -> Result<Option<Subject>>;
    async fn list_subjects(&self, kind: Option<SubjectKind>) -> Result<Vec<Subject>>;

    // Facts — append-only writes
    async fn record_fact(&self, input: NewFact) -> Result<Fact>;

    // Lifecycle events — recorded in their own tables, never mutate facts
    async fn supersede(&self, old_id: Uuid, replacement: NewFact) -> Result<(Supersession, Fact)>;
    async fn retract(&self, fact_id: Uuid, reason: Option<String>) -> Result<Retraction>;

    // Reads
    async fn get_facts(
        &self,
        subject_id: Uuid,
        as_of: Option<DateTime<Utc>>,
        include_inactive: bool,   // if false, only Active; if true, also Superseded/Retracted
    ) -> Result<Vec<ResolvedFact>>;

    async fn materialize(
        &self,
        subject_id: Uuid,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Option<ContactView>>;

    async fn search(&self, query: &FactQuery) -> Result<Vec<Subject>>;
}
```

---

## CardDAV Layer

### Why CardDAV

CardDAV (RFC 6352) is the standard protocol for contact synchronization. Supporting it means the contact store integrates out of the box with Apple Contacts, Thunderbird, DAVx⁵ on Android, Evolution, and any other compliant client.

CardDAV stores contacts as **vCard** objects (RFC 6350) hosted on a WebDAV server. The mapping from the fact model to vCard happens at the protocol boundary — the internal representation stays as facts, and vCards are generated on the fly when a client requests them.

### Mapping Strategy

**Fact → vCard (on read)**: Materialize the `ContactView` as of "now" (or as of the `If-Modified-Since` date if provided), then serialize all active facts into a single vCard. Multi-valued vCard properties (TEL, EMAIL, ADR) map to multiple facts of the same type.

**vCard → Facts (on write/PUT)**: When a client PUTs a vCard, diff it against the current materialized view. Properties that are new become new facts recorded now with `effective_at = now`. Properties that have changed create a new fact superseding the old one. Properties that have disappeared are retracted. This preserves the event-sourced history even when clients use the standard protocol without knowing about it.

**ETag handling**: Use a hash of all current fact IDs and their recorded_at timestamps as the ETag for a contact resource. This changes whenever any fact is added, superseded, or retracted.

### vCard Field Mapping

| vCard Property | Fact Type |
|---|---|
| `FN`, `N` | `name` |
| `NICKNAME` | `alias` |
| `TEL` | `phone` |
| `EMAIL` | `email` |
| `ADR` | `address` |
| `URL` | `url` |
| `BDAY` | `birthday` |
| `ANNIVERSARY` | `anniversary` |
| `ORG`, `TITLE`, `ROLE` | `organization_membership` |
| `NOTE` | `note` |
| `PHOTO` | `photo` |
| `X-*` custom properties | `custom` |

The `PRODID`, `REV`, and `UID` fields are generated from server-side metadata and do not map to facts directly. `UID` maps to `subject_id`.

### CardDAV Endpoints

```
/dav/                         Principal collection (RFC 5397)
/dav/addressbooks/            Address book home set
/dav/addressbooks/{name}/     Address book collection
/dav/addressbooks/{name}/{uid}.vcf   Individual contact resource
```

### HTTP Methods to Implement

- `PROPFIND` — list collections and resource properties
- `GET` — retrieve a vCard
- `PUT` — create or update a contact (triggers the vCard diff → fact ingestion pipeline)
- `DELETE` — retract all active facts for a subject (recorded as retractions, subject remains)
- `REPORT` — `addressbook-query` and `addressbook-multiget` for bulk fetch and search
- `OPTIONS` — advertise CardDAV compliance via `DAV:` header

---

## HTTP Server

Use **`axum`** as the HTTP framework. Axum's handler model is ergonomic, `tower` middleware integrates cleanly, and async-first design suits the store trait.

Key middleware:

- Basic auth or digest auth (RFC 7617) for initial implementation; JWT/OAuth2 later
- Request ID injection for logging
- Per-request timing

The WebDAV XML bodies use **`quick-xml`** for both parsing and generation, keeping the dependency footprint small. Define typed Rust structs for the WebDAV/CardDAV XML vocabulary and implement serialization manually or with `serde` + `quick-xml`'s serde support.

---

## Crate Structure

```
kith/
├── Cargo.toml
├── crates/
│   ├── kith-core/          # Fact types, Subject, ContactView, trait definitions
│   ├── kith-store-sqlite/  # SQLite implementation of ContactStore
│   ├── kith-vcard/         # vCard RFC 6350 parser and serializer
│   ├── kith-carddav/       # WebDAV/CardDAV protocol layer (axum handlers)
│   └── kith-cli/           # Command-line management tool
└── src/
    └── main.rs             # Wires everything together, reads config
```

This workspace layout lets each layer be tested in isolation. `kith-vcard` has no async code and no HTTP dependencies. `kith-store-sqlite` has no HTTP knowledge. `kith-carddav` depends on `kith-core` and `kith-vcard` but not on the store implementation (via the trait).

---

## Key Dependencies

| Crate | Purpose |
|---|---|
| `axum` | HTTP server |
| `tokio` | Async runtime |
| `rusqlite` / `rusqlite-tokio` | SQLite storage |
| `uuid` | UUID generation and parsing |
| `chrono` | Date/time handling |
| `serde` + `serde_json` | Fact value serialization |
| `quick-xml` | WebDAV XML parsing/generation |
| `thiserror` | Error type derivation |
| `tracing` + `tracing-subscriber` | Structured logging |
| `config` | Configuration file support |
| `clap` | CLI argument parsing |

---

## Temporal Queries

The event-sourced model enables queries that flat contact stores cannot answer:

- "What was Alice's phone number in 2019?"
- "When did Bob join Acme Corp?"
- "Show me everything I recorded about Carol in March"
- "What facts did I record but then retract?"

The `as_of` parameter on `get_facts` and `materialize` drives point-in-time reads. The distinction between `recorded_at` and `effective_at` is the key design choice:

- `recorded_at` answers: "When did I learn this?"
- `effective_at` answers: "When was this true?"

Both are always queryable independently.

---

## Conflict and Concurrency Model

The store is single-writer by design (personal use, one SQLite file). No CRDT or distributed conflict resolution is needed. Concurrent CardDAV clients are handled via ETags and `If-Match` headers — a PUT that provides a stale ETag receives a `412 Precondition Failed` response, prompting the client to re-fetch and retry.

---

## Search

Full-text search over fact values is implemented in two phases:

**Phase 1 (MVP):** SQL `LIKE` queries over the JSON `value` column for simple name/email lookups. Sufficient for personal use.

**Phase 2:** Integrate SQLite FTS5 for proper full-text search. Index a normalized text representation of each fact value. Exposed via the CardDAV `addressbook-query` REPORT request's `prop-filter` and `text-match` elements.

---

## Configuration

TOML configuration file:

```toml
[server]
host = "127.0.0.1"
port = 5232
base_url = "http://localhost:5232"

[auth]
username = "user"
password_hash = "$argon2..."   # argon2 hash

[store]
path = "~/.local/share/contact-store/contacts.db"

[addressbooks]
default = "personal"
```

---

## Implementation Phases

**Phase 1 — Core store:** Define all Rust types in `core`. Implement `store-sqlite`. Write unit tests for fact ingestion, supersession, retraction, and `materialize`. No HTTP yet.

**Phase 2 — vCard codec:** Implement vCard 3.0 and 4.0 parsing and serialization in the `vcard` crate. Map the full fact taxonomy to vCard fields. Fuzz the parser.

**Phase 3 — CardDAV server:** Implement the axum handlers in `carddav`. Support `PROPFIND`, `GET`, `PUT`, `DELETE`. Test with `curl` and then with a real client (Apple Contacts or DAVx⁵).

**Phase 4 — REPORT and search:** Implement `addressbook-query` and `addressbook-multiget`. Add FTS5.

**Phase 5 — CLI:** A `contact` CLI for importing/exporting vCards, inspecting history, querying facts by date, and managing subjects without a CardDAV client.

---

## Resolved Design Decisions

**vCard 3.0 and 4.0:** The `vcard` crate will parse both. On GET, Kith serves vCard 4.0 by default and negotiates down to 3.0 if the client's `Accept` header demands it. On PUT, Kith accepts both and normalizes to the internal fact model regardless of version.

**Single address book:** One address book for now (`personal`). The schema does not need an `addressbook_id` column at this stage; the path `/dav/addressbooks/personal/` is the only collection.

**Relationship facts and CardDAV:** Exposed via the `X-KITH-RELATION` custom vCard property. Full relationship querying is only available through the native API.

**Photo storage:** Photos live on disk at `{photo_dir}/{subject_id}/{content_hash}.{ext}`. The `PhotoValue` fact stores the relative path, content hash (SHA-256), and MIME type. The hash enables deduplication (two subjects can share a file by hash) and is used as a component of the ETag. No photo data is stored in SQLite.

**Import provenance:** The `RecordingContext::Imported` variant carries `source_name` and `original_uid`. This threads through every fact ingested via the import tool or a CardDAV PUT, so the full history of where information came from is always queryable.
