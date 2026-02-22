# Plan: Phase 2 — kith-vcard

## Context

Phase 1 delivered `kith-core` (all domain types + `ContactStore` trait) and `kith-store-sqlite` (full SQLite backend, 19 passing tests). Phase 2 implements the `kith-vcard` crate: a pure synchronous codec that converts between vCard 3.0/4.0 strings and `kith-core` domain types. This is the bridge that will allow the CardDAV server (Phase 3) to speak standard vCard with clients like Apple Contacts and Thunderbird.

The PLAN.md calls for "Implement vCard 3.0 and 4.0 parsing and serialization in `kith-vcard`. Map the full fact taxonomy to vCard fields. Fuzz the parser." This plan implements everything except actually running the fuzzer (a skeleton fuzz target will be noted but not wired up — cargo-fuzz requires nightly).

No external vCard crate is used. All mature Rust vCard crates target RFC 6350 only and can't be fuzzed easily. A custom content-line parser is ~350 lines, gives us full vCard 3.0 + 4.0 coverage, and is straightforwardly fuzzable.

---

## Files to Create / Modify

| File | Action |
|---|---|
| `crates/kith-vcard/Cargo.toml` | Add `chrono` and `uuid` workspace deps |
| `crates/kith-vcard/src/lib.rs` | Replace stub with public API |
| `crates/kith-vcard/src/error.rs` | New — error types |
| `crates/kith-vcard/src/parse.rs` | New — parser |
| `crates/kith-vcard/src/serialize.rs` | New — serializer |

---

## Public API (`lib.rs`)

```rust
pub struct ParsedVcard {
    pub uid:   Option<String>,   // from vCard UID property
    pub facts: Vec<NewFact>,     // subject_id = Uuid::nil(); caller fills in
}

pub fn parse(input: &str, source_name: &str) -> Result<ParsedVcard>
pub fn parse_many(input: &str, source_name: &str) -> Vec<Result<ParsedVcard>>
pub fn serialize(view: &ContactView) -> Result<String>     // vCard 4.0
pub fn serialize_v3(view: &ContactView) -> Result<String>  // vCard 3.0
```

Key contracts:
- All `NewFact.subject_id` from the parser = `Uuid::nil()` — caller must replace before recording
- All parsed facts get `RecordingContext::Imported { source_name, original_uid: uid }`
- Parser must never panic on any input (all errors returned as `Err`)
- Serializer uses CRLF line endings and folds at 75 octets per RFC 6350

---

## Error Type (`error.rs`)

```rust
pub enum Error {
    MissingEnvelope,
    MalformedContentLine(String),
    MalformedParam(String),
    MalformedN(usize),
    MalformedAdr(usize),
    InvalidDate { property: String, value: String },
    InvalidImppUri(String),
    UnsupportedVersion(String),
    InvalidPhotoPath(String),
    Json(#[from] serde_json::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
```

---

## Parser (`parse.rs`)

### Pipeline

```
raw &str
  └─ unfold_lines()       → Vec<String>   (join CRLF+SP continuation lines)
       └─ parse_content_line() → ContentLine { name, params, value }
            └─ map_property()  → accumulate into facts / name_accum / org_groups
                 └─ flush accumulators → final Vec<NewFact>
```

### Content-line representation
```rust
struct ContentLine { name: String, params: Vec<Param>, value: String }
struct Param { name: String, value: String }
```

### Key helpers
- `unfold_lines(s)` — joins lines beginning with SP/HT onto the previous line; tolerates bare LF (real-world robustness)
- `find_unquoted_colon(s)` — splits `name;params` from `value` respecting double-quoted param values
- `split_semicolons_respecting_quotes(s)` — splits parameter tokens
- `type_values(params)` — collects all `TYPE=` values (handles both `TYPE=A,B` and multiple `TYPE=` params)
- `pref_from_params(params, types)` — handles vCard 4.0 `PREF=N` and vCard 3.0 `TYPE=PREF`
- `parse_vcard_date(property, value)` — accepts `YYYYMMDD` and `YYYY-MM-DD`; returns `Err` for year-omitted `--MMDD` (skipped silently by caller)
- `decode_quoted_printable(s)` — minimal QP decoder for vCard 3.0 `ENCODING=QUOTED-PRINTABLE`

