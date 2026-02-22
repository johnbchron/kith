# Plan: Phase 3 — kith-carddav

## Context

Phase 1 delivered `kith-core` + `kith-store-sqlite` (19 passing tests). Phase 2
delivered `kith-vcard`, a pure synchronous vCard 3.0/4.0 codec. Phase 3 wires
them together into a working CardDAV server.

Goal: a running HTTP server that Apple Contacts, Thunderbird, DAVx⁵, and
`vdirsyncer` can connect to over standard CardDAV (RFC 6352). Internally, every
contact write is transformed into append-only fact operations so the full
event-sourced history is preserved.

This phase implements `kith-carddav` (handlers + server binary). Phase 4 adds
`REPORT` and FTS5 search.

---

## New Workspace Dependencies

Add to `[workspace.dependencies]` in the root `Cargo.toml`:

| Dep | Version | Feature flags |
|---|---|---|
| `axum` | `0.8` | `macros` |
| `quick-xml` | `0.37` | `serialize` |
| `argon2` | `0.5` | — |
| `base64` | `0.22` | — |
| `config` | `0.14` | — |
| `clap` | `4` | `derive` |
| `sha2` | `0.10` | — |
| `hex` | `0.4` | — |
| `tower` | `0.5` | — |
| `tower-http` | `0.6` | `trace` |
| `bytes` | `1` | — |

---

## Files to Create / Modify

| File | Action |
|---|---|
| `Cargo.toml` (workspace root) | Add new workspace deps above |
| `crates/kith-carddav/Cargo.toml` | Wire in deps; add `[[bin]]` entry |
| `crates/kith-carddav/src/lib.rs` | Replace stub: `router()`, `AppState<S>`, re-exports |
| `crates/kith-carddav/src/error.rs` | New — error types + axum `IntoResponse` |
| `crates/kith-carddav/src/auth.rs` | New — Basic auth axum extractor |
| `crates/kith-carddav/src/xml.rs` | New — WebDAV XML parse/generate |
| `crates/kith-carddav/src/etag.rs` | New — ETag computation |
| `crates/kith-carddav/src/diff.rs` | New — vCard diff → store operations |
| `crates/kith-carddav/src/handlers/mod.rs` | New |
| `crates/kith-carddav/src/handlers/options.rs` | New |
| `crates/kith-carddav/src/handlers/propfind.rs` | New |
| `crates/kith-carddav/src/handlers/get.rs` | New |
| `crates/kith-carddav/src/handlers/put.rs` | New |
| `crates/kith-carddav/src/handlers/delete.rs` | New |
| `crates/kith-carddav/src/bin/server.rs` | New — binary entry point |

---

## Public Surface (`lib.rs`)

```rust
pub use error::Error;

pub fn router<S>(state: AppState<S>) -> axum::Router
where
    S: ContactStore + Clone + 'static,
    S::Error: std::error::Error + Send + Sync + 'static;

#[derive(Clone)]
pub struct AppState<S: ContactStore> {
    pub store:  Arc<S>,
    pub config: Arc<ServerConfig>,   // base_url, addressbook name
    pub auth:   Arc<AuthConfig>,     // username + argon2 password hash
}
```

The router is generic over the store so `kith-carddav` never imports
`kith-store-sqlite`. The binary crate instantiates the concrete type.

---

## Error Type (`error.rs`)

```rust
pub enum Error {
    Unauthorized,
    NotFound,
    PreconditionFailed,           // If-Match mismatch
    Conflict(String),
    BadRequest(String),
    Xml(String),
    Vcard(kith_vcard::Error),
    Store(Box<dyn std::error::Error + Send + Sync>),
}
```

`Error` implements axum's `IntoResponse`:

| Variant | HTTP status |
|---|---|
| `Unauthorized` | 401 + `WWW-Authenticate: Basic realm="kith"` |
| `NotFound` | 404 |
| `PreconditionFailed` | 412 |
| `Conflict` | 409 |
| `BadRequest` | 400 |
| `Xml` / `Vcard` / `Store` | 500 |

---

## Auth (`auth.rs`)

Axum `FromRequestParts` extractor. Reads the `Authorization: Basic …` header,
base64-decodes it, splits on `:`, and verifies with `argon2::verify_password`.
Returns `Error::Unauthorized` on any mismatch (no timing oracle — argon2's
constant-time compare is used throughout).

