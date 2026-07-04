-- 011_structure_packages.sql — generic structure-package metadata and parsed unit storage.
INSERT OR IGNORE INTO schema_version (version) VALUES (11);

CREATE TABLE IF NOT EXISTS structure_packages (
    package_name   TEXT PRIMARY KEY,
    version        INTEGER NOT NULL,
    description    TEXT,
    content_sha    TEXT NOT NULL,
    imported_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS structure_package_sources (
    package_name   TEXT NOT NULL,
    source         TEXT NOT NULL,
    imported_at    INTEGER NOT NULL,
    PRIMARY KEY (package_name, source)
);

CREATE TABLE IF NOT EXISTS structure_profiles (
    package_name   TEXT NOT NULL,
    profile_name   TEXT NOT NULL,
    version        INTEGER NOT NULL,
    definition     TEXT NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (package_name, profile_name)
);

CREATE TABLE IF NOT EXISTS identity_rules (
    package_name   TEXT NOT NULL,
    ordinal        INTEGER NOT NULL,
    definition     TEXT NOT NULL,
    PRIMARY KEY (package_name, ordinal)
);

CREATE TABLE IF NOT EXISTS corpus_bindings (
    package_name   TEXT NOT NULL,
    ordinal        INTEGER NOT NULL,
    kind           TEXT NOT NULL,
    definition     TEXT NOT NULL,
    PRIMARY KEY (package_name, kind, ordinal)
);

CREATE TABLE IF NOT EXISTS doc_units (
    unit_id          TEXT PRIMARY KEY,
    doc_node_id      TEXT NOT NULL,
    parent_unit_id   TEXT,
    section_id       TEXT,
    unit_kind        TEXT NOT NULL,
    label            TEXT NOT NULL,
    label_norm       TEXT NOT NULL,
    title            TEXT,
    ordinal          INTEGER NOT NULL,
    level            INTEGER NOT NULL,
    text             TEXT NOT NULL,
    span_start       INTEGER NOT NULL,
    span_end         INTEGER NOT NULL,
    page_start       INTEGER,
    page_end         INTEGER,
    path             TEXT NOT NULL,
    citation         TEXT NOT NULL,
    package_name     TEXT NOT NULL,
    profile_name     TEXT NOT NULL,
    profile_version  INTEGER NOT NULL,
    confidence       REAL NOT NULL DEFAULT 1.0
);
CREATE INDEX IF NOT EXISTS idx_doc_units_doc_label ON doc_units(doc_node_id, label_norm);
CREATE INDEX IF NOT EXISTS idx_doc_units_doc_kind  ON doc_units(doc_node_id, unit_kind);
CREATE INDEX IF NOT EXISTS idx_doc_units_section   ON doc_units(section_id);
CREATE INDEX IF NOT EXISTS idx_doc_units_package   ON doc_units(package_name, profile_name);

CREATE TABLE IF NOT EXISTS doc_unit_locators (
    unit_id      TEXT NOT NULL,
    doc_node_id  TEXT NOT NULL,
    key          TEXT NOT NULL,
    value        TEXT NOT NULL,
    PRIMARY KEY (unit_id, key)
);
CREATE INDEX IF NOT EXISTS idx_doc_unit_locators_lookup ON doc_unit_locators(doc_node_id, key, value);

CREATE TABLE IF NOT EXISTS doc_unit_refs (
    from_unit_id      TEXT NOT NULL,
    to_doc_node_id    TEXT,
    target_label      TEXT NOT NULL,
    target_label_norm TEXT NOT NULL,
    ref_text          TEXT NOT NULL,
    PRIMARY KEY (from_unit_id, target_label_norm, ref_text)
);
CREATE INDEX IF NOT EXISTS idx_doc_unit_refs_target ON doc_unit_refs(to_doc_node_id, target_label_norm);

ALTER TABLE document_identity ADD COLUMN pinned_profile TEXT;
