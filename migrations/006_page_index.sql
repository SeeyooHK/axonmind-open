-- 006_page_index.sql — PageIndex vectorless retrieval. Monotonic version: 6.
-- Derived retrieval index, SEPARATE from the semantic graph tables.
INSERT OR IGNORE INTO schema_version (version) VALUES (6);

-- Per-document metadata (top-level navigation / doc summary).
CREATE TABLE IF NOT EXISTS page_tree (
    doc_node_id  TEXT PRIMARY KEY,
    sha256       TEXT NOT NULL,
    title        TEXT NOT NULL,
    doc_summary  TEXT,
    built_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_page_tree_sha ON page_tree(sha256);

-- One row per section. parent_section_id reconstructs the tree if ever needed.
CREATE TABLE IF NOT EXISTS page_sections (
    section_id        TEXT PRIMARY KEY,
    doc_node_id       TEXT NOT NULL,
    parent_section_id TEXT,
    ordinal           INTEGER NOT NULL,
    level             INTEGER NOT NULL,
    title             TEXT NOT NULL,
    path              TEXT NOT NULL,
    summary           TEXT,
    text              TEXT,
    span_start        INTEGER NOT NULL,
    span_end          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_page_sections_doc ON page_sections(doc_node_id);

-- BM25 search surface. Manually synced by PageIndexStore on every section write.
-- Indexed: title, path, summary, text. section_id and doc_node_id are UNINDEXED.
CREATE VIRTUAL TABLE IF NOT EXISTS page_section_fts USING fts5(
    section_id   UNINDEXED,
    doc_node_id  UNINDEXED,
    title,
    path,
    summary,
    text
);