```rust
pub struct AuthConfig {
    pub username:      String,
    pub password_hash: String,   // PHC string, e.g. "$argon2id$v=19$…"
}

pub struct Authenticated;   // zero-size marker

impl<S: Send + Sync> FromRequestParts<AppState<S>> for Authenticated { … }
```

For development a helper CLI command can print the argon2 hash for a given
plaintext password.

---

## WebDAV XML (`xml.rs`)

Uses `quick-xml`'s **writer API** (not serde) for generating all XML responses.
Uses a simple hand-written parser for reading PROPFIND request bodies.

### Namespaces

```
DAV:             — prefix D
urn:ietf:params:xml:ns:carddav  — prefix card
```

### PROPFIND request parsing

```rust
pub enum PropfindRequest {
    AllProp,
    PropNames,
    Prop(Vec<PropName>),   // only the names the client asked for
}

pub enum PropName {
    ResourceType, DisplayName, GetContentType, GetETag,
    GetContentLength, GetLastModified,
    CurrentUserPrincipal, AddressbookHomeSet,
    AddressbookDescription, SupportedAddressData,
    AddressData,
    Unknown(String),
}

pub fn parse_propfind(xml: &[u8]) -> Result<PropfindRequest, Error>
```

Empty / missing body is treated as `AllProp`.

### PROPFIND response builder

```rust
pub struct MultistatusBuilder;

impl MultistatusBuilder {
    pub fn response(&mut self, href: &str) -> ResponseBuilder;
    pub fn finish(self) -> Vec<u8>;  // UTF-8 XML bytes
}

pub struct ResponseBuilder<'a> {
    pub fn propstat_ok(self, props: &[Property]) -> &'a mut MultistatusBuilder;
    pub fn propstat_not_found(self, names: &[PropName]) -> &'a mut MultistatusBuilder;
}
```

### Property enum

```rust
pub enum Property {
    ResourceType(ResourceType),        // collection, addressbook, principal
    DisplayName(String),
    GetContentType(String),
    GetETag(String),
    GetContentLength(u64),
    GetLastModified(String),           // RFC 7231 / RFC 9110 format
    CurrentUserPrincipal(String),      // href value
    AddressbookHomeSet(String),        // href value
    AddressbookDescription(String),
    SupportedAddressData,              // advertises vCard 3.0 + 4.0
}
```

---

## ETag (`etag.rs`)

```rust
/// Stable ETag for a contact view.
/// SHA-256 of all active fact_ids (sorted) + their recorded_at timestamps.
/// Changes whenever any fact is added, superseded, or retracted.
pub fn compute_etag(view: &ContactView) -> String {
    // Sort by fact_id for determinism.
    // Hash: for each fact in sorted order, SHA-256 update with
    //   16 bytes (UUID) + 8 bytes (timestamp micros, little-endian)
    // Return: format!("\"{}\"", hex::encode(sha256.finalize()))
}

/// Compute ETag directly from (fact_id, recorded_at) pairs.
/// Used in PUT to compute the new ETag after writes complete.
pub fn compute_etag_from_pairs(pairs: &[(Uuid, DateTime<Utc>)]) -> String { … }
```

---

## vCard Diff Pipeline (`diff.rs`)

This is the most complex module. Called by the PUT handler.

```rust
pub struct DiffResult {
    pub new_facts:     Vec<NewFact>,
    pub supersessions: Vec<(Uuid /* old_fact_id */, NewFact)>,
    pub retractions:   Vec<Uuid>,
}

/// Compute the minimal set of store operations that transitions
/// `current_view` to match `incoming_vcard`.
///
/// When `current_view` is `None` (new contact), all parsed facts are new.
pub fn diff(
    incoming_vcard:  &str,
    subject_id:      Uuid,
    source_name:     &str,
    current_view:    Option<&ContactView>,
) -> Result<DiffResult, kith_vcard::Error>
```

### Matching strategy

Incoming parsed facts are matched to existing active facts by **type + key
fields**. A match means "this is the same logical piece of information".

| Fact type | Match key |
|---|---|
| `Name` | (only one per contact; always matched) |
| `Email` | `address` (normalized to lowercase) |
| `Phone` | `number` (stripped of whitespace/dashes) |
| `Address` | `(street, locality, postal_code)` |
| `OrgMembership` | `org_name` (case-insensitive) |
| `Birthday` | (only one per contact) |
| `Anniversary` | (only one per contact) |
| `Gender` | (only one per contact) |
| `Alias` | `name` |
| `Url` | `url` |
| `Im` | `(service, handle)` |
| `Social` | `(platform, handle)` |
| `Note` | exact content match |
| `GroupMembership` | `group_id` if present, else `group_name` |
| `Relationship` | `(relation, other_id)` |
| `Meeting` | `(summary, effective_at)` |
| `Introduction` | exact content match |
| `Custom` | `key` |

