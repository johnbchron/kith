//! SQL schema for the Kith SQLite store.
//!
//! Executed once at connection startup via `PRAGMA user_version`. Future
//! migrations will be gated on that version number.

/// Full schema DDL; idempotent thanks to `CREATE TABLE IF NOT EXISTS`.
pub const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS subjects (
    subject_id  TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL,
    kind        TEXT NOT NULL    -- 'person' | 'organization' | 'group'
);

-- Facts are strictly append-only.
-- No UPDATE or DELETE is ever issued against this table.
CREATE TABLE IF NOT EXISTS facts (
    fact_id           TEXT PRIMARY KEY,
    subject_id        TEXT NOT NULL REFERENCES subjects(subject_id),
    fact_type         TEXT NOT NULL,   -- discriminant of FactValue variant
    value_json        TEXT NOT NULL,   -- JSON payload (inner data only)
    recorded_at       TEXT NOT NULL,   -- ISO 8601 UTC; server-assigned
    effective_at      TEXT,            -- JSON-encoded EffectiveDate or NULL
    effective_until   TEXT,            -- JSON-encoded EffectiveDate or NULL
    source            TEXT,
    confidence        TEXT NOT NULL DEFAULT 'certain',
    recording_context TEXT NOT NULL DEFAULT '{\"kind\":\"manual\"}',
    tags              TEXT NOT NULL DEFAULT '[]'
);

-- A fact replaced by a newer corrected/updated version.
CREATE TABLE IF NOT EXISTS supersessions (
    supersession_id TEXT PRIMARY KEY,
    old_fact_id     TEXT NOT NULL REFERENCES facts(fact_id),
    new_fact_id     TEXT NOT NULL REFERENCES facts(fact_id),
    recorded_at     TEXT NOT NULL,
    UNIQUE (old_fact_id),
    CHECK  (old_fact_id != new_fact_id)
);

-- A fact withdrawn with no replacement.
CREATE TABLE IF NOT EXISTS retractions (
    retraction_id TEXT PRIMARY KEY,
    fact_id       TEXT NOT NULL REFERENCES facts(fact_id),
    reason        TEXT,
    recorded_at   TEXT NOT NULL,
    UNIQUE (fact_id)
);

CREATE INDEX IF NOT EXISTS facts_subject_idx  ON facts(subject_id);
CREATE INDEX IF NOT EXISTS facts_type_idx     ON facts(fact_type);
CREATE INDEX IF NOT EXISTS facts_recorded_idx ON facts(recorded_at);

PRAGMA user_version = 1;
";
