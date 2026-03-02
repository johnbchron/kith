# Kith HTTP API Plan

A JSON REST API built in a standalone `kith-api` crate and mounted into `kith-carddav`. Facts are the first-class primitive; subjects are thin envelopes. The TUI receives raw `ResolvedFact` arrays and owns all display logic.

---

## Architectural Choice

New `kith-api` crate depends only on `kith-core` and `axum`. Auth, config, and transport remain `kith-carddav`'s responsibility. `kith-cli` can embed the same router without pulling in CardDAV.

```
kith-core
  ├── kith-store-sqlite
  ├── kith-vcard
  ├── kith-api          ← new
  └── kith-carddav
        ├── kith-store-sqlite
        ├── kith-vcard
        └── kith-api
```

```rust
// kith-carddav/src/lib.rs
.nest("/api", kith_api::api_router(store.clone()))
```

---

## Endpoints

### Subjects

| Method | Path | Store call | Notes |
|---|---|---|---|
| `GET` | `/api/subjects` | `list_subjects(kind)` | Optional `?kind=person\|organization\|group` |
| `POST` | `/api/subjects` | `add_subject(kind)` | Body: `{"kind": "person"}` |
| `GET` | `/api/subjects/:id` | `get_subject(id)` | 404 if not found |

### Facts

| Method | Path | Store call | Notes |
|---|---|---|---|
| `GET` | `/api/facts` | `get_facts(subject_id, as_of, include_inactive)` | See query params below |
| `GET` | `/api/facts/:id` | `get_facts` + filter by id | Returns a single `ResolvedFact` |
| `POST` | `/api/facts` | `record_fact(NewFact)` | Body: `NewFact`; `subject_id` in body |
| `POST` | `/api/facts/:id/supersede` | `supersede(old_id, replacement)` | Body: replacement `NewFact` |
| `POST` | `/api/facts/:id/retract` | `retract(fact_id, reason)` | Body: `{"reason": "..."}` |

`GET /api/facts` query params: `subject_id` (required), `fact_type`, `as_of` (RFC3339), `include_inactive` (default false).

### Search

`GET /api/search` → `Vec<Subject>`. Params map directly to `FactQuery` fields: `text`, `kind`, `fact_types`, `tags`, `confidence`, `recorded_after`, `recorded_before`, `limit`, `offset`.

---

## What the TUI Calls and When

| TUI event | Endpoint |
|---|---|
| Startup | `GET /api/subjects` + `GET /api/facts?subject_id=:id&fact_type=name` per subject (lazy) |
| Contact selected | `GET /api/facts?subject_id=:id` |
| History tab | `GET /api/facts?subject_id=:id&include_inactive=true` |
| Time-travel | `GET /api/facts?subject_id=:id&as_of=<date>` |
| Search | `GET /api/search?text=<query>` → subject list → name facts lazily |
| Add / edit / retract | `POST /api/facts`, `POST /api/facts/:id/supersede`, `POST /api/facts/:id/retract` |
| New contact | `POST /api/subjects` then `POST /api/facts` |

---

## Response Shape

All meaningful responses are `Vec<ResolvedFact>`. The status discriminant drives TUI rendering:

```json
[
  { "fact": { "fact_id": "...", "subject_id": "...", "value": { "Name": { ... } }, "..." }, "status": "Active" },
  { "fact": { ... }, "status": { "Superseded": { "by": "uuid", "at": "..." } } },
  { "fact": { ... }, "status": { "Retracted": { "reason": "left company", "at": "..." } } }
]
```

---

## Notes

**Auth** is applied by `kith-carddav`'s existing basic-auth layer wrapping the mounted router. `kith-api` is middleware-free.

**Phase D SSE**: add `GET /api/events` as an SSE stream (`axum::response::sse`) emitting `fact-recorded`, `fact-superseded`, and `fact-retracted` for background refresh.

---

## File Layout

```
crates/kith-api/
├── Cargo.toml          # kith-core, axum, serde, tokio, uuid, chrono
└── src/
    ├── lib.rs          # pub fn api_router<S: ContactStore>(store: Arc<S>) -> Router
    ├── subjects.rs
    ├── facts.rs
    └── search.rs
```

```
crates/kith-carddav/
├── Cargo.toml          # + kith-api = { path = "../kith-api" }
└── src/lib.rs          # + .nest("/api", kith_api::api_router(store.clone()))
```
