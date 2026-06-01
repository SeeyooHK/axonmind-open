-- Phase 4: generation tracking + source versioning
-- Additive only. Does NOT touch document_cache.

CREATE TABLE IF NOT EXISTS generation (
    id         TEXT PRIMARY KEY,   -- UUID v4
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS generation_source (
    generation_id TEXT    NOT NULL REFERENCES generation(id) ON DELETE CASCADE,
    path          TEXT    NOT NULL,
    sha256        TEXT    NOT NULL,
    PRIMARY KEY (generation_id, path, sha256)
);

-- Per-path version label, content-keyed (D6).
-- First content at a path → version 1. Changed content → max+1.
-- Re-seeing a prior (path, sha256) pair reuses its recorded version number.
CREATE TABLE IF NOT EXISTS source_version (
    path          TEXT    NOT NULL,
    sha256        TEXT    NOT NULL,
    version       INTEGER NOT NULL,
    first_seen_at INTEGER NOT NULL,
    PRIMARY KEY (path, sha256)
);

CREATE INDEX IF NOT EXISTS idx_source_version_path ON source_version(path);

INSERT OR IGNORE INTO schema_version (version) VALUES (3);