### Grouping accumulators

**Name**: `NameAccum { given, family, additional, prefix, suffix, full }` — populated by `N` and `FN` separately, flushed into one `FactValue::Name` at the end.

**OrgMembership**: `Vec<OrgGroup>` where each `OrgGroup { org_name, title, role }` — a new `OrgGroup` is pushed when `ORG` is seen; `TITLE`/`ROLE` attach to the last group. Flushed into one `FactValue::OrgMembership` per group at the end.

### Property → FactValue mapping

| vCard property | `FactValue` | Notes |
|---|---|---|
| `FN` | → `NameAccum.full` | Merged with `N` |
| `N` | → `NameAccum` fields | family;given;additional;prefix;suffix |
| `NICKNAME` | `Alias` | One fact per comma-separated token |
| `TEL` | `Phone` | TYPE=, PREF= / TYPE=PREF |
| `EMAIL` | `Email` | TYPE=, PREF= / TYPE=PREF |
| `ADR` | `Address` | 7-field semicolon-split; fields 0-1 (pobox, ext) discarded |
| `URL` | `Url` | TYPE= or heuristic on domain |
| `BDAY` | `Birthday` | Skip `--MMDD` year-omitted forms silently |
| `ANNIVERSARY` | `Anniversary` | Same |
| `GENDER` | `Gender` | First component before `;` |
| `ORG` | → `OrgGroup.org_name` | First semicolon component |
| `TITLE` | → `OrgGroup.title` | Attaches to last OrgGroup |
| `ROLE` | → `OrgGroup.role` | Attaches to last OrgGroup |
| `NOTE` | `Note` | |
| `PHOTO` (URI) | `Custom { key: "photo_uri" }` | No content_hash available; base64 photos dropped |
| `IMPP` | `Im` | `scheme:handle` split |
| `X-AIM`, `X-JABBER`, `X-SKYPE`, etc. | `Im` | vCard 3.0 legacy IM properties |
| `X-KITH-SOCIAL;PLATFORM=P` | `Social` | |
| `X-KITH-GROUP;GROUP-ID=uuid` | `GroupMembership` | |
| `X-KITH-RELATION;RELATION=r;OTHER-ID=uuid` | `Relationship` | |
| `X-KITH-MEETING;LOCATION=l` | `Meeting` | |
| `X-KITH-INTRODUCTION` | `Introduction` | |
| `X-*` (other) | `Custom { key: prop_name }` | |
| Unknown IANA | silently skipped | |

---

## Serializer (`serialize.rs`)

### Key helpers
- `fold_line(s)` — emits `s\r\n`; if `s.len() > 75`, splits at UTF-8 char boundary with CRLF + SP continuation
- `escape_value(s)` — escapes `\`, `,`, `;`, `\n` per RFC 6350
- `escape_component(s)` — escapes `\` and `;` (for N/ADR semicolon-delimited fields; commas are list separators within components)
- `build_type_param(types)` → `"TYPE=WORK,VOICE"`
- `build_pref_param(pref)` → `Some("PREF=1")` when `pref < 255`, else `None`

### FactValue → vCard lines

| `FactValue` | vCard output |
|---|---|
| `Name` | `FN:...` + `N:family;given;additional;prefix;suffix` |
| `Alias` | `NICKNAME:...` |
| `Photo` | `PHOTO;VALUE=URI:{path}` |
| `Birthday` | `BDAY:YYYYMMDD` |
| `Anniversary` | `ANNIVERSARY:YYYYMMDD` (v4) / `X-ANNIVERSARY:YYYYMMDD` (v3) |
| `Gender` | `GENDER:...` (v4 only; omitted in v3) |
| `Email` | `EMAIL;TYPE=WORK;PREF=1:addr` |
| `Phone` | `TEL;TYPE=HOME,CELL;PREF=2:num` |
| `Address` | `ADR;TYPE=WORK:;;street;city;region;postal;country` |
| `Url` | `URL;TYPE=github:https://...` |
| `Im` | `IMPP:xmpp:user@host` (v4) / `X-JABBER:user@host` etc. (v3) |
| `Social` | `X-KITH-SOCIAL;PLATFORM=Twitter:@alice` |
| `Relationship` | `X-KITH-RELATION;RELATION=sister;OTHER-ID=uuid:Name` |
| `OrgMembership` | `ORG:...` + `TITLE:...` + `ROLE:...` (single); `ORGn.ORG:...` etc. (multiple) |
| `GroupMembership` | `X-KITH-GROUP;GROUP-ID=uuid:Name` |
| `Note` | `NOTE:...` |
| `Meeting` | `X-KITH-MEETING;LOCATION=loc:summary` |
| `Introduction` | `X-KITH-INTRODUCTION:...` |
| `Custom` | `X-{KEY}:{json_value}` |

