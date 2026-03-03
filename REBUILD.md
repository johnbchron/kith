# Kith: Rebuild Plan

## Objective

Rebuild kith to accomplish the same goals — an event-sourced personal contact store that speaks CardDAV — while significantly reducing the implementation burden. The core domain model (`kith-core`) and the event-sourcing design are correct and should be preserved. The largest implementation costs are in the protocol and storage layers, where mature external crates can shoulder the work.

**Estimated line reduction: ~34k → ~18k lines of Rust** (roughly 45%), concentrated in the most fragile and RFC-compliance-sensitive code.

---

## Diagnosis: Where the Burden Lives

| Crate | LOC | What's expensive |
|---|---|---|
| `kith-core` | ~1.2k | Not expensive. Types and trait are clean. |
| `kith-store-sqlite` | ~1.4k | Verbose due to rusqlite closure pattern; `encode.rs` is boilerplate. |
| `kith-vcard` | ~2.2k | Full custom parser and serializer for RFC 6350/6352. Fragile. |
| `kith-carddav` | ~2.2k | Custom WebDAV XML layer, PROPFIND/REPORT handlers, ETag generation. |
| `kith-api` | ~1k | Fine as-is. |
| `kith-cli` | ~0.7k | Manageable; hand-rolled text input could use a library widget. |

The three highest-value targets for external replacement:

1. **`kith-vcard`** — the vCard parser/serializer is a maintenance liability. Real-world clients emit subtly malformed vCards, and RFC 6350 has many edge cases (line folding, quoted-printable, date format variants, parameter quoting). This is exactly the problem a mature library should own.

2. **`kith-carddav`** — the WebDAV XML layer (PROPFIND multistatus, REPORT, ETag, DAV compliance headers) is complex and client-picky. A WebDAV server library handles this correctly by construction.

3. **`kith-store-sqlite`** — the `rusqlite + tokio-rusqlite` pattern requires every query to live inside a blocking closure with manually encoded types. `sqlx` with compile-time query checking is cleaner and eliminates most of `encode.rs`.

---

## External Crates Selected

### `dav-server` 0.11 (Apache 2.0)

A WebDAV/CardDAV server library that passes the WebDAV Litmus test suite. It provides the full RFC 4918 (WebDAV) and RFC 6352 (CardDAV) protocol layer — PROPFIND, REPORT, OPTIONS, ETag handling, Depth headers, multistatus XML responses — through a trait-based backend interface.

The `carddav` feature (added in 0.11) layers CardDAV semantics on top, handling `addressbook-query` and `addressbook-multiget` REPORT types. Internally it uses `calcard` (see below) as its vCard model.

**What it replaces:** `kith-carddav/src/handlers/propfind.rs`, `options.rs`, the XML layer in `xml.rs`, ETag generation in `etag.rs`, and all Depth-header parsing. The handler for `GET` and `DELETE` become simple delegations to the store.

**Integration model:** Implement the `DavFileSystem` trait. The trait exposes a virtual filesystem where address books are directories and `.vcf` contacts are files. `dav-server` handles all incoming requests, calls into the `DavFileSystem` implementation, and produces correct WebDAV responses.

### `calcard` 0.3 (stalwartlabs, Apache 2.0)

A production-grade vCard 3.0/4.0 (and iCalendar, JSContact) parser and serializer maintained by Stalwart Labs — the team behind a production Rust mail server that already ships a CardDAV endpoint. It follows the Robustness Principle (liberal on input, correct on output), has fuzz infrastructure, and is the vCard library that `dav-server` itself uses.

**What it replaces:** `kith-vcard/src/parse.rs` and `kith-vcard/src/serialize.rs`. The custom line-folding, quoted-printable, date-format, and parameter-quoting code becomes `calcard`'s problem.

**What remains custom:** The mapping between `calcard::VCard` and kith's `Vec<FactValue>`. This mapping layer is unavoidable — it encodes kith's domain model (fact types, labels, preference ranks, custom `X-KITH-*` extension properties). But it is pure data transformation, not protocol implementation, and it has no RFC compliance risk surface.

### `sqlx` 0.8 (MIT)

An async-first SQL library with compile-time query verification. Compared to `rusqlite + tokio-rusqlite`, it eliminates the closure-dispatch pattern and provides typed `FromRow` derivation that replaces most of `kith-store-sqlite/src/encode.rs`.