### Decision logic

For each incoming fact:
- If a match exists in `current_view` and the values are **identical** → skip
  (no-op, fact is unchanged).
- If a match exists and values **differ** → supersession (old_fact_id,
  replacement NewFact).
- If **no match** → new fact.

For each active fact in `current_view` not matched by any incoming fact →
retraction.

### `NewFact` construction

All facts produced by `diff` get:
- `subject_id`: the real UUID passed in
- `confidence`: `Certain`
- `recording_context`: `Imported { source_name, original_uid: uid_from_vcard }`
- `effective_at`: `None` (clients can set it later via the native API)
- `tags`: `[]`

---

## URL Router (`lib.rs` / `handlers/`)

```
OPTIONS  /dav/*path             → options::handler
PROPFIND /dav/                  → propfind::principal
PROPFIND /dav/addressbooks/     → propfind::home_set
PROPFIND /dav/addressbooks/:ab/ → propfind::collection
PROPFIND /dav/addressbooks/:ab/:uid.vcf → propfind::resource
GET      /dav/addressbooks/:ab/:uid.vcf → get::handler
HEAD     /dav/addressbooks/:ab/:uid.vcf → get::handler (no body)
PUT      /dav/addressbooks/:ab/:uid.vcf → put::handler
DELETE   /dav/addressbooks/:ab/:uid.vcf → delete::handler
```

All handlers require `Authenticated` extractor (except `OPTIONS`).

### OPTIONS (`handlers/options.rs`)

Returns `204 No Content` with:
```
Allow: OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, REPORT
DAV: 1, 3, addressbook
```
No auth required (CalDAV/CardDAV clients probe OPTIONS first to discover the
server before sending credentials).

### PROPFIND (`handlers/propfind.rs`)

**Depth header:** `0` or `1`. Depth-infinity not supported (return 403).

**Principal (`/dav/`):**
- Returns `href`, `displayname`, `current-user-principal`, `addressbook-home-set`.

**Home set (`/dav/addressbooks/`):**
- Returns `href`, `displayname`, `resourcetype` (collection).

**Collection (`/dav/addressbooks/personal/`, Depth: 0):**
- Returns collection properties: `resourcetype` (collection + addressbook),
  `displayname`, `supported-address-data`, `addressbook-description`.

**Collection (Depth: 1):**
- Depth-0 response for the collection itself, plus one `<D:response>` per
  subject. For each subject, call `store.materialize(id, None)` and emit
  `href`, `getcontenttype`, `getetag`, `getcontentlength`.

**Resource (`/dav/addressbooks/personal/{uuid}.vcf`):**
- `store.materialize(uuid, None)` → 404 if `None`.
- Properties: `getcontenttype`, `getetag`, `getcontentlength`, `getlastmodified`.

### GET / HEAD (`handlers/get.rs`)

1. Parse `uid` from path (UUID, strip `.vcf`).
2. `store.materialize(uid, None).await` → 404 if `None`.
3. `kith_vcard::serialize(&view)` → vCard 4.0 string.
4. Set headers: `Content-Type: text/vcard; charset=utf-8`, `ETag`.
5. HEAD: headers only, no body.

### PUT (`handlers/put.rs`)

