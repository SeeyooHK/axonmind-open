-- axonmind-open initial schema
-- Monotonic version: 1
-- Apply via axonmind_engine::store::migrations::run_migrations_on_init

PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

-- Schema version tracker (monotonic integer, not SemVer)
CREATE TABLE IF NOT EXISTS schema_version (
    version  INTEGER PRIMARY KEY
);
INSERT OR IGNORE INTO schema_version (version) VALUES (1);

-- Core node table.
-- attrs: MessagePack-encoded JSON (kind-specific payload, e.g. KpiAttrs).
-- Cannot be queried directly — use search_index for text search.
CREATE TABLE IF NOT EXISTS nodes (
    id                    TEXT PRIMARY KEY,
    kind                  TEXT NOT NULL,
    name                  TEXT NOT NULL,
    attrs                 BLOB NOT NULL,
    confidence            REAL NOT NULL,
    is_tainted            INTEGER NOT NULL DEFAULT 0,
    requires_human_review INTEGER NOT NULL DEFAULT 0,
    created_at            INTEGER NOT NULL,  -- Unix timestamp (seconds)
    updated_at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nodes_kind    ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_updated ON nodes(updated_at);

-- FTS5 search index over node text fields.
-- Manually synced by GraphStore::apply_mutation on every write. See store/sqlite.rs for sync rules.
-- node_id and kind are UNINDEXED so FTS does not tokenise them; name, definition, evidence_quotes are indexed.
CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
    node_id       UNINDEXED,
    kind          UNINDEXED,
    name,
    definition,
    evidence_quotes
);

-- Edges between nodes. All writes go through GraphMutation::UpsertEdge.
-- Invariant: every edge must have at least one row in edge_evidence. Enforced by GraphStore.
CREATE TABLE IF NOT EXISTS edges (
    id                    TEXT PRIMARY KEY,
    from_id               TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    to_id                 TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind                  TEXT NOT NULL,
    confidence            REAL NOT NULL,
    created_by            TEXT NOT NULL,  -- ExtractorKind
    is_tainted            INTEGER NOT NULL DEFAULT 0,
    requires_human_review INTEGER NOT NULL DEFAULT 0,
    created_at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_id, kind);
CREATE INDEX IF NOT EXISTS idx_edges_to   ON edges(to_id, kind);

-- Evidence records. Source of truth for confidence and taint.
CREATE TABLE IF NOT EXISTS evidence (
    id                    TEXT PRIMARY KEY,
    source_node_id        TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    source_type           TEXT NOT NULL,  -- SourceType enum
    quote                 TEXT,
    row_ref               TEXT,           -- e.g. "Sheet1!B7" or "p.12 §3"
    blob_sha256           TEXT,           -- SHA-256 of file in blobs/ directory
    timestamp             INTEGER,
    extractor             TEXT NOT NULL,  -- ExtractorKind enum
    confidence            REAL NOT NULL,
    is_tainted            INTEGER NOT NULL DEFAULT 0,
    requires_human_review INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_evidence_source ON evidence(source_node_id);
CREATE INDEX IF NOT EXISTS idx_evidence_blob   ON evidence(blob_sha256);

-- Junction table: which evidence backs which edge.
CREATE TABLE IF NOT EXISTS edge_evidence (
    edge_id     TEXT NOT NULL REFERENCES edges(id) ON DELETE CASCADE,
    evidence_id TEXT NOT NULL REFERENCES evidence(id) ON DELETE CASCADE,
    PRIMARY KEY (edge_id, evidence_id)
);

-- Document ingestion cache. Used by kpi_recompute_worker to detect stale files.
-- sha256 is the hash at last-index time; if current file hash differs, re-ingest.
CREATE TABLE IF NOT EXISTS document_cache (
    path       TEXT PRIMARY KEY,
    sha256     TEXT NOT NULL,
    indexed_at INTEGER NOT NULL,
    node_id    TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE
);

-- Historical metric values per KPI. Appended, never updated. Enables trend computation.
CREATE TABLE IF NOT EXISTS metric_values (
    id             TEXT PRIMARY KEY,
    kpi_node_id    TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    metric_node_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    value          REAL NOT NULL,
    unit           TEXT NOT NULL,
    period_start   INTEGER,
    period_end     INTEGER,
    observed_at    INTEGER NOT NULL,
    evidence_id    TEXT NOT NULL REFERENCES evidence(id)
);
CREATE INDEX IF NOT EXISTS idx_metric_kpi ON metric_values(kpi_node_id, observed_at);

-- KPI discovery candidates. Populated by kpi_discovery_worker; requires human approval.
-- status: pending | approved | rejected | merged
-- merged_into: node_id of the KPI node created on approval, NULL otherwise.
CREATE TABLE IF NOT EXISTS kpi_candidates (
    id          TEXT PRIMARY KEY,           -- UUID v4
    name        TEXT NOT NULL,
    definition  TEXT,
    detected_in TEXT NOT NULL,              -- JSON array of document node_ids
    confidence  REAL NOT NULL,
    proposed_at INTEGER NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    merged_into TEXT                        -- REFERENCES nodes(id), nullable
);
CREATE INDEX IF NOT EXISTS idx_candidates_status ON kpi_candidates(status);
