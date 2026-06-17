-- Migration 7: document version lineage (Phase 0, docs/document_versioning.md §2).
-- Authoritative per-version log. document_cache stays as path → current HEAD node;
-- this table records every version of a logical document and its parent/child chain.
-- HEAD is derived (max version_no per logical_doc_id). Old version nodes are retained,
-- not orphaned; `superseded=1` marks every version that a newer one has replaced (§3).
--
-- Backfill is done in code (store::backfill_document_versions), not here, so logical_doc_ids
-- are minted with the same UUID format as runtime and the backfilled/skipped counts can be
-- logged (Rule 12). Past lineage is unrecoverable and is never fabricated.
CREATE TABLE IF NOT EXISTS document_versions (
    logical_doc_id    TEXT    NOT NULL,            -- ldoc.<uuidv4>, minted at first ingest
    version_no        INTEGER NOT NULL,            -- 1-based, monotonic per logical doc
    node_id           TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    sha256            TEXT    NOT NULL,             -- = blob filename
    structural_sha256 TEXT,
    indexed_at        INTEGER NOT NULL,
    source_path       TEXT,                         -- path at this version (nullable)
    previous_node_id  TEXT,                         -- parent version's node_id (null for v1)
    superseded        INTEGER NOT NULL DEFAULT 0,   -- 1 once a newer version exists (§3)
    PRIMARY KEY (logical_doc_id, version_no)
);
CREATE INDEX IF NOT EXISTS idx_docver_node ON document_versions(node_id);
CREATE INDEX IF NOT EXISTS idx_docver_sha  ON document_versions(sha256);

INSERT OR IGNORE INTO schema_version (version) VALUES (7);
