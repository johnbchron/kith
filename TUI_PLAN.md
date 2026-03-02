# Kith TUI Plan

A keyboard-driven terminal UI for browsing, editing, and inspecting the kith contact store. Built in Rust using [`ratatui`](https://ratatui.rs/) inside the existing `kith-cli` crate.

---

## Tech Stack

| Concern | Choice | Notes |
|---|---|---|
| TUI framework | `ratatui` 0.29 | |
| Event loop | `crossterm` | |
| Async runtime | `tokio` | Already in workspace |
| API client | `reqwest` 0.12 | |
| Config | `kith-carddav` config.toml | Reuse `base_url`, `auth_username`, `auth_password` |
| Fuzzy matching | `nucleo` or `fuzzy-matcher` | |

```toml
# kith-cli/Cargo.toml
ratatui = "0.29"
crossterm = "0.28"
reqwest = { version = "0.12", features = ["json"] }
fuzzy-matcher = "0.3"
```

---

## Layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ kith  [/] search  [n]ew  [?] help                          2026-03-01│
├────────────────────────────┬─────────────────────────────────────────┤
│ Contacts (47)              │ Alice Pemberton                         │
│ ──────────────────         │ ─────────────────────────────────────── │
│ > Alice Pemberton          │ email   alice@example.com  (work)       │
│   Bob Nakamura             │ email   alice.p@gmail.com  (personal)   │
│   Carol Osei               │ phone   +1 555 234 5678   (mobile)      │
│   David Choi               │ org     Acme Corp · Senior Engineer     │
│   Erín Ní Fhaoláin         │ url     https://alice.dev               │
│   ...                      │ note    Met at RustConf 2024            │
│                            │                                         │
│                            │ ── History ──────────────────────────── │
│                            │  ● email alice@example.com  recorded    │
│                            │      2024-11-12 · supersedes old addr   │
│                            │  ⊘ email alice@corp.com     retracted   │
│                            │      2024-11-12 · left company          │
│                            │                                         │
│                            │ [e]dit  [r]etract  [h]istory  [t]ime    │
├────────────────────────────┴─────────────────────────────────────────┤
│ NORMAL  ↑↓ navigate  / search  Enter detail  q quit                  │
└──────────────────────────────────────────────────────────────────────┘
```

List + detail split; collapses to full-screen detail on narrow terminals.

---

## Screens

### Contact List

- Sorted list of subjects; name derived from active `Name` fact
- Subject kind icons: `👤` person, `🏢` org, `👥` group
- `/` opens inline fuzzy filter

### Contact Detail

- Tabs: **Facts** | **History** | **Raw**
- **Facts**: active facts grouped by type; each row shows value, label, tags, confidence
  - `e` → Edit Fact overlay; `r` → retract with reason; `a` → Add Fact
- **History**: chronological log — `●` active (green), `→` superseded (dim), `⊘` retracted (red)
- **Raw**: JSON dump of the `Vec<ResolvedFact>` response

### Time-Travel

`t` opens a date picker; re-fetches via `GET /api/facts?subject_id=:id&as_of=<date>`. Banner shows selected date. `Escape` returns to present.

### Edit / Add Fact

```
┌─────────────────────────────────┐
│ Add Fact                        │
│ Type    [email           ▼]     │
│ Value   alice@new.com           │
│ Label   work                    │
│ Conf.   [Certain         ▼]     │
│ Tags    []                      │
│      [Save]   [Cancel]          │
└─────────────────────────────────┘
```

Tab/Shift-Tab between fields; enum fields use a dropdown widget. Save → `POST /api/facts` (new) or `POST /api/facts/:id/supersede` (edit).

### New Contact

Kind + display name. `POST /api/subjects` then `POST /api/facts` for the name. Opens detail pane immediately.

---

## Navigation

| Key | Context | Action |
|---|---|---|
| `↑` / `k`, `↓` / `j` | List | Move up / down |
| `Enter` / `→` / `l` | List | Open detail |
| `←` / `h` / `Escape` | Detail | Back to list |
| `/` | List | Search |
| `Tab` / `Shift-Tab` | Detail | Next / previous tab |
| `a`, `e`, `r` | Detail / Facts | Add / edit / retract fact |
| `t` | Detail | Time-travel |
| `n` | Global | New contact |
| `d` | List | Delete subject (confirm first) |
| `?` | Global | Help |
| `q` / `Ctrl-C` | Global | Quit |

---

## Application Architecture

```
kith-cli/src/
├── main.rs            # Arg parsing, config loading, enter TUI
├── app.rs             # App state machine, event dispatch
├── client.rs          # Async HTTP client wrapping kith-api endpoints
├── ui/
│   ├── mod.rs
│   ├── layout.rs
│   ├── contact_list.rs
│   ├── contact_detail.rs
│   ├── history.rs
│   ├── edit_form.rs
│   ├── time_travel.rs
│   ├── help.rs
│   └── widgets/
│       ├── fact_row.rs
│       ├── confidence_badge.rs
│       └── dropdown.rs
└── keys.rs
```

### State Machine

```rust
enum Screen {
    ContactList,
    ContactDetail { tab: DetailTab },
    EditFact { subject_id: Uuid, editing: Option<Uuid> },  // None = new
    NewContact,
    RetractConfirm { fact_id: Uuid },
    TimeTravel { subject_id: Uuid, date: NaiveDate },
    Help,
}
```

`App` holds: `screen`, `subjects: Vec<Subject>`, `names: HashMap<Uuid, String>`, `filter`, `selected_contact: Option<Uuid>`, `facts: Option<Vec<ResolvedFact>>`, `client: Arc<ApiClient>`.

API calls are `await`ed inline; on completion the relevant cached data is invalidated and re-fetched.

---

## Colours

- Active: default foreground
- Superseded: dim
- Retracted: red
- Confidence `Rumored`: yellow
- Selected row: reverse video

---

## Implementation Phases

### Phase A — Skeleton (read-only)

1. `GET /api/subjects` on startup; name facts loaded lazily
2. Contact list with fuzzy filter
3. Contact detail (Facts tab) via `GET /api/facts?subject_id=:id`
4. Navigation: `j/k`, `Enter`, `Escape`, `q`

### Phase B — Full Read

5. History tab via `GET /api/facts?subject_id=:id&include_inactive=true`
6. Time-travel (`t`, date picker, `?as_of=`)
7. Help overlay

### Phase C — Writes

8. Add fact (`a`, `POST /api/facts`)
9. Edit fact (`e`, `POST /api/facts/:id/supersede`)
10. Retract fact (`r`, `POST /api/facts/:id/retract`)
11. New contact (`n`, `POST /api/subjects` + `POST /api/facts`)
12. Delete contact (`d`, confirm, retract all facts)

### Phase D — Polish

13. Mouse support
14. Background refresh via `GET /api/events` SSE
15. Toast notifications
16. Configurable colour themes
17. Export contact as vCard to stdout

---

## Open Questions

1. **Config**: `--config <path>` pointing to an existing `config.toml`, or bare `--url` / `--user` / `--password` flags?
2. **Async in ratatui**: `tokio::task::block_in_place` in the event handler (simple) vs. a dedicated async runtime with an `mpsc` channel (cleaner).