**What it replaces:** All of `tokio-rusqlite`'s `conn.call(|conn| { ... })` wrappers, the hand-rolled type encoding for UUIDs, DateTimes, and JSON blobs, and the `RawResolvedFact` intermediate struct used for row mapping.

**What stays the same:** The SQL schema (no changes needed), the `ContactStore` trait and its semantics, and the complex transactional lifecycle queries (`supersede`, `retract` with UNIQUE constraint race detection).

### `tui-textarea` 0.7 (MIT)

A ratatui widget providing a multi-line editor with cursor movement, undo/redo, and single-line input mode. It directly replaces the hand-rolled `filter` string buffer in `kith-cli` and will be the primary input widget for fact editing forms.

---

## Rebuild Structure

The workspace crate layout stays the same. The changes are internal to each crate.

```
kith/
└── crates/
    ├── kith-core/          # UNCHANGED — types, trait, no modifications
    ├── kith-store-sqlite/  # REFACTORED — rusqlite → sqlx
    ├── kith-vcard/         # REPLACED — becomes a thin mapping layer only
    ├── kith-carddav/       # REPLACED — becomes a DavFileSystem implementation
    ├── kith-api/           # UNCHANGED — JSON REST endpoints
    └── kith-cli/           # MINOR UPDATE — tui-textarea for input fields
```

---

## Phase 1 — Migrate Storage to `sqlx`

**Goal:** Eliminate the `rusqlite + tokio-rusqlite` closure pattern and the `encode.rs` boilerplate.

**Changes to `kith-store-sqlite`:**

1. Replace `rusqlite` and `tokio-rusqlite` with `sqlx` configured for SQLite with the bundled feature.
2. Replace `encode.rs` custom type converters with `sqlx::Type` derive impls:
   - `Uuid` → encode as hyphenated lowercase string (implement `sqlx::Encode`/`Decode` for the newtype or use `sqlx::types::Uuid` if compatible)
   - `DateTime<Utc>` → encode as RFC 3339 string
   - `FactValue`, `EffectiveDate`, `RecordingContext` → encode as JSON strings (implement `Json<T>` wrapper or `sqlx::Type` for newtypes)
3. Replace `RawResolvedFact` + manual row-mapping closures with `#[derive(FromRow)]` on an intermediate struct, then convert to domain types.
4. Replace all `conn.call(|conn| { ... })` blocks with `sqlx::query!()` or `sqlx::query_as!()` macro calls.
5. Use `sqlx::SqlitePool` for connection management (replaces the single `Connection` in a `Mutex`).
6. Add a `sqlx migrate!()` setup so the schema is managed by `sqlx`'s migration system rather than inline `CREATE TABLE IF NOT EXISTS` strings.

**Schema:** No changes to the schema itself. The migration files are the existing `CREATE TABLE` statements extracted into `migrations/` directory files.

**Estimated result:** `encode.rs` (~170 LOC) eliminated; `store.rs` (~600 LOC) reduced to ~350 LOC.

---

## Phase 2 — Replace `kith-vcard` with a Mapping Layer

**Goal:** Remove the custom RFC 6350 parser and serializer. Keep only the kith-specific property mappings.

**The new `kith-vcard` crate contains only:**

### `src/from_vcard.rs` — `calcard::VCard` → `Vec<NewFact>`

This replaces `parse.rs`. Instead of parsing raw vCard text, it receives a `calcard::VCard` (already parsed by `calcard`) and maps its typed property values to kith's `FactValue` variants.

Standard property mapping (no parsing logic, just field extraction):

```rust
// Example: calcard already gave us a typed EmailProperty
fn map_email(prop: &calcard::types::EmailProperty) -> NewFact {
    NewFact::new(subject_id, FactValue::Email(EmailValue {
        address: prop.address.clone(),
        label:   map_type_param(&prop.types),
        preference: prop.pref.unwrap_or(99),
    }))
}
```

X-KITH-* extension properties are available in `calcard::VCard` as `extensions: Vec<(String, calcard::types::Property)>` (or equivalent). Filter by `X-KITH-SOCIAL`, `X-KITH-GROUP`, `X-KITH-RELATION` and map to the corresponding `FactValue` variants.

### `src/to_vcard.rs` — `ContactView` → `calcard::VCard`