1. Parse `uid` from path.
2. Read body as UTF-8 string.
3. `store.get_subject(uid).await`:
   - If `None`: this is a create. Check `If-Match` header — if present, return 412 (can't match a non-existent resource). Create the subject with `store.add_subject(SubjectKind::Person).await`.
   - If `Some`: this is an update. If `If-Match` header is present, compute current ETag and return 412 if mismatch.
4. `store.materialize(uid, None).await` → `current_view` (None for new subjects).
5. `diff::diff(body, uid, source_name, current_view.as_ref())` → `DiffResult`.
6. Apply in order: new facts → record, supersessions → supersede, retractions → retract.
7. Compute new ETag and return:
   - `201 Created` + `ETag` (new subject)
   - `204 No Content` + `ETag` (updated subject)

`source_name` is `"carddav-put"`.

### DELETE (`handlers/delete.rs`)

1. Parse `uid` from path.
2. `store.get_subject(uid).await` → 404 if `None`.
3. `store.get_facts(uid, None, false).await` → active facts.
4. For each active fact: `store.retract(fact_id, Some("Deleted via CardDAV"))`.
5. Return `204 No Content`.

Subject record itself is kept (per design: subjects are permanent envelopes).

---

## Server Binary (`src/bin/server.rs`)

```rust
#[derive(clap::Parser)]
struct Cli {
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. init tracing-subscriber
    // 2. parse Cli
    // 3. load Config from TOML
    // 4. open SqliteStore (from kith-store-sqlite)
    // 5. build AppState { store, config, auth }
    // 6. build router(state)
    // 7. bind TcpListener and axum::serve(…).await
}
```

The binary crate adds `kith-store-sqlite` as a dev-dependency here only, keeping
the `kith-carddav` library itself store-agnostic.

### `ServerConfig` struct

```rust
#[derive(serde::Deserialize, Clone)]
pub struct ServerConfig {
    pub host:          String,      // "127.0.0.1"
    pub port:          u16,         // 5232
    pub base_url:      String,      // "http://localhost:5232"
    pub addressbook:   String,      // "personal"
    pub store_path:    PathBuf,
    pub auth_username: String,
    pub auth_password_hash: String, // argon2 PHC string
}
```

Example `config.toml`:
```toml
host             = "127.0.0.1"
port             = 5232
base_url         = "http://localhost:5232"
addressbook      = "personal"
store_path       = "~/.local/share/kith/contacts.db"
auth_username    = "user"
auth_password_hash = "$argon2id$v=19$m=19456,t=2,p=1$…"
```

---

## Tests

All tests live in `#[cfg(test)]` modules within each source file. Integration
tests use `tower::ServiceExt::oneshot` to drive the axum router without binding
a socket.

### `xml.rs` tests
- `parse_propfind` with `<D:allprop/>` → `AllProp`
- `parse_propfind` with `<D:prop>` list → `Prop([…])`
- Empty body → `AllProp`
- `MultistatusBuilder` round-trip: build a two-response multistatus, parse back
  with quick-xml, verify hrefs and status text.

### `etag.rs` tests
- Same facts in different insertion order → same ETag.
- Adding a fact changes the ETag.

### `diff.rs` tests
- `None` current view → all incoming facts are new.
- Unchanged contact → empty `DiffResult`.
- Email address changed → one supersession, zero new/retracted.
- Phone number added → one new fact, rest unchanged.
- Email removed from vCard → one retraction.
- Full contact (name + email + phone + org + note) round-trip.

### `auth.rs` tests
- Correct credentials → `Ok(Authenticated)`.
- Wrong password → `Err(Error::Unauthorized)`.
- Missing `Authorization` header → `Err(Error::Unauthorized)`.
- Invalid base64 → `Err(Error::Unauthorized)`.

### Handler integration tests (using `oneshot`)

**OPTIONS**
- Returns 204 with `DAV: 1, 3, addressbook` header.

**PROPFIND collection**
- Empty store → 207 with just the collection response.
- One subject in store → 207 with two responses (collection + one contact).

**GET**
- Non-existent UUID → 404.
- Existing subject → 200, `Content-Type: text/vcard`, body contains `BEGIN:VCARD`.

**PUT (create)**
- PUT with new UUID → 201, `ETag` header set.
- Subsequent GET returns the same vCard content.

**PUT (update, If-Match)**
- PUT with correct `If-Match` → 204, ETag changes.
- PUT with stale `If-Match` → 412.

**DELETE**
- DELETE existing → 204.
- Subsequent GET → 404.
- DELETE non-existent → 404.

**Auth**
- All non-OPTIONS requests without auth → 401.

---

## Verification

```bash
# All tests pass
cargo test -p kith-carddav
cargo test

# No new warnings
cargo clippy --all-targets

# Manual smoke test with curl
cargo run -p kith-carddav --bin server -- --config config.toml &

curl -i -X OPTIONS http://localhost:5232/dav/
# → 204, DAV: 1, 3, addressbook

curl -i -u user:secret -X PROPFIND http://localhost:5232/dav/addressbooks/personal/ \
     -H "Depth: 1" -H "Content-Type: application/xml" \
     --data '<D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>'
# → 207 Multi-Status

curl -i -u user:secret -X PUT \
     http://localhost:5232/dav/addressbooks/personal/$(uuidgen).vcf \
     -H "Content-Type: text/vcard" \
     --data $'BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Test User\r\nEMAIL:test@example.com\r\nEND:VCARD\r\n'
# → 201 Created
```
