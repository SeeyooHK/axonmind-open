-- 010_document_identity_and_locators.sql — document identity + legal locators. Monotonic version: 10.
INSERT OR IGNORE INTO schema_version (version) VALUES (10);

CREATE TABLE IF NOT EXISTS document_identity (
    doc_node_id      TEXT PRIMARY KEY,
    source_filename  TEXT NOT NULL,
    source_path      TEXT,
    raw_title        TEXT,
    canonical_title  TEXT NOT NULL,
    language         TEXT,
    jurisdiction     TEXT,
    domain           TEXT,
    instrument_type  TEXT,
    corpus           TEXT,
    confidence       REAL NOT NULL DEFAULT 0.0,
    reviewed_at      INTEGER,
    updated_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS document_aliases (
    doc_node_id  TEXT NOT NULL,
    alias        TEXT NOT NULL,
    alias_norm   TEXT NOT NULL,
    source       TEXT NOT NULL,
    PRIMARY KEY (doc_node_id, alias_norm)
);
CREATE INDEX IF NOT EXISTS idx_document_aliases_alias_norm ON document_aliases(alias_norm);

CREATE TABLE IF NOT EXISTS legal_units (
    unit_id          TEXT PRIMARY KEY,
    doc_node_id      TEXT NOT NULL,
    parent_unit_id   TEXT,
    section_id       TEXT,
    unit_type        TEXT NOT NULL,
    label            TEXT NOT NULL,
    label_norm       TEXT NOT NULL,
    article          TEXT,
    recital          TEXT,
    section_label    TEXT,
    paragraph        TEXT,
    title            TEXT,
    ordinal          INTEGER NOT NULL,
    level            INTEGER NOT NULL,
    text             TEXT NOT NULL,
    span_start       INTEGER NOT NULL,
    span_end         INTEGER NOT NULL,
    page_start       INTEGER,
    page_end         INTEGER,
    path             TEXT NOT NULL,
    parser_profile   TEXT NOT NULL,
    confidence       REAL NOT NULL DEFAULT 1.0
);
CREATE INDEX IF NOT EXISTS idx_legal_units_doc_label ON legal_units(doc_node_id, label_norm);
CREATE INDEX IF NOT EXISTS idx_legal_units_doc_article ON legal_units(doc_node_id, article, paragraph);
CREATE INDEX IF NOT EXISTS idx_legal_units_doc_recital ON legal_units(doc_node_id, recital);
CREATE INDEX IF NOT EXISTS idx_legal_units_doc_section ON legal_units(doc_node_id, section_label);
CREATE INDEX IF NOT EXISTS idx_legal_units_doc_type ON legal_units(doc_node_id, unit_type);
CREATE INDEX IF NOT EXISTS idx_legal_units_section ON legal_units(section_id);

CREATE TABLE IF NOT EXISTS legal_unit_refs (
    from_unit_id        TEXT NOT NULL,
    to_doc_node_id      TEXT,
    target_label        TEXT NOT NULL,
    target_label_norm   TEXT NOT NULL,
    ref_text            TEXT NOT NULL,
    PRIMARY KEY (from_unit_id, target_label_norm, ref_text)
);
CREATE INDEX IF NOT EXISTS idx_legal_refs_target ON legal_unit_refs(to_doc_node_id, target_label_norm);

CREATE VIRTUAL TABLE IF NOT EXISTS document_identity_fts USING fts5(
    doc_node_id UNINDEXED,
    canonical_title,
    aliases,
    source_filename,
    corpus,
    jurisdiction,
    domain,
    instrument_type
);