This replaces `serialize.rs`. Instead of generating raw vCard text, it constructs a `calcard::VCard` struct from active facts, then calls `calcard`'s serializer for the actual output.

The serializer handles line folding, CRLF, character encoding, vCard 3.0/4.0 version negotiation — kith's code just populates the struct.

**What disappears entirely:**
- `src/unfold.rs` — line-folding logic
- `src/parse.rs` — raw text parsing, quoted-printable, parameter quoting
- All date format parsing variants (handled by calcard)
- The `flush_accumulators` pattern (calcard parses into typed structs directly)

**Estimated result:** `kith-vcard` reduced from ~2.2k LOC to ~500 LOC (the mapping layer only).

---

## Phase 3 — Replace `kith-carddav` with a `DavFileSystem` Implementation

**Goal:** Remove the custom WebDAV XML layer, PROPFIND handlers, ETag generation, and REPORT handlers. Replace with a `dav-server` backend implementation.

### Architecture

`dav-server` accepts an axum `Router` integration and routes all WebDAV/CardDAV requests through its own handler logic. The application provides a `DavFileSystem` impl that maps the virtual filesystem model to the kith store.

The virtual filesystem mapping:

| WebDAV resource | kith entity |
|---|---|
| `/dav/` | Principal collection |
| `/dav/addressbooks/` | Address book home |
| `/dav/addressbooks/personal/` | Address book (directory) |
| `/dav/addressbooks/personal/{uuid}.vcf` | Contact (file, content is vCard) |

### `src/fs.rs` — `DavFileSystem` implementation

The implementation stores an `Arc<dyn ContactStore>` and answers `dav-server`'s queries:

- **`read_dir`** (for `PROPFIND Depth:1` on the address book): call `store.list_subjects()`, return a `DavDirEntry` for each subject. The `DavMetaData` for each entry provides the ETag (computed from fact IDs + recorded_at timestamps, same as current `etag.rs`) and content length (approximate, or computed on demand).
- **`get_file`** (for `GET` and `REPORT` multiget): call `store.materialize(subject_id)`, convert `ContactView` → `calcard::VCard` via `kith-vcard`'s `to_vcard`, serialize with `calcard`, return as a `DavFile` whose `read` yields the vCard bytes.
- **`put_file`** (for `PUT`): receive vCard bytes, parse with `calcard`, convert to facts via `kith-vcard`'s `from_vcard`, run the diff against current active facts, apply store operations (new facts, supersessions, retractions). This is the current `diff.rs` logic, preserved.
- **`remove_file`** (for `DELETE`): call `store.retract` for all active facts of the subject.
- **`create_dir`** / **`remove_dir`**: return `Forbidden` or `MethodNotAllowed` (single address book for now).

### `src/diff.rs` — **PRESERVED**

The vCard diff algorithm (incoming facts vs. current active facts → minimal store operations) is custom logic that encodes kith's domain semantics (idempotency, supersession vs. retraction decisions, multi-valued property matching). It stays in kith-carddav and is called from `put_file`. It receives `Vec<NewFact>` from `kith-vcard::from_vcard` and `Vec<ResolvedFact>` from `store.materialize`.

### `src/auth.rs` — **PRESERVED**

Basic auth with Argon2 stays as-is. `dav-server` integrates via Tower middleware, so the existing auth layer slots in unchanged as a `tower::Layer` wrapping the `dav-server` service.

### What disappears entirely:

- `src/handlers/propfind.rs` (~150 lines of multistatus XML building)
- `src/handlers/options.rs` (DAV compliance headers)
- `src/handlers/report.rs` (~200 lines of addressbook-query/multiget XML parsing and dispatch)
- `src/xml.rs` (~524 lines of WebDAV XML generation)
- `src/etag.rs` (~123 lines — ETag generation moves into `DavMetaData::etag`)
- Most of `src/handlers/get.rs`, `src/handlers/delete.rs` (become one-liners via DavFileSystem)
- `src/handlers/put.rs` routing logic (~200 lines, replaced by `put_file`)

**Estimated result:** `kith-carddav` reduced from ~2.2k LOC to ~600 LOC (`fs.rs` + `diff.rs` + `auth.rs` + wiring).

---

## Phase 4 — CLI Input with `tui-textarea`

**Goal:** Replace the hand-rolled `filter` string buffer with a proper input widget; prepare for fact editing forms.