### Multiple OrgMemberships

When a `ContactView` has >1 `OrgMembership` fact, use RFC 6350 group prefixes:
```
ORG1.ORG:Acme Corp
ORG1.TITLE:Engineer
ORG2.ORG:Open Source Foundation
ORG2.TITLE:Board Member
```
Single org: no prefix (maximally compatible).

### vCard envelope

```
BEGIN:VCARD
VERSION:4.0
UID:{subject_id hyphenated}
PRODID:-//Kith//Kith vCard//EN
REV:{as_of as YYYYMMDDTHHMMSSz}
KIND:individual|org|group   (v4 only)
... facts ...
END:VCARD
```

### vCard 3.0 differences

| Element | v4 | v3 |
|---|---|---|
| `KIND` | emitted | omitted |
| `GENDER` | `GENDER:M` | omitted |
| `ANNIVERSARY` | `ANNIVERSARY:...` | `X-ANNIVERSARY:...` |
| `IMPP` | `IMPP:xmpp:...` | `X-JABBER:...` etc. |
| `PREF` | `;PREF=1` param | `TYPE=...,PREF` in TYPE list |
| `FN` + `N` | N optional | both required (emit empty `N:;;;;` if no Name fact) |

---

## Tests

Written alongside implementation in `#[cfg(test)]` modules within each file, following Phase 1 patterns.

### Parser tests
- Missing/malformed envelope → `Error::MissingEnvelope`
- `FN`-only and `N`+`FN` → single `Name` fact with correct fields
- `TEL` with `TYPE=WORK,VOICE;PREF=1` (v4) and `TYPE=WORK,PREF` (v3)
- `EMAIL` with preference roundtrip
- `ADR` 7-field split, label detection
- `BDAY` in `YYYYMMDD` and `YYYY-MM-DD`; `--MMDD` skipped gracefully
- `ORG`+`TITLE`+`ROLE` → single `OrgMembership`; two consecutive ORGs → two facts
- `IMPP:xmpp:...` → `Im`; `X-JABBER:...` → `Im { service: "XMPP" }`
- `X-KITH-SOCIAL`, `X-KITH-GROUP`, `X-KITH-RELATION`, `X-KITH-MEETING`, `X-KITH-INTRODUCTION`
- Folded lines (CRLF + SP continuation) unfolded correctly
- All facts have `RecordingContext::Imported` with correct `source_name` and `original_uid`
- `parse_many` with two-card input

### Serializer tests
- Envelope contains `BEGIN:VCARD`, `VERSION:4.0`, `UID:`, `END:VCARD`
- Name → `FN:` + `N:family;given;;;`
- Email with `TYPE=WORK;PREF=1`
- Phone without `PREF` param when `preference == 255`
- Lines > 75 bytes are folded; each continuation line ≤ 75 bytes
- Semicolons in address components are escaped
- Two OrgMembership facts → `ORG1.ORG:` / `ORG2.ORG:` group prefixes
- `X-KITH-SOCIAL` emitted correctly
- v3: `ANNIVERSARY` → `X-ANNIVERSARY`; `KIND` omitted; PREF as `TYPE=...,PREF`

### Round-trip test
`ContactView → serialize → parse → verify all fact values preserved` for a fully-populated contact (name, email, phone, address, org, note, social, relationship).

---

## Verification

```bash
cargo test -p kith-vcard          # all new tests pass
cargo test                        # entire workspace still passes (19 existing tests)
cargo clippy --all-targets        # no new warnings
```