**Changes to `kith-cli`:**

1. Replace the `filter: String` + `filter.push(c)` / `filter.pop()` pattern in `app.rs` with a `tui-textarea::TextArea` in single-line mode. This gives cursor movement, backspace, and clipboard support without additional code.
2. When implementing `Screen::EditFact` (Phase C of the TUI plan), use a `TextArea` per editable field rather than building a custom text input widget. One `TextArea` per `FactValue` field.
3. No structural changes to the state machine. The `Screen` enum and event dispatch pattern are adequate for the current scope.

**Estimated result:** ~50 LOC saved now; larger savings when the edit screen is implemented.

---

## Risk Register

| Risk | Likelihood | Mitigation |
|---|---|---|
| `dav-server` 0.11 `carddav` feature doesn't fully cover `addressbook-query` REPORT | Medium | The existing `kith-carddav/src/handlers/report.rs` can be retained as a fallback; evaluate by reading `dav-server` source before starting Phase 3 |
| `calcard` extension property API doesn't expose `X-KITH-*` values cleanly | Low | `calcard` preserves unknown properties as raw extension fields per RFC; verify in a spike before starting Phase 2 |
| `sqlx` compile-time query checking requires an offline `.sqlx/` cache to build without a live database | Low | Add `sqlx prepare` to the CI workflow; or use `sqlx::query()` (runtime-only) for the complex multi-join queries |
| `dav-server`'s `DavFileSystem` trait is filesystem-oriented; some CardDAV collection properties may need custom implementation | Medium | Evaluate by prototyping the `DavFileSystem` impl; worst case, some PROPFIND properties need manual property handlers which dav-server supports via extension hooks |

---

## Implementation Order

Each phase is independently shippable and testable without breaking the others.

1. **Phase 1 (sqlx)** — Pure refactor of `kith-store-sqlite`. All existing store tests should pass unchanged after the migration. No API or protocol changes.

2. **Phase 2 (calcard mapping)** — Spike first: write a `calcard` parsing test against a real-world `.vcf` file and verify that `X-KITH-*` extension properties survive the round-trip. Then implement `from_vcard.rs` and `to_vcard.rs`, porting test cases from the existing `kith-vcard` test suite.

3. **Phase 3 (dav-server)** — Spike first: read `dav-server` 0.11 source to confirm REPORT coverage. Implement a minimal `DavFileSystem` backed by the store and verify it passes basic CardDAV client smoke tests (Apple Contacts, DAVx⁵) before deleting the old handlers.

4. **Phase 4 (tui-textarea)** — Can be done at any point; lowest risk and highest independence from other phases.

---

## What Is Not Changing

- **The `kith-core` crate** — `FactValue`, `ContactStore`, `ResolvedFact`, all domain types are correct and stay exactly as-is.
- **The event-sourcing model** — facts are still immutable, lifecycle still lives in separate tables, temporal queries still work.
- **The JSON API** (`kith-api`) — no changes.
- **The `X-KITH-*` extension properties** — `X-KITH-SOCIAL`, `X-KITH-GROUP`, `X-KITH-RELATION` are preserved in the vCard mapping layer.
- **Authentication** — Argon2 Basic Auth stays.
- **Configuration** — TOML config and CLI argument handling are unchanged.
- **Single-file SQLite deployment model** — still a personal tool, still zero infrastructure.

---

## Appendix: Crates Evaluated and Rejected

| Crate | Reason not selected |
|---|---|
| `vcard4` (tmpfs/vcard4) | vCard 4.0 only; no 3.0 support; small community |
| `vcard` | vCard 4.0 only; 2.1/3.0 listed as TODO; outdated dependencies |
| `vcard_parser` | vCard 4.0 only; 139 downloads/month; minimal activity |
| `ical` (Peltoche/ical-rs) | Archived August 2024 |
| `vobject` | Self-described RFC non-compliance; unstable API |
| `sea-orm` | ORM abstraction is a poor fit for the append-only domain model; adds 11MB of dependencies for no benefit on kith's query patterns |
| `tui-realm` | Full component architecture is overkill until the TUI grows to 5+ screens with complex form editing; revisit if the app expands |
| `rustydav` | WebDAV client, not server |
| `webdav-xml` | RFC 4918 elements only, no CardDAV namespace, version 0.1.0 |
